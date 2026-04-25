# 14 — Replay, prod-as-test-suite, and behavior-diff: what shipped (Phase 21)

This section is the **implementation reference** for Corvid's replay + governance layer — the flagship v1.0 invention that makes AI-safety guarantees survive into production operations, not just into the compiler. Earlier spec sections covered the *static* side of the thesis (effect algebra, approval contracts, grounding). Phase 21 closes the loop: once a program ships, every run writes a schema-versioned trace; those traces can be deterministically replayed against later code; production traffic becomes the regression suite; and every PR carries a signed behavior receipt.

The thesis in one sentence: **LLM-backed programs without deterministic replay are untestable in production; Corvid ships replay as a first-class language feature rather than an afterthought library.**

Slice trail shipped in Phase 21:

| Slice | Name | Delivers |
|---|---|---|
| 21-A-schema | Trace schema | `SCHEMA_VERSION`, `SchemaHeader`, `SeedRead` / `ClockRead`, JSONL format, forward-compat validation |
| 21-A-determinism-hooks | Determinism catalog | Every non-deterministic source the runtime recognises, each with a recording hook |
| 21-B-rec-interp / 21-B-rec-native | Recording | Interpreter + native tiers emit identical trace shape |
| 21-C-replay-interp / 21-C-replay-native | Replay | Byte-identical post-replay state across tiers |
| 21-inv-A | `@replayable` checker | Compile-time guarantee an agent can be deterministically replayed |
| 21-inv-F | `@deterministic` checker | Stricter sibling: no network / LLM / tool / approve at all |
| 21-inv-B-adapter / 21-inv-B-cli / 21-inv-B-cli-wire | Differential replay | `corvid replay --model <id> <trace>` swaps one axis (LLM) and reports divergences |
| 21-inv-D-runtime / 21-inv-D-cli / 21-inv-D-cli-wire | Counterfactual mutation | `corvid replay --mutate STEP JSON <trace>` overrides one step, reports divergences |
| 21-inv-C-1 / 21-inv-C-2 | Provenance DAG | `ProvenanceEdge` event variant; `corvid trace dag` DOT renderer |
| 21-inv-E-1..E-4 / 21-inv-E-runtime | `replay` language primitive | `replay <trace>: when <pat> -> <expr> else <expr>` |
| 21-inv-G-cli / 21-inv-G-harness / 21-inv-G-cli-wire / 21-inv-G-cli-wire-promote | Prod-as-test-suite | `corvid test --from-traces`, including `--promote` with atomic rewrite |
| 21-inv-H-1 | PR behavior receipt | `corvid trace-diff`, reviewer-as-Corvid-program |
| 21-inv-I | Shadow daemon | Runtime-verify compile-time invariants against live traffic |

Every example below is a real `.cor` block that compiles against the current toolchain. `corvid test spec --meta` keeps them honest on every build.

## 14.1 The `@replayable` attribute (slice 21-inv-A)

`@replayable` is a compile-time claim that an agent's execution can be deterministically reproduced from a trace of its external effects. The checker enforces the claim by rejecting any code path that reads from a non-recorded non-deterministic source (random, clock, environment, network, un-recorded tool) without routing it through the runtime's deterministic catalog.

```corvid
# expect: compile
prompt classify(text: String) -> String:
    """Classify the sentiment of {text}. Reply with positive, negative, or neutral."""

@replayable
agent classify_inbox(message: String) -> String:
    return classify(message)
```

Prompts are deterministic from a replay standpoint — their `LlmCall` / `LlmResult` events are substituted verbatim during replay. Tool calls likewise. Anything that reads a clock or PRNG seed must go through a `SeedRead` / `ClockRead` event so replay can substitute the recorded value.

An agent that touches an un-recorded source is rejected at compile time. Implementation reference: [crates/corvid-types/src/checker/replayable.rs](../../crates/corvid-types/src/checker/replayable.rs).

## 14.2 The `@deterministic` attribute (slice 21-inv-F)

`@deterministic` is `@replayable`'s stricter sibling: no LLM calls at all, no tool dispatch, no `approve`. The function computes purely from its arguments. Used for receipt renderers, pure data transforms, and anything that needs to be re-runnable offline without a runtime.

