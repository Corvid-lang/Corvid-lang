# Corvid — Build Roadmap

> Phase-by-phase plan from v0.1 (complete) to v1.0 (public launch).
> For feature definitions see [`FEATURES.md`](./FEATURES.md).
> For architecture see [`ARCHITECTURE.md`](./ARCHITECTURE.md).

Every phase has:
- A pre-phase chat (concepts, decisions, success criteria) before any code.
- Tests green at the phase boundary.
- A dev-log entry describing decisions made.

---

## Completed phases

### Phase 1 — AST types ✅
Rust data types for every parsed Corvid construct. ~550 LOC.

### Phase 2 — Lexer ✅
Hand-rolled state machine. 22 keywords, Python-style indent/dedent, triple-quoted strings.

### Phase 3 — Parser ✅
Recursive descent + Pratt. Expressions (3a), statements (3b), declarations (3c).

### Phase 4 — Name resolution ✅
Two-pass; side-table keyed by span; strict duplicate detection.

### Phase 5 — Type + effect checker ✅
**The killer feature.** Dangerous tool calls without prior `approve` fail compilation.

### Phase 6 — IR lowering ✅
Typed IR; references resolved; parse-time sentinels normalized.

### Phase 7 — Python codegen ✅
Walks IR, emits runnable Python. Becomes `--target=python` in v1.0.

### Phase 8 — Python runtime ✅
`corvid_runtime` Python package. Interim home for HTTP/approvals/tracing.

### Phase 9 — CLI wiring ✅
`corvid new`, `check`, `build`, `run`, `doctor`. Real diagnostics.

### Phase 10 — Polish ✅
Ariadne multi-line error rendering. Error codes. README. Offline demo.

**v0.1 complete. 134 Rust + 10 Python tests green.**

### Phase 11 — Interpreter + native runtime ✅
Tree-walking interpreter (`corvid-vm`), async end-to-end. Native runtime (`corvid-runtime`) with `ToolRegistry`, `Approver` trait, `MockAdapter`, `AnthropicAdapter`, `OpenAiAdapter`, JSONL tracing with secret redaction, `.env` loading via `dotenvy`. `corvid run` dispatches natively — no Python on the path. Done-when met: refund_bot demo runs end-to-end with Python uninstalled; `cargo run -p openai_hello` / `anthropic_hello` make real provider calls. Test count grew from 134 (v0.1) to ~219 across the workspace.

Carry-overs explicitly tracked elsewhere:
- Effect-tagged `import python` → Phase 16
- Proc-macro `#[tool]` + `corvid run` user-tool loading → Phase 14
- Google / Ollama adapters → Phase 18
- Streaming `Stream<T>` → Phase 22
- Async-native concurrent multi-agent execution → revisit when Phase 25 demands it

---

## In progress

### Phase 12 — Cranelift scaffolding (~2 months)
Goal: compile trivial IR to native code and prove parity with the interpreter.

### Phase 12 — Cranelift scaffolding (~2 months)
Goal: compile typed IR to native machine code via Cranelift. Interpreter and compiled binary produce the same answer on every fixture — the oracle parity the async-interpreter decision was defending.

Pre-phase decisions locked: **AOT-first** via `cranelift-object` (no JIT detour — the v1.0 pitch is a single binary), **trap-on-overflow** arithmetic via `sadd_overflow` + explicit branch to a runtime overflow handler (safety wins; Rust-debug-mode cost accepted).

