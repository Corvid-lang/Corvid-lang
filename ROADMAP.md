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

---

## In progress

### Phase 11 — Interpreter foundation (4 weeks)
Goal: a native Rust interpreter that executes the IR directly. The `corvid-vm` crate.

- `Value` enum (Int/Float/String/Bool/Nothing/Struct/List/Function/Closure)
- `Environment` mapping `LocalId` → `Value`
- `Interpreter` walking `IrBlock`/`IrStmt`/`IrExpr`
- Native HTTP client via `reqwest`
- Native Anthropic adapter (direct REST)
- Native tool registry + approval + tracing
- `corvid run` dispatches to interpreter, not Python

**Done-when:** `corvid run examples/refund_bot_demo/src/refund_bot.cor` works with Python uninstalled.

### Phase 12 — Cranelift scaffolding (~2 months)
Goal: compile trivial IR to native code and prove parity with the interpreter.

- Add `cranelift`, `cranelift-module`, `cranelift-jit` crates.
- `corvid-codegen-cl` crate: IR → Cranelift IR for arithmetic, control flow, calls.
- Differential tests: interpreter output == compiled output for every fixture.
- `corvid build` begins emitting `target/bin/<name>` (native).

---

## Upcoming

### Phase 13 — Strings, structs, lists in native code (~1 month)
Memory representation for composite values in compiled output.

### Phase 14 — Native tool dispatch (~2–3 weeks)
Tools registered via Rust proc macro compile directly into generated code.

### Phase 15 — Native async runtime (~1–2 months)
Tokio integration. Compiled agents use Tokio's executor for LLM and tool I/O.

### Phase 16 — Python FFI via PyO3 (~1 month)
`import python "..."` works in both interpreter and native backends. Lazy CPython load.

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

### Phase 22 — Uncertainty + cost + streaming (~2 months)
`T?confidence`, `@budget($)`, `Stream<T>`.

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

### Phase 31 — Eval framework as language feature (~1 month)
`eval ... against dataset(...) { ... }` syntax.

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