```corvid
# expect: compile
@deterministic
agent render_summary(name: String, count_note: String) -> String:
    return "# " + name + "\n\n" + count_note + "\n"
```

The PR behavior-diff reviewer shipped in §14.8 is itself a `@deterministic` agent — that is load-bearing for CI memoisation.

## 14.3 Trace schema (slice 21-A-schema)

Every run writes a newline-delimited JSON (`.jsonl`) trace. The first line is always a `SchemaHeader` carrying the `SCHEMA_VERSION` + run metadata; subsequent lines are event records.

```json
{"type":"SchemaHeader","schema_version":3,"run_id":"2026-04-22-ab12","ts_ms":1745000000000,"source_path":"src/classify.cor"}
{"type":"RunStarted","ts_ms":1745000000001,"run_id":"2026-04-22-ab12","agent":"classify_inbox","args":["hello"]}
{"type":"LlmCall","ts_ms":1745000000002,"run_id":"2026-04-22-ab12","prompt":"classify","model":"claude-sonnet-4-6","args":["hello"]}
{"type":"LlmResult","ts_ms":1745000000010,"run_id":"2026-04-22-ab12","prompt":"classify","model":"claude-sonnet-4-6","result":"positive"}
{"type":"RunCompleted","ts_ms":1745000000011,"run_id":"2026-04-22-ab12","ok":true,"result":"positive"}
```

`SCHEMA_VERSION` is monotonically increasing; the runtime's `validate_supported_schema` rejects traces older than `MIN_SUPPORTED_SCHEMA_VERSION` with a typed error so a future compiler can refuse to replay a stale trace instead of mis-substituting. Implementation reference: [crates/corvid-trace-schema/src/event.rs](../../crates/corvid-trace-schema/src/event.rs).

Event variants that can substitute during replay: `ToolCall`, `ToolResult`, `LlmCall`, `LlmResult`, `ApprovalRequest`, `ApprovalResponse`, `ApprovalDecision`, `SeedRead`, `ClockRead`. `ProvenanceEdge` records provenance graph structure (§14.9) but does not substitute.

## 14.4 Replay modes (slices 21-inv-B / 21-inv-D)

`corvid replay <trace>` re-executes a recorded run against the current code, substituting every recorded external response. Three modes compose on top of that primitive:

- **Plain replay** (default). Byte-identical reproduction. Useful for golden-trace regression testing.
- **Differential replay** (`--model <id>`). Every `LlmCall` event dispatches to the named live model instead of the recorded substitution; every other axis (tool, approval, clock, seed) replays strict. The command reports `ReplayDifferentialReport` with a per-prompt divergence list.
- **Counterfactual mutation** (`--mutate STEP JSON`). One substitutable event (at 1-based `STEP` among events of that kind) is overridden with the user-supplied JSON; every other event replays strict. Useful for "what if the LLM had said X instead?" experiments.

```text
corvid replay trace.jsonl                                        # plain
corvid replay trace.jsonl --model claude-opus-5-0                # differential
corvid replay trace.jsonl --mutate 3 '{"should_refund": false}'  # counterfactual
```

Implementation reference: [crates/corvid-runtime/src/replay/mod.rs](../../crates/corvid-runtime/src/replay/mod.rs), [crates/corvid-driver/src/replay.rs](../../crates/corvid-driver/src/replay.rs).

## 14.5 The `replay` language primitive (slices 21-inv-E-1..E-4, 21-inv-E-runtime)

Replay is also a first-class expression, so a Corvid program can examine a trace event-by-event and react. Pattern syntax matches against event kind; `as <ident>` tails capture event-specific fields; an `else` arm handles unmatched events.

```corvid
# expect: skip
# Replay block pattern syntax is the normative design; parser support is tracked
# separately from the runtime replay commands that already ship.
prompt classify(text: String) -> String:
    """Classify {text}."""

@replayable
agent summarise_run(trace: TraceId) -> Int:
    llm_calls = 0
    tool_calls = 0
    replay trace:
        when LlmCall -> llm_calls = llm_calls + 1
        when ToolCall -> tool_calls = tool_calls + 1
        else -> pass
    return llm_calls + tool_calls
```