#### Slice 12a ✅ — AOT scaffolding + Int arithmetic (Day 19)
- [x] `corvid-codegen-cl` workspace crate with Cranelift 0.116 deps
- [x] Host ISA via `target-lexicon` + native flag builder
- [x] Lowering: Int literals, parameter loads, Int arithmetic with overflow trap, return, agent-to-agent calls
- [x] Overflow via `sadd_overflow`/`ssub_overflow`/`smul_overflow` + branch to runtime handler
- [x] C entry shim + `corvid_runtime_overflow` handler
- [x] `cc` crate drives MSVC `cl.exe` on Windows (per-test `/Fo<tempdir>\` to prevent `.obj` collisions); GCC/Clang on Unix
- [x] `corvid_entry` trampoline — shim stays static, user agents get `corvid_agent_` symbol prefix to avoid C-runtime collisions
- [x] `corvid-driver::build_native_to_disk` emits `target/bin/<stem>[.exe]`
- [x] `corvid build --target=native <file>` wired
- [x] Differential parity harness with 15 fixtures (all literal + arithmetic cases, agent-to-agent calls, + 3 overflow/div-by-zero parity cases)
- [x] `CodegenError::NotSupported { reason, span }` for everything outside Int-only, each message pointing at the slice that unblocks it

#### Slice 12b ✅ — Bool, comparisons, if/else (Day 20)
- [x] `cl_type_for` gate maps `Int → I64`, `Bool → I8`, others raise `NotSupported`
- [x] Agent signatures retyped through `cl_type_for` (parameters + returns)
- [x] Bool literals, six comparison ops (`==`, `!=`, `<`, `<=`, `>`, `>=`) via `icmp`
- [x] Unary `not` as `icmp_eq(v, 0)`; unary `-` via `ssub_overflow(0, x)` trapping on `-i64::MIN`
- [x] Short-circuit `and` / `or` (both tiers — interpreter updated to match). Observable proof fixture: `true or (1 / 0 == 0)` returns `true`.
- [x] `if` / `else` statement lowering with merge-block pattern
- [x] Trampoline extends Bool → I64 via `uextend` when entry agent returns Bool
- [x] 18 new parity fixtures (bringing the suite to 33)

#### Slice 12c — Let bindings, for loops, recursive agent calls with branching
(drafted in its own pre-phase brief)

#### Slice 12d — Float, String, Struct, List memory representation
(drafted in its own pre-phase brief; boundary with Phase 13 is fuzzy, may absorb)

#### Slice 12e — Make native the default for tool-free programs
(drafted in its own pre-phase brief; `corvid run` begins AOT-compiling + executing instead of interpreting where possible)

#### Slice 12f — Polish + benchmarks
(drafted in its own pre-phase brief)

**Out of Phase 12 (deliberately):**
- Cross-compilation to non-host targets (Phase 32)
- Tool / prompt / approve calls in compiled code (Phase 14, needs proc-macro registry)
- WASM target (Phase 27)
- `@wrapping` annotation for opt-out overflow checks (Phase 22 alongside `@budget($)`)

---

## Upcoming

### Phase 13 — Strings, structs, lists in native code (~1 month)
Memory representation for composite values in compiled output.

### Phase 14 — Native tool dispatch (~2–3 weeks)
Tools registered via Rust proc macro compile directly into generated code.

### Phase 15 — Native async runtime (~1–2 months)
Tokio integration. Compiled agents use Tokio's executor for LLM and tool I/O.

### Phase 16 — Python FFI via PyO3, effect-tagged (~1 month)
`import python "..."` works in both interpreter and native backends. Lazy CPython load. **Imports declare effects at the import site** (`import python "requests" as requests effects: network`); untagged imports are rejected by the checker. `effects: unsafe` is the opt-in escape hatch and is flagged for review. This is the TypeScript `.d.ts` analog: the compiler trusts declared effects, and untagged Python usage cannot be introduced by accident.

### Phase 17 — Testing primitives (~3 weeks)
`test`, `mock`, `fixture` as language features.

### Phase 18 — Multi-provider LLM adapters (~2 weeks)
OpenAI, Google, Ollama adapters in `corvid-runtime`.

### Phase 19 — Memory primitives (~1 month)
`session`, `memory` as typed, SQLite-backed stores.

### Phase 20 — Error handling + retry (~1 month)
`Result` / `Option` types; retry policies as syntax.

### Phase 21 — HITL expansion (~2 weeks)
`ask(...)`, `choose(...)`, richer approval UI.

### Phase 22 — Effect rigor + grounding + uncertainty + cost + streaming (~3 months)

The moat phase. All compile-time, all language-level.

- **Custom effects + effect rows.** User-declared `effect Name` beyond `safe`/`dangerous` (`retrieves`, `spends`, `reads_pii`, `mutates_db`, `cites`, ...). Tool and agent signatures carry effect rows; body verified against declaration. Data-flow tracking propagates effects across calls; per-effect approval policies declarable. Revisits the Day-4 `Safe | Dangerous` decision now that the frontend is stable — additive, no breaking change.
- **Grounding + citation contracts** (`std.rag` language half).
  - `grounds_on ctx` annotation on prompts; template must reference `ctx` or `E0201`.
  - `cites ctx` effect; return type must be `Grounded<T>` or `E0202`; template must request citations or `E0203`.
  - `cites ctx strictly` for runtime verification failure.
  - `Grounded<T>` compiler-known stdlib type; unwrap via `.unwrap_discarding_sources()`.
  - Retriever tools carry the `retrieves` effect; agents propagate it.
  - The runtime substrate (sqlite-vec, document loaders, chunking, embedder) ships as the separate `corvid-rag` package and is out of scope here.
- **`eval ... assert ...` language syntax.** First-class evaluation declarations. Parsed, typechecked, lowered. The CLI runner and reports land in Phase 31.
- **Uncertainty, budgets, streaming.** `T?confidence`, `@budget($)`, `Stream<T>`.
- **Property-based bypass tests.** Prove the effect checker cannot be circumvented via FFI, generics, or indirect calls.
- **Written effect-system specification.** 20–40 pages: syntax, typing rules, worked examples, FFI/async/generics interactions. Related work: Koka, Eff, Frank, Haskell effect libs, Rust `unsafe`, capability systems. Ships at the phase boundary alongside the code.

### Phase 23 — Prompt-aware compilation (~1 month)
Schema caching, TOON compression, template deduplication.

### Phase 24 — Replay (~3 weeks)
`corvid replay <trace>` primitive; every run replayable by construction.

### Phase 25 — Multi-agent + durable execution (~2 months)
Crash-safe agents; recursion/composition with automatic trace merging.

### Phase 26 — Hot reload (~2 weeks)
In-flight runs keep version; new runs use new code.

### Phase 27 — WASM target (~2 months)
`corvid-codegen-wasm` reads the same IR; browsers + edge.

### Phase 28 — Package manager (~1–2 months)
`corvid add <pkg>`, lockfile, registry.

### Phase 29 — LSP + IDE (~1–2 months)
`corvid-lsp`, VS Code extension, hover/completion/go-to-def.

### Phase 30 — Standard library (~2 months)
Common agent patterns as stdlib.

### Phase 31 — Eval tooling (~1 month)
`corvid eval` CLI, terminal + HTML reports, regression detection, CI exit-code contract. The `eval ... assert ...` syntax already landed in Phase 22; this phase is runner, report, and wiring — not language.

### Phase 32 — Polish for launch (~2 months)
Stability guarantees, docs, installer, site, GIF/video, HN launch.

---

## Total estimated effort

**~18–24 months of focused solo work** from today to v1.0 public launch.

The dates aren't the point. The point is that each phase has:
- A clear goal.
- A pre-phase brief before code.
- Tests at the boundary.
- A dev-log entry.

That discipline is what makes the 24 months possible.

---

## Non-goals

Explicitly not on the roadmap:

- Replacing Python as a general-purpose language.
- Rust-speed competition for non-AI workloads.
- Supporting every LLM provider at launch — Anthropic first, others follow.
- Windows + Linux + macOS day-one. Start on one OS (macOS), add others in Phase 32.

---

## Velocity markers

To keep momentum honest, ship these at the phase boundaries:

- End of Phase 11: a `corvid run` that doesn't need Python.
- End of Phase 12: a compiled binary for `hello.cor`.
- End of Phase 16: full interpreter + compiler parity on `refund_bot.cor`.
- End of Phase 20: production-grade error handling.
- End of Phase 25: multi-agent demo.
- End of Phase 32: v1.0 public release.