The `TraceId` / `TraceEvent` types are built-in to the resolver. Pattern exhaustiveness is checked at compile time — a `replay` block that doesn't cover every substitutable event variant is rejected unless it has an `else` arm. Implementation reference: [crates/corvid-types/src/checker/replay.rs](../../crates/corvid-types/src/checker/replay.rs), [crates/corvid-ir/src/lower/replay.rs](../../crates/corvid-ir/src/lower/replay.rs).

## 14.6 Prod-as-test-suite (slice 21-inv-G family)

A directory of recorded `.jsonl` traces is a regression suite. `corvid test --from-traces <DIR> --from-traces-source <FILE>` loads + schema-validates every trace, replays each against the current source, and reports per-trace verdicts.

```text
corvid test --from-traces traces/ --from-traces-source src/classify.cor
```

Exit code `0` on a clean run; `1` when at least one trace diverged, flaked, or errored; `bail` on hard errors. Seven inventive filter flags compose on top of the base behaviour:

| Flag | Behaviour |
|---|---|
| `--replay-model <ID>` | Cross-model differential replay over every trace |
| `--only-dangerous` | Filter to traces that hit a `@dangerous` tool (detected by `ApprovalRequest` events — the approve-before-dangerous guarantee makes this exact) |
| `--only-prompt <NAME>` | Filter to traces exercising the named prompt |
| `--only-tool <NAME>` | Filter to traces exercising the named tool |
| `--since <RFC3339>` | Filter to traces with any event at or after the timestamp |
| `--promote` | Jest-snapshot mode: TTY prompts per divergence and atomically rewrites the golden trace when accepted; non-TTY fails closed with a one-time warning |
| `--flake-detect <N>` | Replay each trace `N` times; any trace producing different output across runs surfaces program-level nondeterminism the `@deterministic` attribute didn't catch |

`--promote` is the critical governance move. On CI, a non-TTY pipeline with `--promote` always rejects — golden traces only change through a human-in-the-loop decision. On a developer's terminal, a `[y/N/a/q]` prompt accepts, rejects, accepts-all, or quits; accepted divergences are atomically written over the old trace, which then becomes the new golden. Implementation reference: [crates/corvid-runtime/src/test_from_traces.rs](../../crates/corvid-runtime/src/test_from_traces.rs), [crates/corvid-cli/src/test_from_traces.rs](../../crates/corvid-cli/src/test_from_traces.rs), [crates/corvid-driver/src/trace_fresh.rs](../../crates/corvid-driver/src/trace_fresh.rs).

The inventive axis: **production traffic is the test suite**, and that only becomes real when the CLI actually runs the traces against the current binary, prints a per-trace verdict, and — when behaviour genuinely changed for the better — lets the operator promote the current run to the new golden instead of having them re-record by hand. Jest proved the pattern works for snapshot testing; Corvid is the first language that brings it to LLM-backed orchestration.

## 14.7 Behavior-diff PR receipt (slice 21-inv-H-1)

`corvid trace-diff <base-sha> <head-sha> <path>` extracts the 22-B ABI descriptor from the source at two git revisions and renders a markdown PR behavior receipt describing every algebraic change between them — trust-tier changes, `@dangerous` transitions, `@replayable` transitions, added / removed exported agents.

```text
corvid trace-diff HEAD~1 HEAD src/agent.cor
```

The tool's design load-bearing claim: **the reviewer is itself a Corvid program.** The `.cor` source of the reviewer agent lives at [crates/corvid-cli/src/trace_diff/reviewer.cor](../../crates/corvid-cli/src/trace_diff/reviewer.cor) and is baked into the CLI via `include_str!`. It is `@deterministic`, so two invocations on the same (base-sha, head-sha) triple produce byte-identical receipts — CI can memoise. That property is not achievable by a Rust reviewer; Corvid's type checker enforces it.

Receipt scope is the *exported surface* — `pub extern "c"` agents and their transitive closure, matching the 22-B ABI boundary that hosts actually consume. Private helpers that change often but never cross the boundary do not appear in the receipt, so the tool never cries wolf on internal refactoring.

Follow-up slices shipped after the first receipt surface: `21-inv-H-2` counterfactual replay over `--traces <dir>`, `21-inv-H-3` structured approval + provenance drill-down, `21-inv-H-4` grounded receipt narratives, and `21-inv-H-5` CI / signing / policy renderers.

## 14.8 Shadow daemon (slice 21-inv-I)

`corvid-shadow-daemon` is a long-running service that ingests live traffic (either tee'd from production or streamed from a log shipper), replays each trace against the current code, and raises alerts when divergences appear. It is the production side of the prod-as-test-suite thesis: tests run by CI catch regressions at merge time; the shadow daemon catches them after merge but before they affect the next user.

```text
corvid-shadow-daemon \
    --source src/agent.cor \
    --trace-stream /var/log/corvid/traces/ \
    --alert-webhook https://alerts.example/hook
```

Implementation reference: [crates/corvid-shadow-daemon](../../crates/corvid-shadow-daemon/).

The daemon can replay either interpreter-recorded or native-recorded traces. The execution tier is explicit in the config:

```toml
[daemon]
trace_dir = "target/trace"
ir_path = "src/agent.cor"
execution_tier = "native"
alert_log = "target/shadow/alerts.jsonl"
```

`execution_tier = "interpreter"` is the default. `execution_tier = "native"` builds or reuses the native binary for `ir_path`, replays native traces through the native writer, and preserves the same differential and mutation report paths as interpreter shadow replay. Cross-tier replay is intentionally rejected: an interpreter-recorded trace must replay under the interpreter executor, and a native-recorded trace must replay under the native executor. That keeps replay equivalence honest instead of masking backend-specific behavior.

## 14.9 Provenance DAG (slices 21-inv-C-1, 21-inv-C-2)

Every `Grounded<T>` value carries provenance — a directed acyclic graph of which source documents each claim is rooted in. `ProvenanceEdge` trace events record the DAG. `corvid trace dag <trace>` renders it as Graphviz DOT.

```text
corvid trace dag trace.jsonl | dot -Tsvg > provenance.svg
```

Implementation reference: [crates/corvid-cli/src/trace_dag.rs](../../crates/corvid-cli/src/trace_dag.rs).

## 14.10 CLI reference for Phase 21

| Command | What it does |
|---|---|
| `corvid replay <trace>` | Plain replay of a trace against the current code |
| `corvid replay <trace> --model <id>` | Differential replay against a live model |
| `corvid replay <trace> --mutate <step> <json>` | Counterfactual mutation replay |
| `corvid test --from-traces <dir> --from-traces-source <file>` | Prod-as-test-suite regression run |
| `corvid test --from-traces ... --promote` | Interactive golden-trace promotion |
| `corvid test --from-traces ... --flake-detect <N>` | Nondeterminism probe |
| `corvid trace list` | List traces under `target/trace/` |
| `corvid trace show <id>` | Print a trace as formatted JSON |
| `corvid trace dag <id>` | Render the provenance DAG as DOT |
| `corvid trace-diff <base-sha> <head-sha> <path>` | PR behavior receipt |

## 14.11 Determinism axes the runtime records

Every non-deterministic source the runtime understands gets a recording hook. The catalog is the ceiling on what `@replayable` can guarantee:

- **LLM calls.** `LlmCall` / `LlmResult` events. Differential replay swaps only this axis.
- **Tool calls.** `ToolCall` / `ToolResult` events.
- **Approval decisions.** `ApprovalRequest` / `ApprovalResponse` / `ApprovalDecision` events. Replay reads the recorded decision; it does not re-invoke the approver.
- **PRNG / random.** `SeedRead` events capture the raw seed bytes; replay substitutes them so any downstream `random()` call reproduces exactly.
- **Clock / time.** `ClockRead` events capture every wall-clock read; replay substitutes the recorded value.
- **Environment variables.** Read once at startup, snapshotted into `SchemaHeader`.

Anything outside this catalog is rejected by the `@replayable` checker. New axes (network timeouts, file-system reads beyond what `corvid.toml` declares) land as new event variants with a monotonic `SCHEMA_VERSION` bump and a compile-time opt-in. Implementation reference: [docs/phase-21-determinism-sources.md](../phase-21-determinism-sources.md).
