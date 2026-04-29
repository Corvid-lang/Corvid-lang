# Corvid — Build Roadmap

> Phase-by-phase plan from v0.1 (complete) to v1.0 (public launch).
> For feature definitions see [`FEATURES.md`](./FEATURES.md).
> For architecture see [`ARCHITECTURE.md`](./ARCHITECTURE.md).

**Positioning.** Corvid is a **general-purpose AI-native language**, not an agent-only DSL and not a RAG framework. Ambition: be the default choice for building AI applications end to end: agents, copilots, workflow automation, model-routed services, human-in-the-loop systems, eval pipelines, memory-backed applications, RAG, and ordinary software around them. "Best at everything" is a trap that has killed every language that tried it (PL/I, Ada, early Scala); the honest version is **narrow excellence on the moat, broad competence on table stakes, disqualified on nothing.**

### Moat — dimensions Corvid is built to genuinely win on

1. **Safety for AI-shaped software.** Effect checker, approve-before-dangerous, compile-time cost bounds, contract verification. Nobody else is competing here.
2. **AI-native ergonomics.** `agent` / `tool` / `prompt` / `approve` / `model` / `eval` as language concepts; replay, grounding contracts, cost budgets, approval boundaries, model routing, trace assertions, and provenance as first-class constructs. Structurally impossible to match without owning the whole pipeline.
3. **Readability for human + LLM.** Pythonic surface, shallow hierarchies, no pointer aliasing, explicit effects. The language machines both read and *write* best.

### Table stakes — top-tier, competitive with best in category (not best overall)

- **Performance.** Go / Swift class. Fast startup (Phase 12 native AOT), throughput where compute rarely bottlenecks real applications.
- **Memory.** Refcount + cycle collector + effect-typed memory model (Phase 17). Region inference + Perceus linearity means most allocations never pay refcount; cycles caught without per-object tracing overhead. Predictable release without Java pauses.
- **Deployment.** Single native binary + WASM (Phase 23) + C ABI embedding (Phase 22).
- **Tooling.** LSP (Phase 24), formatter, package manager (Phase 25), REPL (Phase 19). Polished, not novel.
- **Cross-platform.** macOS + Linux + Windows all first-class by v1.0 (Phase 33).

### Deliberately not competing

- Systems-level control — Rust / Zig win. No pointer arithmetic, no manual allocators.
- Raw hot-loop numerics — C++ / Fortran win. FFI for the ~1% of apps that need it.
- Dynamic metaprogramming — Ruby / Lisp win. Opposite trade-off to compile-time checking.
- Ecosystem size at launch — Python / JS have 20-year head starts. Python FFI (Phase 30) closes the gap.

**The test applied to every proposed feature:** does it strengthen a moat dimension, or bring us to parity on a table-stakes dimension where we're below the floor? If yes, build it. If it moves neither bar, defer.

### Phase standard

Every remaining phase must make Corvid more AI-native and more general-purpose at the same time. Generic infrastructure is allowed only when it carries Corvid's effect, provenance, approval, cost, replay, eval, model, human-boundary, distribution, or deployment semantics through that layer.

AI-native does **not** mean "RAG with syntax." RAG is one standard-library pattern. The language primitives must be broad enough for the full AI application surface: autonomous and supervised agents, copilots, workflow orchestration, extraction/classification, tool-use, approval-gated actions, model routing, memory, replay, evals, governance, and normal application code.

1. **More AI-native.** Each phase must ship at least one semantic capability that makes AI behavior more visible, constrained, replayable, typed, auditable, or governable.
2. **More general-purpose.** Each phase must also make Corvid stronger as a normal programming language: modules, packages, tooling, deployment, tests, memory, FFI, standard library, editor support, portability, or maintainability.
3. **More powerful without shortcuts.** Corvid does not ship feature-shaped placeholders. A feature is not done until it has real semantics, user-visible behavior, positive and negative tests, honest docs, clear non-scope, and validation through the command path users will actually run.

Every phase has:
- A pre-phase chat (concepts, decisions, success criteria) before any code.
- Tests green at the phase boundary.
- A dev-log entry describing decisions made.

### Autonomous execution protocol

Default working mode after Phase 22: proceed through the remaining roadmap automatically, one coherent slice at a time, without asking for routine permission between phases. Product, security, scope, and marketing decisions are delegated to the implementation lane by default: choose the design that is more durable, more auditable, more AI-native, and more general-purpose, even when it is harder.

The next phase starts only after the current slice has real implementation, tests, docs or roadmap updates where required, validation through user-facing commands, and a commit. Do not optimize for the easiest path. A shortcut is any change that preserves the appearance of progress while weakening semantics, skipping validation, hiding a limitation, or moving a hard requirement into vague follow-up language.

Planning assumption: the remaining roadmap is repo-local. It should not require credentials, payments, external account setup, destructive remote-history changes, or public claims without committed evidence. If a slice appears to require one of those, redesign the slice to keep the strongest local, testable version first; document any truly external launch step as operational follow-up, not implementation scope.

Pause and ask only for:
- Secrets, credentials, payments, domain/account ownership, or external service actions that cannot be completed safely from the local repository alone.
- Destructive actions against user work, published artifacts, tags, releases, or remote history.
- Any conflict with user work, dirty files owned by someone else, or failing validation that cannot be resolved locally.
- Evidence gaps where a public claim cannot be backed by a committed test, command, benchmark archive, or spec.

Do not pause for:
- Routine implementation sequencing.
- Normal refactors needed to keep the code correct.
- Adding positive and negative tests.
- Documentation needed to make a shipped feature honest.
- Product, security, scope, or marketing tradeoffs when the stronger non-shortcut direction is clear from the roadmap.
- Continuing from one completed roadmap slice to the next.

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
- Proc-macro `#[tool]` + `corvid run` user-tool loading → Phase 14
- Streaming `Stream<T>` → Phase 20 (moat phase)
- Google / Ollama adapters → Phase 31
- Effect-tagged `import python` → Phase 30
- Distributed concurrent multi-agent orchestration → Post-v1.0 (Phase 38 now covers durable single-backend jobs and agent runs; cross-service multi-agent graphs remain out of pre-v1.0 scope)

**v0.2 complete. ~219 tests green.**

---

## In progress

### Phase 12 — Cranelift scaffolding (~2 months) ✅ closed
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

#### Slice 12c ✅ — Local bindings + `pass` (Day 21)
- [x] `IrStmt::Let` lowering with declare-or-reuse: each `LocalId` maps to a Cranelift `Variable`; first sight declares with `cl_type_for(ty)`, later sights reuse and `def_var`
- [x] Type-change-on-reassignment defensive guard → `CodegenError::Cranelift` (typechecker should catch it; this closes the failure mode if not)
- [x] `IrStmt::Pass` becomes a no-op (was `NotSupported`)
- [x] Env signature changed from `HashMap<LocalId, Variable>` to `HashMap<LocalId, (Variable, clir::Type)>` so the type-change guard has the existing width to compare against
- [x] 9 new parity fixtures: simple binding, multi-binding arithmetic, repeated use, three-step reassignment, Bool binding, reassignment inside `if`, binding used in comparison, `pass` as noop, parameterised agent with locals
- [x] Smoke-tested `corvid build --target=native examples/with_locals.cor`: locals + reassignment + `if` + native execution end-to-end

#### Slice 12d ✅ — `Float` (Day 22)
- [x] `cl_type_for(Float) → F64`; `IrLiteral::Float` lowering via `f64const`
- [x] Float arithmetic via `fadd`/`fsub`/`fmul`/`fdiv`; `%` via `a - trunc(a/b) * b` to match Rust `f64::%`
- [x] Float comparisons via `fcmp` with IEEE-correct NaN semantics (`==` returns false on NaN, `!=` returns true on NaN)
- [x] Mixed Int+Float promotion via `fcvt_from_sint` — same widening rule as the interpreter
- [x] Float unary negation via `fneg` (no trap — IEEE)
- [x] **Interpreter updated to follow IEEE for Float div/mod by zero** (was trapping; now returns `Inf` / `NaN`). Closes a divergence rather than creates one.
- [x] Defensive guard: Float entry-agent returns blocked with `NotSupported` pointing at slice 12h (where the C shim grows to handle non-Int print formats)
- [x] 10 new parity fixtures including the IEEE divergence proofs (`1.0 / 0.0 > 1.0` true; `NaN != NaN` true)

#### Slice 12e ✅ — Memory management foundation (Day 23)

Originally scoped as "Memory foundation + String"; user split into 12e (foundation) + 12f (String) for cleaner landings after agreeing the combined slice was too large to ship safely in one session.

- [x] `runtime/alloc.c` — 16-byte header (`atomic refcount + reserved`), `corvid_alloc` / `corvid_retain` / `corvid_release`, atomic leak counters
- [x] `i64::MIN` immortal sentinel for `.rodata` literals — `retain` / `release` short-circuit so static memory is never written to
- [x] `runtime/strings.c` — `corvid_string_concat` / `_eq` / `_cmp` built on the allocator (descriptor + bytes share one allocation block)
- [x] `shim.c` updated — prints `ALLOCS` / `RELEASES` to stderr when `CORVID_DEBUG_ALLOC` is set, kept off by default so existing parity output is unchanged
- [x] `link.rs` compiles and links all three C files via the host C compiler with `/std:c11` (MSVC) / `-std=c11` (GCC/Clang) for `<stdatomic.h>` support
- [x] `cl_type_for(String) → I64` (descriptor pointer); `is_refcounted_type` helper; runtime helper symbol constants (`RETAIN_SYMBOL` / `RELEASE_SYMBOL` / `STRING_CONCAT_SYMBOL` / `STRING_EQ_SYMBOL` / `STRING_CMP_SYMBOL`)
- [x] All 52 existing parity fixtures still green with the new C runtime linked into every binary

**Pre-phase decisions locked**: 16-byte header (preserves payload alignment + reserves a future-use word), atomic refcount (post-v1.0 multi-agent work will need it; cheap insurance now), scope-driven release insertion (correct now, liveness-driven optimisation is Phase 20 — moat phase), combined slice (foundation + String) — then split mid-session into 12e (foundation) + 12f (String) once the String integration revealed itself as a slice's worth of work on its own.

#### Slice 12f ✅ — `String` operations + ownership wiring (Day 24)
- [x] `RuntimeFuncs` struct (FuncIds for retain / release / concat / eq / cmp + `Cell<u64>` literal counter), declared once per module in `lower_file`, threaded through every lowering function
- [x] `LocalsCtx` data structure — `(env, var_idx, scope_stack)` bundle for lowering-function locals
- [x] Lower `IrLiteral::String` via `module.declare_data` + `define_data` — single `.rodata` block per literal `[refcount=i64::MIN | reserved | bytes_ptr → self+32 | length | bytes...]` with `write_data_addr(16, self_gv, 32)` self-relative relocation
- [x] Lower `String + String` (concat) via `corvid_string_concat` call
- [x] Lower String comparison ops (`==`, `!=`, `<`, `<=`, `>`, `>=`) via `corvid_string_eq` / `corvid_string_cmp`; narrow result `i64 → I8`
- [x] Scope-stack tracking for refcounted locals; `Vec<Vec<(LocalId, Variable)>>` pushed/popped at if/else branch entry/exit; function-root scope pushed in `define_agent`
- [x] Ownership management: retain on `use_var` of refcounted (Borrowed → Owned), release after passing to a call (consumed temp), release-on-rebind, retain return value + release locals on return, walk all scopes on return for cleanup
- [x] Parameter binding retains incoming refcounted args (+0 ABI: caller passes without bump; callee retains on entry)
- [x] Driver guard: String entry params / returns → `NotSupported` pointing at slice 12i
- [x] Parity harness leak detector — `CORVID_DEBUG_ALLOC=1` on every binary, parse stderr `ALLOCS=N\nRELEASES=N`, assert equal
- [x] Leak-counter semantic fix: `corvid_release_count` only increments when an allocation is actually freed (refcount hits 0), so it pairs 1:1 with `corvid_alloc_count`
- [x] 7 new String parity fixtures (literal eq, literal neq, concat+eq, empty-string concat both directions, !=, six orderings, reassignment+concat+compare). Leak detector runs on all 59 fixtures (52 existing + 7 new), all balanced.

#### Slice 12g ✅ — `Struct` (memory layout + field access) (Day 25)
- [x] New `IrCallKind::StructConstructor { def_id }` variant in `corvid-ir`; `lower.rs` detects `DeclKind::Type` callees and emits the new variant
- [x] Typechecker: `check_struct_constructor` validates arity and field types; replaces the v0.1-era `TypeAsValue` rejection
- [x] `cl_type_for(Struct) → I64`; `is_refcounted_type(Struct) → true`
- [x] `corvid_alloc_with_destructor(size, fn_ptr)` runtime helper; `corvid_release` calls the destructor (if any) before freeing
- [x] Per-struct-type destructor function generated by codegen in `lower_file` for structs with refcounted fields — walks the refcounted fields at fixed 8-byte offsets and calls `corvid_release` on each
- [x] Struct constructor lowering: alloc (with or without destructor), per-field stores at `i * 8` offsets; field arg's Owned +1 transfers into the struct
- [x] Field access lowering: load at compile-time-known offset; retain if refcounted field; release temp struct pointer
- [x] `RuntimeFuncs.ir_types` now carries cloned struct metadata so lowering can resolve field offsets / constructor arities without threading `&IrFile` through every call site
- [x] 7 new parity fixtures: scalar-only struct, Bool field, String field (exercises destructor), String field extract + compare, struct-as-agent-parameter, reassignment, nested struct field access (two deep)

#### Slice 12h ✅ — `List` + `for` + `break` / `continue` (Day 26)

- [x] `runtime/lists.c` with shared `corvid_destroy_list_refcounted(payload)` — walks length at offset 0, releases each element; one helper handles List<String>, List<Struct>, List<List>
- [x] `link.rs` compiles + links `lists.c` alongside the other runtime files
- [x] `cl_type_for(List) → I64`; `is_refcounted_type(List) → true`; `LIST_DESTROY_SYMBOL` constant + FuncId on `RuntimeFuncs`
- [x] `LoopCtx { step_block, exit_block, scope_depth_at_entry }` + `loop_stack: Vec<LoopCtx>` threaded through `lower_block` / `lower_stmt` / `lower_if`
- [x] `IrExprKind::List` lowered to alloc + length store + per-element stores; refcounted-element lists use `corvid_alloc_with_destructor` + `corvid_destroy_list_refcounted`
- [x] `IrExprKind::Index` lowered with runtime bounds check: traps on `idx < 0` or `idx >= length`; refcounted elements retained after load
- [x] `IrStmt::For` lowered as four-block pattern: `entry → header → body → step → exit`; loop var declared once, initialised to 0 (null), rebinds per iteration with release-on-rebind
- [x] `IrStmt::Break` / `IrStmt::Continue` release refcounted locals across all scopes deeper than the loop's entry depth, then jump to `exit_block` or `step_block` respectively
- [x] Typechecker expansion: `Expr::List` infers `List<T>` from the first element (with homogeneity check + Int→Float promotion); `Expr::Index` returns the List's element type and enforces Int index; `Stmt::For`'s loop variable gets the list's element type (was `Unknown`)
- [x] Pre-existing codegen-py and corvid-ir tests that used `if x:` on a String loop var updated to `if x == "a":` — the lenient v0.1 typechecker had let them through; the stricter slice-12h inference correctly rejects them
- [x] 8 new parity fixtures: list sum via for, break exits early, continue skips, subscript access, List<String> destructor, List of heap strings (real releases), nested List<List<Int>> two-deep subscript, empty-like list

#### Slice 12i ✅ — Parameterised entry agents + Float-/String-returning entries (Day 27)

- [x] `runtime/entry.c`: per-type argv decoders (`corvid_parse_i64` / `_f64` / `_bool` with slice-specific parse errors — not reusing the overflow handler), per-type result printers (`corvid_print_i64` / `_bool` prints `true`/`false` / `_f64` via `%.17g` / `_string` raw bytes), `corvid_arity_mismatch`, `corvid_init` (registers `atexit(corvid_on_exit)` so leak counters still print)
- [x] `runtime/strings.c`: `corvid_string_from_cstr` — wraps a null-terminated argv pointer into a refcount-1 Corvid String descriptor
- [x] `runtime/shim.c` trimmed: `main` removed (now codegen-emitted per program); keeps only `corvid_runtime_overflow`
- [x] `link.rs` wires `entry.c` into both MSVC and GCC/Clang command paths
- [x] `RuntimeFuncs` gains 10 new `FuncId`s (`entry_init`, `arity_mismatch`, `parse_i64`/`_f64`/`_bool`, `string_from_cstr`, `print_i64`/`_bool`/`_f64`/`_string`)
- [x] `emit_entry_trampoline` replaced by `emit_entry_main(module, entry_agent, entry_func_id, runtime)` — signature-aware Cranelift function: `main(i32 argc, i64 argv) -> i32` that calls `corvid_init`, checks arity, loads/decodes each `argv[(i+1)*8]` via the type-appropriate helper, calls the entry agent, prints the return via the type-appropriate printer, releases refcounted args/returns, returns 0
- [x] Driver guards updated: Int/Bool/Float/String allowed at both param and return position; Struct/List still rejected with `NotSupported` pointing at the future serialization slice
- [x] 11 new parity fixtures (total 85): int/two-int/bool/float/string param echoing, float + string returns (with and without params), NaN round-trip, arity-mismatch exits non-zero, parse-error exits non-zero with slice-specific message (verified NOT reusing the overflow message)
- [x] Every fixture runs under `CORVID_DEBUG_ALLOC=1` — `ALLOCS == RELEASES` confirms refcounted argv descriptors and String returns are released exactly once

#### Slice 12j ✅ — Make native the default for tool-free programs (Day 28)

- [x] `native_ability(ir)` pre-flight scan in `corvid-driver` returns structured `NotNativeReason` (`ToolCall` / `PromptCall` / `Approve` / `PythonImport`). Names the native-ability rule explicitly; no codegen-internal errors bubble up.
- [x] Compile cache at `<project>/target/cache/native/<fnv1a64-hex>[.exe]` keyed on source + `corvid-codegen-cl` pkg version + every C runtime shim (`shim.c` / `entry.c` / `alloc.c` / `strings.c` / `lists.c`). Second run of an unchanged file skips codegen + link entirely — measured ~15× speedup on `examples/answer.cor` (1.15s → 0.08s).
- [x] `RunTarget::{Auto, Native, Interpreter}` + `run_with_target(path, target)` entry point. Auto picks native when native-able, falls back to interpreter with a one-line stderr notice ("↻ running via interpreter: <reason>"). Native refuses with a clean error naming the phase that would lift the restriction. Interpreter forces the old path.
- [x] CLI flag: `corvid run <file> [--target=auto|native|interpreter]`, default `auto`. `corvid run` by itself now AOT-compiles + executes when possible.
- [x] 7 new driver tests: native-able program passes scan, tool-using / python-import / prompt-using programs fail scan with the right `NotNativeReason`, cache hits on second call (mtime-verified), auto dispatch populates the cache, `--target=native` on a tool-using program exits non-zero.
- [x] Smoke-tested on `examples/answer.cor` (auto → native, cached on second run) and `examples/hello.cor` (auto → fallback with notice, `--target=native` → clean error).

#### Slice 12k ✅ — Phase 12 close-out benchmarks (Day 29)

- [x] Criterion benchmark harness at `crates/corvid-codegen-cl/benches/phase12_benchmarks.rs`. Three workloads: `arith_loop` (500k Int ops), `string_concat_loop` (50k refcount concats), `struct_access_loop` (100k struct alloc + field read + destructor). Each runs on both tiers — interpreter via `corvid_vm::run_agent`, native via `Command::new(binary).output()`.
- [x] Measured wall-clock published in ARCHITECTURE.md §18 ("Phase 12 performance characteristics"). Headline numbers: **13.6× native for arithmetic, 3.5× for struct access, 2.7× for string concat** (end-to-end including the ~11 ms Windows process-spawn tax). Compute-only ratios are 32× / 7.3× / 6.8×.
- [x] Fair-comparison gate passed: native beats interpreter on all three workloads at the scaled workload sizes. The spawn-cost crossover (interpreter < 5 ms → native loses E2E) is documented explicitly alongside the numbers as a known AOT+process-spawn property, with the Phase 22 cdylib path and post-v1.0 JIT path called out as future fixes.
- [x] Native-tier non-goals documented below under "Out of Phase 12."

Cache-eviction policy, stability guarantees across compiler versions, and cross-compilation all move to Phase 33 (launch polish) — none are load-bearing for development work while there are no external users.

**Out of Phase 12 (deliberately):**
- Tool / prompt / `approve` calls in compiled code — Phases 13–15.
- WASM target — Phase 23.
- C ABI + library mode — Phase 22.
- `@wrapping` annotation for opt-out overflow checks — Phase 20 (moat phase, alongside `@budget($)`).
- Cross-compilation to non-host targets — Phase 33 (launch polish).

**v0.3 cuts here** (Phase 12 close-out). Native AOT is the default tier for tool-free programs, cached between runs, benchmarked against the interpreter.

---

## Upcoming

Ordering principles (applied without exception):

1. **Hard dependencies drive sequence.** If B needs A's output, B comes after A. Every phase below names its dep as either **Hard** (can't ship without it) or **Soft** (release-narrative pairing, technically decoupled). No soft deps dressed as hard to make an order look forced.
2. **Themed releases, not feature-soup versions.** Each version has a narrative — v0.4 is "native tier useful," v0.5 is "GP feel," v0.6 is "moat + replay," v0.7 is "embed + deploy." Users upgrade for a coherent story per cut, not a grab-bag of unrelated features. Mixing moat and table stakes inside one version fragments the upgrade pitch.
3. **Moat lands early relative to the total roadmap.** Phase 20 is the mid-point of pre-v1.0, not the end. Every phase after Phase 20 inherits the moat and strengthens it rather than being moat-less GP-polish work that ships without Corvid-ness.
4. **Version cut-lines are explicit.** Every phase is tagged to the version it ships in. `v1.0` is a calendar commitment, not a feature list.
5. **Speculative scope moved post-v1.0.** Features that are "enterprise maturity" or "optimization on top of v1.0 capability" (distributed multi-agent orchestration, hot reload, prompt-aware compilation optimization) do not sit in the pre-v1.0 critical path. Durable single-backend jobs and resumable agent runs are now part of the production-backend track because real AI applications need them before launch.

---

### Phase 13 ✅ — Native async runtime (Day 30)

**Hard dep:** Phase 12 (native codegen). **Hard deps on this:** Phases 14, 15, 30.

- [x] `corvid-runtime` emits a staticlib (`crate-type = ["lib", "staticlib"]`) that `corvid-codegen-cl` links into every compiled Corvid binary. Produces a self-contained executable — no separate runtime file to ship.
- [x] `ffi_bridge` module in `corvid-runtime` exposes the C-ABI surface: `corvid_runtime_probe`, `corvid_runtime_init`, `corvid_runtime_shutdown`, `corvid_tool_call_sync_int`. `deny(unsafe_code)` at crate level; `ffi_bridge` opts in explicitly with a written rationale. Every `unsafe` block carries a SAFETY comment.
- [x] **Eager-init globals, no lazy semantics.** `corvid_runtime_init()` constructs the tokio Runtime + the `Arc<corvid_runtime::Runtime>` and publishes both via `Box::leak` behind an `AtomicPtr`. Readers panic loudly if init hasn't run — no "lazy first-use" branches anywhere.
- [x] **Multi-thread tokio runtime.** `tokio::runtime::Builder::new_multi_thread().enable_all().build()`. Picked multi-thread (not current-thread) at the pre-phase chat: GP-class positioning demands a production-grade runtime from day one; the ~5-10 ms startup tax only applies to programs that actually use the runtime (pure-computation programs skip init entirely — see `ir_uses_runtime` in codegen-cl). `CORVID_TOKIO_WORKERS` env override respected.
- [x] Codegen-emitted main (`emit_entry_main`) conditionally calls `corvid_runtime_init` + registers `corvid_runtime_shutdown` via `atexit` when `ir_uses_runtime(ir)` returns true. Tool-free programs preserve slice 12k's benchmark numbers — no runtime tax paid for what isn't used.
- [x] `IrCallKind::Tool` lowering in `lowering.rs`: for the narrow `() -> Int` signature, emits a call to `corvid_tool_call_sync_int(name_ptr, name_len)` where `name_ptr` is a `.rodata` byte-array emitted by the new `emit_cstr_bytes` helper. Any other tool signature still raises `NotSupported` pointing at Phase 14. `IrCallKind::Prompt` stays pointing at Phase 15.
- [x] Env-var-based mock-tool hook for the parity harness: `CORVID_TEST_MOCK_INT_TOOLS="name1:v1;name2:v2"` registers zero-arg Int-returning mocks during `corvid_runtime_init`. Test-only convention; users never set this env var.
- [x] `tests/ffi_bridge_smoke.rs` — FFI contract test. Hand-written C program that calls `corvid_runtime_probe` / `_init` / `_tool_call_sync_int` / `_shutdown` end-to-end, linked against the staticlib via the same cc-crate pipeline `link.rs` uses. Idempotent shutdown verified; unknown-tool error sentinel verified.
- [x] 6 new parity fixtures (total 91): tool returns Int directly, tool result in arithmetic, tool result drives conditional (both branches), two distinct tools added, agent-to-helper-agent-to-tool chain. Every fixture runs under `CORVID_DEBUG_ALLOC=1` — `ALLOCS == RELEASES` confirms no bridge-induced leaks.
- [x] Link flow handles the `+44 MB` staticlib + native system libs (bcrypt / advapi32 / kernel32 / ntdll / userenv / ws2_32 / dbghelp / legacy_stdio_definitions on MSVC; -lpthread -ldl -lm + macOS frameworks on Unix). `build.rs` in corvid-codegen-cl emits `CORVID_STATICLIB_DIR` at build time so link.rs finds the artifact without runtime discovery.

**Non-scope (deliberate):** User-declared tools via proc-macro registry — Phase 14. Prompt calls — Phase 15. Python FFI — Phase 30. Generalised tool-call bridge (non-Int returns, multi-arg) — Phase 14 extends `corvid_tool_call_sync_int` into a full JSON-marshalling `corvid_tool_call_sync`. True concurrent agents — Phase 25 post-v1.0. Binary size optimization (compiled binaries are ~30 MB stripped after Phase 13) — Phase 33 launch polish.

**Driver-level user-visible behavior:** unchanged in Phase 13. `corvid run <file>` with a tool-using program still falls back to the interpreter via `native_ability::NotNativeReason::ToolCall` — Phase 14 updates the driver to allow tool-using programs to run natively once the proc-macro registry is wired. Phase 13's codegen supports tools; Phase 13's driver doesn't expose that support to users. Parity harness tests it directly.

### Phase 14 ✅ — Native tool dispatch (Day 31)

**Hard dep:** Phase 13 (native async runtime).

- [x] `corvid-macros` proc-macro crate. `#[tool("name")]` on an `async fn` generates a typed-ABI `extern "C"` wrapper + an `inventory::submit!(ToolMetadata)` registration. The user's async fn remains callable as plain Rust for interpreter-tier use.
- [x] **Typed C ABI — no JSON marshalling.** Committed to the extraordinary answer after auditing JSON as the lazy default: both sides of the tool-call boundary know the schemas at compile time, both sides are ours, no LLM tokens cross this boundary. JSON's compactness / universality don't apply; its costs (heap alloc per call, UTF-8 parsing, type erasure, opacity to the optimizer) do. Typed direct calls are what Rust FFI uses idiomatically — Corvid picks the same.
- [x] `#[repr(C)]` ABI wrappers in `corvid-runtime::abi`: `CorvidString` (transparent over descriptor pointer), identity wrappers for `i64` / `f64` / `bool`. `FromCorvidAbi` / `IntoCorvidAbi` traits the macro calls at conversion sites.
- [x] **Refcount conventions for the tool-call boundary:** caller uses the same Owned (+1) / release-after-call pattern as agent-to-agent calls; wrapper's `FromCorvidAbi for String` is borrow-only (reads bytes, never touches refcount). Net: one retain + one release around the call, matching a borrow-style FFI contract. Leak detector (`CORVID_DEBUG_ALLOC=1`) green on every fixture including String-in String-out round-trip.
- [x] `inventory::collect!(ToolMetadata)` in `corvid-runtime`; `corvid_runtime_init` iterates at startup, records the count for diagnostics.
- [x] Cranelift lowering for `IrCallKind::Tool`: emits a direct `call` to `__corvid_tool_<name>` with typed arguments. Link-time symbol resolution means missing `#[tool]` implementations produce linker errors naming the missing symbol — better than the Phase 13 runtime "tool not found" it replaces. Phase 13's narrow `corvid_tool_call_sync_int` bridge is deleted; the single typed-ABI path covers every signature.
- [x] `IrStmt::Approve` lowers to a no-op in compiled code. Effect checker (Phase 5) already enforces `approve`-before-dangerous-tool-call at COMPILE time; runtime verification of approve tokens is Phase 20's moat-phase territory where custom effect rows make it meaningful. Arg expressions still lower (side effects + refcount).
- [x] `corvid-test-tools` crate: staticlib with mock `#[tool]` implementations covering each scalar type + multi-arg. Parity harness links this into every fixture binary; env-var-based tool bodies let tests vary behavior without rebuilding.
- [x] Driver gate lifted conditionally: `native_ability::NotNativeReason::ToolCall` still fires, but `run_with_target` treats it as "satisfied" when `--with-tools-lib <path>` is provided. Fall back to interpreter (auto) or error with a clear pointer-at-the-fix message (native) otherwise. `NotNativeReason::Approve` removed entirely — approve compiles fine.
- [x] CLI: `corvid run <file> [--target=...] [--with-tools-lib <path>]`. Flag-validation checks the path exists. Cache key incorporates the tools-lib path so `--with-tools-lib A` vs `--with-tools-lib B` produce distinct cached binaries.
- [x] 10 new parity fixtures exercising: Int arg, two Int args, String→Int, String→String roundtrip, approve-before-dangerous tool call. Every fixture leak-detector-audited. Total parity suite: **96 fixtures** (up from 85).
- [x] Live smoke: `corvid run examples/tool_call.cor --with-tools-lib target/release/corvid_test_tools.lib` prints `42`. Without `--with-tools-lib`, auto falls back to interpreter with a clear "pass --with-tools-lib" notice. `--target=native` without the lib errors out.

**Linker architecture note.** `corvid-runtime` now ships as both rlib + staticlib. Rust crates use the rlib; compiled Corvid binaries link exactly ONE "runtime-bearing" staticlib — either the standalone `corvid-runtime.lib` (tool-free programs) or the user's tools staticlib which transitively includes corvid-runtime via rlib dep (tool-using programs). Linking both produces `LNK2005` duplicate-symbol errors on every Rust std symbol because each staticlib bundles its own std. The conditional-link logic in `link.rs` handles the either/or.

**Non-scope (deliberate):** Prompt calls — Phase 15. Runtime approve-token verification — Phase 20 (alongside effect rows + custom effects + cost budgets). Tool signatures with Struct/List args — Phase 15 (composite-type marshalling lands alongside prompts). Auto-build of tools crate via `corvid build` spawning cargo — Phase 33 launch polish. `corvid.toml` `[tools]` section — Phase 25 (package manager).

### Phase 15 ✅ — Native prompt dispatch + multi-provider LLM coverage (Day 32)

**Hard dep:** Phase 13 (native async runtime).

User pushback during pre-phase chat caught two latent shortcuts in the original brief: provider coverage limited to Anthropic + OpenAI (insufficient for AI-native positioning, especially missing local-model support) and naive text-then-parse with no retry (brittle by design). Both got rewritten before any code shipped.

- [x] **5 LLM provider adapters cover every category for v0.4.**
  - **Anthropic** — existing.
  - **OpenAI** — existing (refactored to extract `extract_usage` for reuse).
  - **`OpenAiCompatibleAdapter`** (new) — universal escape hatch via `openai-compat:<base-url>:<model>` model spec. Covers OpenRouter, Together, Anyscale, Groq, Fireworks, Azure OpenAI, llama.cpp server, vLLM, LM Studio, and ~20 other providers exposing OpenAI-compatible endpoints. **One adapter, ~30+ backends.**
  - **`OllamaAdapter`** (new) — local-first via `POST localhost:11434/api/chat`. Routed by `ollama:<model>` prefix. No API key. `OLLAMA_BASE_URL` override for non-default servers.
  - **`GeminiAdapter`** (new) — Google Gemini via `POST /v1beta/models/<m>:generateContent`. Routed by `gemini-*` prefix. Auth via `GOOGLE_API_KEY` / `GEMINI_API_KEY`.
- [x] **`TokenUsage` on every `LlmResponse`.** Every adapter populates `prompt_tokens` / `completion_tokens` / `total_tokens` from the provider response — Anthropic's `input_tokens`/`output_tokens`, OpenAI's `prompt_tokens`/`completion_tokens`, Ollama's `prompt_eval_count`/`eval_count`, Gemini's `usageMetadata`. Foundation for Phase 20's `@budget($)` cost annotations.
- [x] **`EnvVarMockAdapter`** — env-var-based mock for parity tests. `CORVID_TEST_MOCK_LLM=1` registers it as the first adapter so its wildcard `handles()` claims every model spec, avoiding real API egress in CI even when keys leak.
- [x] **4 typed prompt-dispatch bridges** in `corvid-runtime::ffi_bridge`: `corvid_prompt_call_int` / `_bool` / `_float` / `_string`. Each takes 4 `CorvidString` args (prompt name, signature, rendered template, model). Mirrors Phase 14's typed-ABI design.
- [x] **Built-in retry-with-validation.** `CORVID_PROMPT_MAX_RETRIES` (default 3). Each retry escalates the system prompt with stronger format instructions + the prior unparseable response. Tolerant `parse_int` / `parse_bool` / `parse_float` strip surrounding quotes, code fences, and whitespace before parsing.
- [x] **Function-signature context in the system prompt.** Every prompt call automatically tells the LLM "you are a function with signature `name(params) -> ReturnType` — return the appropriate value." Codegen embeds the signature as a literal at compile time. Treats prompts as typed functions the LLM is implementing, not ad-hoc string queries.
- [x] **Stringification helpers** (`corvid_string_from_int` / `_bool` / `_float`) in the C runtime. Cranelift codegen calls them when interpolating non-String args into prompt templates.
- [x] **Cranelift lowering for `IrCallKind::Prompt`.** Compile-time template parser splits `{var}` placeholders; codegen emits a chain of `corvid_string_concat` operations with stringified args between literal segments. Bridge selection by return type.
- [x] **Driver gate lifted unconditionally.** `NotNativeReason::PromptCall` removed. Prompt-using programs compile + run natively without any extra user-provided lib (`corvid-runtime` ships the adapters built-in). Runtime errors surface at LLM call time if no provider is configured.
- [x] **Architectural fix: C runtime moved into `corvid-runtime`.** The `runtime/*.c` files (alloc, strings, lists, entry, shim) relocated from `corvid-codegen-cl/runtime/` to `corvid-runtime/runtime/`. New `corvid-runtime/build.rs` compiles them via `cc::Build` into a `corvid_c_runtime` staticlib. `corvid-runtime` re-exports the path via `c_runtime::C_RUNTIME_LIB_PATH`. `corvid-codegen-cl::link.rs` and the FFI smoke test add this lib to their linker invocations. **Why:** the prompt bridges' `IntoCorvidAbi for String` reaches `corvid_string_from_bytes`, which any binary linking corvid-runtime must resolve — making corvid-runtime self-contained means Rust test binaries link cleanly without separate C-source compilation per test.
- [x] **4 new parity fixtures** (total: **99**): zero-arg Int return, Int-arg interpolation + Int return, String-arg interpolation + Int return. Every fixture uses the env-var mock LLM. Leak-detector-audited.

**Non-scope (deliberate, named for future phases):**
- **Provider-specific JSON-schema structured output** (OpenAI `response_format`, Anthropic tool-use for structured returns, Gemini's `responseSchema`) → Phase 20 (moat, alongside `Grounded<T>`). Phase 15's text-then-parse with retry covers ~95% of cases.
- **Streaming `Stream<T>` returns** → Phase 20.
- **Replay** (deterministic re-execution of recorded LLM calls) → Phase 21.
- **`@budget($)` cost bounds** → Phase 20 (uses the `TokenUsage` Phase 15 plumbed through).
- **Per-prompt model selection in source** (`prompt foo() -> Int using "gpt-4o":`) → Phase 31.
- **Caching response by `(prompt, args, model)`** → Phase 21.
- **Real-API integration tests** against Ollama / OpenAI / Anthropic / Gemini → Phase 33 launch polish, when CI has a runner that can install Ollama + has provider keys configured.
- **`corvid stats` CLI subcommand** for token-usage diagnostics → Phase 20 ships this alongside `@budget($)` enforcement that uses the same data.

**v0.4 cuts here.** Native tier is actually useful for real programs.

---

### Phase 16 ✅ — Methods on types (Day 33)

**Hard dep:** frontend (✅), IR (✅). Single Cranelift-symbol disambiguation needed (DefId-suffixed); otherwise codegen unchanged.

Pre-phase chat caught two limiting shortcuts in my brief and reshaped the phase substantially. The shipped form:

- [x] **Syntax: `extend T:` block** (not Rust's `impl T:`). Full word matches Corvid's keyword style (`agent`, `tool`, `prompt`, `approve`, `dangerous`, `type`); reads as English ("extend Order with these methods"); leaves room for Phase 20 traits via `extend T as Serializable:` without retroactive renaming. `type T:` stays purely structural — better for LLM readability.
- [x] **Methods can be ANY decl kind** — `extend T:` blocks hold a mix of `agent`, `prompt`, `tool` declarations. Same dot-syntax dispatches all of them: `order.total()` is a pure-function call, `order.summarize()` is an LLM call, `order.fetch_status()` is a tool call, `order.process()` is an effectful agent. **No other language unifies prompts, tools, and pure code under a single typed dot-syntax** — for an AI-native language this turns "AI is a method on your type" from positioning into syntax.
- [x] **Effect inference handles purity** — no `function` keyword introduced. Agents inherit their effect rows from their bodies (which already worked via the existing checker). A method that doesn't call any effectful primitive has no effect row; replay/cost-budget machinery (Phases 20–21) won't track it. Avoids fourth-keyword proliferation; keeps the moat phase's effect-row work simple.
- [x] **`public` / private visibility shipped now**, with parens-extension reserved for Phase 20 effect-scoped variants. Default visibility is private (file-scoped). `public` and `public(package)` are the Phase 16 surface; `public(effect: audited)` lands in Phase 20 without breaking syntax. Decision motivated by: public-by-default is a one-way door for API stability; retrofitting visibility post-v1.0 would be a breaking change every existing impl block has to absorb.
- [x] **Receiver as explicit first parameter** — no `self` keyword. `extend Order: agent total(o: Order) -> Int` makes the receiver a parameter like any other. Mental model matches "methods are agents with a receiver"; Pythonic users adapt instantly. Less special-casing than Rust's `self`.
- [x] **Receiver-type-keyed method lookup.** Resolver builds a per-type method side-table `(type_def_id, method_name) -> DefId`. Multiple types can share method names (`Order.total`, `Line.total`) without collision. Field/method name collisions on the same type are compile-time errors.
- [x] **Cranelift symbol mangling** updated to include the agent's `DefId` so `extend Order: agent total` and `extend Line: agent total` get distinct internal symbols. Symbols are `Linkage::Local`; the suffix never leaks into a public API.
- [x] **6 new parity fixtures** (total: 105) — receiver-only method, multi-arg method, method-calls-method, methods-with-same-name-on-different-types (verifies receiver-type dispatch), method on a struct with a refcounted `String` field (leak-detector-audited).
- [x] **5 new resolver tests** (total: 19) — extend registers methods, extend on unknown type errors, duplicate methods error, method/field name collision error, methods on different types coexist.
- [x] **5 new parser tests** (total: 80 → 85) — `extend` blocks parse, mixed decl kinds, default + `public` + `public(package)` visibility, malformed `public(...)` rejected.

**Non-scope (deliberate, named for future phases):**
- **`self` keyword** — explicit first param model is the answer; revisit only if a real foot-gun surfaces.
- **Static methods** (`Type.factory()`) — free agents serve the role today; non-breaking to add later.
- **Methods on built-in types** (Int, String, List) — orphan-rule design must come with Phase 25's package manager. Phase 30+ stdlib decides.
- **Method overloading** — duplicate names on a type are compile errors. Rust + Go thrive without overloading; not adding it.
- **Multi-file `extend` blocks** (one type extended in many files) — Phase 25.
- **Trait/interface system** — Phase 20 (moat). The `extend T as TraitName:` syntactic slot is reserved.
- **Effect-scoped visibility** (`public(effect: audited)`) — Phase 20.

**Architecturally important:** Phase 16 introduces NO new IR variants. Method calls compile to ordinary `IrCallKind::Agent` / `Prompt` / `Tool` calls with the receiver prepended as the first argument. Codegen (Cranelift, Python transpile, future WASM) needs no per-method handling — methods are agents/prompts/tools with a different declaration syntax and a different lookup path.

### Phase 17 — Cycle collector + effect-typed memory model (~10–14 weeks) ✅ closed

**Goal.** Backstop refcount against cycles AND lift the memory model to take advantage of Corvid's typed effects. Most allocations should never see refcount at all; the ones that do should rarely be atomic; cycles should be caught without per-allocation tracing overhead.

**Hard dep:** Phase 12 (refcount runtime + native codegen).

**Status.** Closed in `v0.1-memory-foundation`.

| Slice | Outcome | Commit / tag |
|---|---|---|
| `17a` | typed heap headers + per-type typeinfo | `1fea6a0` |
| `17b-0` | retain/release counters + baseline RC counts | `7ef4304` |
| `17b-1a` | `Dup` / `Drop` IR + borrow-signature scaffolding | `82f78b5` |
| `17b-1b.1` | borrow inference + callee-side ABI elision | `2bce2a8` |
| `17b-1b.2` | string operand borrow-at-use-site peephole | `71c7fe4` |
| `17b-1b.3` | field/index target borrow-at-use-site peephole | `de3acb5` |
| `17b-1b.4` | `for` iterator borrow-at-use-site peephole | `a725449` |
| `17b-1b.5` | call-arg borrow-at-use-site peephole | `b0a911e` |
| `17b-1b.6a` | ownership dataflow groundwork | `760b07e` |
| `17b-1b.6b` | IR `Dup` / `Drop` insertion | `1d1af44` |
| `17b-1b.6c` | ownership hook into codegen pipeline | `f3762cd` |
| `17b-1b.6d-1` | transition guard stage | `8e2e98e` |
| `17b-1b.6d-2a` | entry / return cleanup stage | `520e30b` |
| `17b-1b.6d-2` | unified ownership pass default-on | `0cc7895` |
| `17b-1c` | whole-program pair elimination | `046806d` |
| `17b-2` | drop specialization | `8c55c3f` |
| `17b-3` | reuse analysis | deferred to Phase 17.5 |
| `17b-4` | Morphic-style specialization | deferred to Phase 17.5 |
| `17b-5` | escape analysis | deferred to Phase 17.5 |
| `17b-6` | effect-row-directed RC | deferred to Phase 20 |
| `17b-7` | latency-aware RC at prompt / LLM boundaries | `6bedbfb` |
| `17c` | safepoints + stack maps | `e55efea` |
| `17d` | native mark-sweep cycle collector | `ca428bf` |
| `17e` | effect-typed scope reduction | `f5a3bce` |
| `17f / 17f++` | deterministic GC triggers + refcount verifier | `a3b841d` |
| `17g` | `Weak<T>` | `ba01e78` |
| `17h.1` | VM-owned heap handles | `318c892` |
| `17h.2` | VM Bacon-Rajan cycle collector | `91d95ac` |
| `17i` | close-out + benchmark lock | `v0.1-memory-foundation` |

**Historical slice plan (kept for design context):**

- [x] **17a — typed heap headers + per-type typeinfo** *(landed 2026-04-14)*. Every refcounted allocation carries a `corvid_typeinfo*` pointer in its 16-byte header. Per-type metadata (destroy_fn, trace_fn, flags, elem_typeinfo) lives in `.rodata`. Refcount dropped `_Atomic` (Phase 25 will do proper multi-threaded RC, not blanket atomics). Bits 61-62 reserved for 17d mark + 17h color. `List<Int>` mis-trace bug eliminated by design (`elem_typeinfo = NULL` sentinel). 6 new runtime tracer tests, all 105 parity tests still green.
- [~] **17b — principled RC optimization (Perceus).** *Region inference dropped from this slice based on Perceus paper analysis + MLton's published rejection of Tofte-Talpin regions; revisit only if post-17b measurements show remaining allocation pressure justifies the complexity.*
  - [x] **17b-0** *(landed 2026-04-15)* — retain/release call-count instrumentation + baseline RC op counts as exact-match assertions on 6 representative workloads.
  - [x] **17b-1a** *(landed 2026-04-15)* — `IrStmt::Dup` / `IrStmt::Drop` as first-class IR variants; `ParamBorrow` enum + `IrAgent.borrow_sig` field; codegen handles the variants end-to-end. Pure scaffolding, behavior-preserving.
  - [x] **17b-1b.1** *(landed 2026-04-15)* — Lean 4-style monotone fixed-point borrow inference over the call graph. Populates `IrAgent.borrow_sig`. Callee-side ABI elision: refcounted params marked `Borrowed` skip entry-retain + scope-exit release. Measured: `passthrough_agent` 13 → 9 ops (31%).
  - [~] **17b-1b.peepholes** *(landed 2026-04-15 as four separate commits: 17b-1b.2, .3, .4, .5)* — **single borrow-at-use-site optimization family** applied to four IR positions: string BinOp operands, FieldAccess / Index targets, for-loop iter, and call-site args (coordinated with callee `borrow_sig`). Shipped as four commits while structurally one optimization; retrospective dev-log entry Day 24 captures the honest framing. Cumulative measured (baseline → current): `string_concat_chain` 12→10 (8%), `struct_build_and_destructure` 14→8 (43%), `list_of_strings_iter` 22→14 (36%), `passthrough_agent` 13→7 (46%), `local_arg_to_borrowed_callee` new at 6 ops.
  - [ ] **17b-1b** *(real, still pending)* — full use-list + CFG-aware last-use + branch-asymmetric `Dup`/`Drop` insertion pass in `ownership::transform_agent`. Deletes the ~40 scattered `emit_retain`/`emit_release` sites in `lowering.rs`. Handles what peepholes cannot: loop-var body analysis, scope-exit Drop redundancy, cross-statement last-use elision, list-literal item-slot Locals. **This is the work that was originally committed as 17b-1b. The peephole series shipped wins but did not replace this.** Needs its own pre-phase chat; multi-session slice when resumed.
  - [x] **17b-1c** - whole-program retain/release pair elimination. Shipped as the first same-block ARC-style cleanup pass after the unified ownership pipeline.
  - [x] **17b-2** - drop specialization. `drop x` on a known typeinfo now inlines the child-release sequence instead of dispatching through `typeinfo->destroy_fn`.
  - [ ] **17b-3** — reuse analysis. Match `drop(x_size_N); alloc(size_M ≤ N)` pairs in a basic block; emit `if (refcount & MASK) == 1 { reuse_in_place } else { drop; alloc }`. Same-size-in-words rule per Perceus / Lean 4.
  - [ ] **17b-4** — Morphic-style per-call-site alias-mode specialization (Lobster-style, gated to mixed-mode callees only).
  - [ ] **17b-5** — Choi-style interprocedural escape analysis → stack / arena promotion for non-escaping allocations.
  - [ ] **17b-6** — **INNOVATION (zero prior art):** effect-row-directed RC. `Pure` effect → static `isUnique` discharge; `<llm>` effect → batching point for RC ops across known-slow suspensions.
  - [x] **17b-7** - **INNOVATION (zero prior art):** latency-aware RC scheduling across prompt/LLM call boundaries.
- [x] **17c** - Cranelift safepoint emission + stack maps. Per-function safepoint records let the native collector find live roots on task stacks.
- [x] **17d** - native mark-sweep cycle collector. Dispatches through `typeinfo->trace_fn` per object with deterministic test hooks and allocation-pressure triggering.
- [x] **17e** - effect-typed scope reduction. Shipped as conservative same-block `Drop` relocation across effect-free spans.
- [x] **17f** - replay-deterministic GC triggers plus the runtime refcount verifier. GC behavior is now measurable and replay-auditable.
- [x] **17g** - `Weak<T>` user-facing type. Weak refs now ship with effect-typed invalidation rules and runtime clearing semantics.
- [x] **17h** - interpreter-side cycle collector. Bacon-Rajan now runs over VM-owned heap handles in the interpreter tier.
- [x] **17i** - tests + close-out. Locked with the same-session ratio archive and release tag `v0.1-memory-foundation`.

**Non-scope:** generational GC. Concurrent collection (mutator-collector concurrency via write barriers — post-v1.0 if multi-threaded Corvid ever becomes a direction).

### Phase 18 — Result + Option + retry policies (~4 weeks) ✅ — core complete

**Goal.** Language-native error handling with a principled native subset first: `Result<T, E>`, `Option<T>`, propagation (`?`), and retry syntax that lowers as deterministic native control flow rather than a library loop.

**Hard dep:** typechecker extension for generic types (landed). The remaining work is native widening, not front-end feasibility.

**Status.** Front-end + interpreter support is landed. Native AOT support is substantially shipped for the compositional one-word subset and selected wide `Option<T>` cases; Phase 18 is no longer "can Corvid do this?" but "how far do we widen native support before moving on?"

**Shipped so far:**
- [x] `Result<T, E>` and `Option<T>` as compiler-known stdlib types in the frontend + interpreter.
- [x] Postfix `?` in the frontend + interpreter.
- [x] Retry syntax in the frontend + interpreter.
- [x] Native nullable `Option<T>` subset for refcounted payloads such as `Option<String>`.
- [x] Native wide scalar `Option<Int|Bool|Float>`.
- [x] Native nested `Option<T>` widening where nullable-pointer encoding would otherwise collapse `Some(None)` into outer `None`; wrapper-backed `Option<Option<...>>` now preserves the distinction.
- [x] Native postfix `?` for the shipped `Option<T>` subsets, including widening into a different native `Option<U>` envelope.
- [x] Native one-word `Result<T, E>` subset with ownership integration.
- [x] Native postfix `?` for `Result<T, E>`, including `Result<A, E>?` inside `Result<B, E>` when both shapes remain in the current native subset.
- [x] Native deterministic retry lowering over the native `Result<T, E>` subset with explicit backoff control flow and ownership-correct cleanup between attempts.
- [x] Native deterministic retry lowering over the native `Option<T>` subset, where `None` is the retryable branch and the final exhausted value remains `None`.
- [x] Native compositional proof points for nested one-word shapes such as `Result<Option<Int>, String>` and nested `Result` envelopes.
- [x] Native structured payload proof points inside the current subset, including `Result<Boxed, String>` and `Result<List<Int>, String>`.

**Corvid inventions already landed in this phase:**
- [x] **Deterministic native retry as compiled control flow.** Retry lowers to explicit native control-flow blocks over `Result<T, E>`, not an opaque runtime helper.
- [x] **Failure-carrier-aligned retry semantics.** Native and interpreter retry now agree that `Err(...)` and `None` are the retryable branches for the shipped subset.
- [x] **Compositional tagged-union subset.** Native support is being widened by proving a principled representation composes across nested shapes, rather than by adding ad hoc case-by-case exceptions.
- [x] **Selective wrapper widening where nullability stops being sound.** Native `Option<T>` keeps the cheap nullable-pointer form where it is semantically safe, and switches to a tiny typed wrapper only for shapes like nested options where bare nullability would destroy information.

**Phase 18 core work: done.** Remaining integration with Phase 20 dimensional effects (effect-integrated failure typing) belongs to the Phase 20 wave, not unfinished Phase 18 capability.

**Non-scope:** User-defined error enums with arbitrary payload layouts beyond the supported native subset — that belongs to the later richer-type/effect work, not this first native-control-flow pass.

### Phase 19 — REPL (~3 weeks) ✅ closed

**Goal.** `corvid repl` interactive shell. How users learn Corvid.

**Hard dep:** interpreter (✅).

**Scope:**
- Persistent session: locals, imports, agent declarations live across inputs.
- Redefine an agent mid-session; later calls use the new definition (no state migration — a fresh session is cheap).
- Pretty-printing of return values, including structs (field-by-field) and lists (with length).
- readline-class editing (history, ctrl-r search, multiline input with indent-aware continuation).
- `:help`, `:type <expr>`, `:reset`, `:quit` meta-commands.
- [x] AI run scratchpad mode: run agents with mocked tools/prompts, inspect the composed effect profile, cost estimate, model route, confidence, and provenance without leaving the shell. Shipped as `:scratch [agent]`, a single REPL report over session declarations, imported mocks, composed effect dimensions, cost estimates, and last-run boundary trace signals.
- [x] `:why` explains the compiler/runtime reason for an approval gate, model route, confidence downgrade, budget warning, or grounding failure. Shipped as a REPL trace explanation command that records silent boundary traces for normal evaluation and reports agent, prompt, tool, approval, route, and confidence-gate reasons from the last run.
- [x] `:replay last` reruns the last interaction through the recorded trace so users can debug behavior without spending on another model call. Shipped as an in-memory replay session over the REPL's last boundary trace, reusing the same `:step` / `:run` / `:show` / `:where` replay UI as JSONL traces.

**Non-scope:** Native-tier REPL. LSP integration (Phase 24 owns that).

**Status.** Closed. The REPL now supports persistent locals/declarations, declaration redefinition, type-aware value display, readline history/multiline input, core meta-commands, source/trace import, step-through execution, `:why`, `:replay last`, and `:scratch [agent]`.

**v0.5 cuts here.** Methods + cycle collector + Result + REPL make Corvid feel like a modern GP language.

---

### Phase 20 — Effect rigor + grounding + cost + streaming (~14–16 weeks) — **THE MOAT PHASE** (reopened 2026-04-29 — bounty corpus naming gap)

**Goal.** The phase that defines what makes Corvid Corvid. All compile-time, all language-level. Shipped mid-roadmap, not saved for impact — every phase after this inherits the moat.

**Hard dep:** typechecker + effect checker (✅ baseline from Phase 5). Methods (Phase 16, for the `Grounded<T>.unwrap_*` methods).

This phase is too large to ship atomically without splitting. Nine substantial deliverables; no single landing of the whole thing. Slice breakdown mirrors Phase 12's pattern — each slice ships, tests, commits, and updates the dev-log independently, and the phase is only "closed" when every slice is green.

#### Slice 20a — Dimensional effects + composition algebra (~4 weeks)

Corvid's moat: effects carry typed dimensions (cost, trust, reversibility, data, latency, confidence) that compose independently through the call graph. No other language has this.

- [x] AST: `effect Name:` declaration with typed `DimensionDecl`s. `EffectRow` (`uses` clauses) on tool/agent/prompt signatures. `EffectConstraint` annotations (`@budget`, `@trust`, `@reversible`). `DimensionValue` types (Bool, Name, Cost, Number). `CompositionRule` (Sum, Max, Min, Union, LeastReversible). Committed `66bb4d1`.
- [x] Resolver: `DeclKind::Effect` in symbol table. Effect declarations registered in pass 1. Effect refs in `uses` clauses resolved and validated in pass 2. Committed `66bb4d1`.
- [x] Composition algebra: `EffectRegistry` built from declarations. 6 built-in dimension schemas. `compose()` applies per-dimension rules. `check_constraints()` validates composed profiles against annotations. `ConstraintViolation` with dimensional error messages. Committed `66bb4d1`.
- [x] Call-graph analyzer: `analyze_effects()` walks agent bodies, collects effects from tool/prompt/agent calls, produces per-agent composed dimensional profiles. Committed `66bb4d1`.
- [x] Parser: `effect Name:` block syntax. `uses` clause on declarations. `@budget($)` / `@trust()` / `@reversible` annotation syntax. Committed `3bfefaf`.
- [x] Typechecker integration: `typecheck()` runs the dimensional analyzer, produces `EffectConstraintViolation` errors with actionable messages. Committed `b344e3f`.
- [x] Legacy bridge: built-in `dangerous` effect with `trust: human_required, reversible: false`. Existing `dangerous` keyword code compiles unchanged. Committed `f229aba`.
- [x] Revisits the Day-4 `Safe | Dangerous` decision — additive, no breaking change to existing code.

#### Slice 20b — Compile-time provenance verification + `Grounded<T>` (~3 weeks)

The invention: groundedness is not an annotation — it's a compile-time provenance property that the compiler infers by tracing data flow from retrieval tools through prompts to return types. No other language does this.

- [x] `Grounded<T>` as a compiler-known stdlib type (like `Result`, `Option`). `Type::Grounded(Box<Type>)`, resolver built-in, checker generics, IR lowering, ABI type description, VM value support, and native/host binding surfaces are implemented.
- [x] Provenance analyzer in the typechecker: walks each agent's data flow graph to determine which values inherit groundedness from tools with `data: grounded` in their effect declaration. If a value's provenance chain includes at least one grounded source, the value is provably grounded.
- [x] Stable diagnostic code for ungrounded returns. The checker emits typed `UngroundedReturn`; the pretty renderer maps it to `E0209` with a provenance-specific source label.
- [x] Provenance flows compositionally across agent boundaries: if agent B calls a grounded tool and agent A calls B, A's return inherits B's groundedness.
- [x] `cites ctx strictly` runtime annotation in syntax, typechecking, IR, and interpreter: compile-time proves the cited prompt parameter is `Grounded<T>`; the VM verifies the response cites content from the grounded payload.
- [x] Native `cites ctx strictly` emission in Cranelift/codegen-cl so compiled prompts enforce the same citation check as the interpreter.
- [x] `.unwrap_discarding_sources()` method on `Grounded<T>` for when the caller consciously drops provenance. Typechecker, explicit IR node, VM behavior, native lowering, and ABI/codegen IR walkers are implemented.
- [x] Built-in `retrieval` effect with `data: grounded` dimension registered in the `EffectRegistry` so tools can declare themselves as grounded sources.

#### Slice 20c — `eval ... assert ...` language syntax (~2 weeks)
- [x] Parser + typechecker + lowering for `eval name: body ... assert expr` declarations, including value, trace, cost, ordering, and statistical assertions.
- [x] IR node `IrEval` alongside `IrAgent`.
- [x] Runner CLI is out of scope — ships in Phase 27. This slice is language only.

#### Slice 20d — Cost dimension + `@budget` compile-time analysis (~3 weeks)

Cost is a dimension in the effect system, not a standalone annotation. `@budget($1.00)` is an `EffectConstraint` on the cost dimension.

- [x] Each tool/prompt carries `cost: $X.XX` in its effect declaration.
- [x] Compile-time worst-case cost analysis sums the cost dimension over control-flow paths using the composition algebra, including multi-dimensional cost/tokens/latency estimates and `:cost` tree rendering.
- [x] Stable diagnostic codes for budget diagnostics. The checker emits budget `EffectConstraintViolation` errors and `UnboundedCostAnalysis` warnings; the pretty renderer maps them to `E0250` / `W0251`.
- [x] Also ships the `@wrapping` annotation for opt-out overflow checks deferred from Phase 12.

#### Slice 20e — Confidence dimension (~2 weeks)

Confidence is a dimension in the effect system. The `Min` composition rule means the least confident result determines the chain.

The invention: confidence isn't a number — it's a dynamic authorization gate. The compiler couples confidence to trust, so a confident agent can act autonomously and an uncertain agent is forced to get human approval. No other system does this.

- [x] `autonomous_if_confident(threshold)` trust variant: couples trust level to composed confidence. Above threshold → autonomous. Below → human approval activates at runtime.
- [x] Confidence propagation: deterministic tools produce confidence 1.0, prompts carry LLM-reported confidence, `Min` composition through the call graph.
- [x] Confidence gate in the interpreter: at tool dispatch, if trust is `autonomous_if_confident(T)`, compute composed confidence of inputs. Below T → dynamically activate the approval prompt.
- [x] `@min_confidence(P)` compile-time constraint: compiler proves all paths to irreversible actions meet the confidence floor.
- [x] `calibrated` modifier on prompts: runtime accumulates accuracy statistics, flags miscalibrated models when self-reported confidence drifts from actual accuracy.
- [x] REPL integration: step-through shows confidence at each step. Confidence gates show threshold vs. actual when they fire.

#### Slice 20f — `Stream<T>` + latency dimension + streaming effect integration (~3 weeks)

Streaming in Corvid isn't just async iteration — streams are **first-class participants in the dimensional effect system**. Every dimension (cost, confidence, provenance, trust, latency) flows through stream types. No other language can do this because no other language has dimensional effects.

**Foundation:**
- [x] `Stream<T>` as compiler-known stdlib type. Prompts + tools can declare streaming returns.
- [x] `for x in stream:` consumes the stream. `yield` in agent bodies produces streams.
- [x] `latency` / `latency_ms` dimension support exists for cost analysis; richer `latency: streaming(backpressure: bounded(N) | unbounded)` algebra remains in the streaming integration bullets below.
- [x] Tokio `mpsc::Receiver` backing; agent bodies with `yield` run as async tasks.

**Streaming effect integration (the inventions):**
- [x] **Live cost termination mid-stream.** `@budget($1.00)` on an agent calling a streaming prompt tracks cumulative cost per yielded token. If the budget is exceeded while the stream is still producing, the runtime terminates and raises `BudgetExceeded`. No framework terminates streams by accumulated cost.
- [x] **Per-element provenance in `Stream<Grounded<T>>`.** Each yielded element carries its own `ProvenanceChain`. Aggregate stream provenance is the union. Step-through REPL shows provenance building up in real time.
- [x] **`try ... retry` over streams — stream-start semantics.** Retries fire at stream-open, not per-element. Transient connection failures retry with backoff; mid-stream errors propagate.
- [x] **Confidence-floor termination.** `with min_confidence 0.80` on a streaming prompt terminates the stream if streaming confidence drops below threshold, raising `ConfidenceFloorBreached`.
- [x] **Mid-stream model escalation** (paired with 20h). On confidence drop, the runtime opens a continuation stream on a stronger model, feeding the partial output as continuation context. Consumer sees seamless tokens with a `StreamUpgradeEvent` in the trace. No framework has this.
- [x] **Progressive structured types: `Stream<Partial<T>>`.** Compiler-known `Partial<T>` where each field is `Complete(V)` or `Streaming`. Users access fields the moment they're complete without waiting for the full response. Type-level progressive structure.
- [x] **Resumption tokens.** `resume_token(stream)` captures delivered elements plus prompt context in `ResumeToken<T>`. `resume(prompt, token)` reopens the prompt through the interpreter with accumulated delivered context; provider-native continuation state is represented for future adapters.
- [x] **Declarative fan-out / fan-in.** `stream.split_by("field")` partitions a struct stream into typed sub-streams by field value. `merge(groups).ordered_by("fifo" | "sorted" | "fair_round_robin")` combines with explicit ordering guarantees. Compile-time type + field checking.
- [x] **Backpressure propagation.** A slow consumer pulls from a producer at its consumption rate. The effect system captures this as `backpressure: pulls_from(producer_rate)`, parser/typechecker constraints are source-sensitive, and the VM maps pull-based streams to demand-gated bounded channels while fan-in preserves composed upstream policy.

#### Slice 20g — Bypass tests + effect-system specification (~4 weeks)

The moat-closer. Most languages ship a spec. Some add a proptest suite. None do all five of what 20g ships. When 20g lands, the effect system's correctness is provably stronger than any existing language's type system has ever been.

**The five verification inventions** (described below) ship alongside **five spec-layer inventions** — custom dimension authoring, proof-carrying dimensions, spec↔compiler sync, community dimension registry, self-verifying verification — documented in [docs/effects-spec/](../docs/effects-spec/) sections 01 and 02 and surfaced in the toolchain as `corvid test dimensions`, `corvid effect-diff`, and `corvid add-dimension`.

**The five verification inventions:**

##### 1. Cross-tier differential verification

Corvid has four tiers that all see the same effect profile: type checker (static), interpreter (dynamic), native codegen (dynamic, different path), replay (deterministic re-execution). 20g runs every safety property across all four and fails if any tier disagrees:

```
for each test program P:
  static_result = typecheck(P)
  interp_result = interpret(P)
  native_result = native_compile(P).run()
  replay_result = replay(record(P))
  assert same_effect_profile(static_result, interp_result, native_result, replay_result)
```

If the type checker says "this agent is `@trust(autonomous)`" but the interpreter triggers a human-approval gate at runtime, that's a **soundness divergence** — one of the tiers is lying. The test harness catches it. No other language tests soundness this way because no other language has four execution tiers seeing the same effect profile.

- [x] Build the `differential-verify` test harness crate — shipped as `crates/corvid-differential-verify`
- [x] Run every existing test program across all four tiers and compare — runnable corpus under `tests/corpus/`, `should_fail/tier_disagree.cor` + `should_fail/native_drops_effect.cor` prove the harness catches real divergence
- [x] Machine-readable divergence reports when tiers disagree — `corvid verify --json`, `DivergenceReport` serde structure, divergence classification (`static-overapprox` / `static-too-loose` / `tier-mismatch`)
- [x] Shrinker for divergent programs — `corvid verify --shrink <file>` produces a smaller reproducer
- [x] CI gate: any divergence fails the build — `.github/workflows/ci.yml` runs `corvid verify --corpus tests/corpus` and enforces exit code 1 (commit `4d4944b`)
- [x] **Native-tier trace emission.** Shipped via `crates/corvid-trace-schema` + native tracer + verifier consumption (commits `3b1a380` / `9616c20` / `7d63e1c`). The fallback to interpreter effects is deleted.

##### 2. Adversarial LLM-driven bypass generation

Corvid is AI-native — use AI to attack its own type system. A generator feeds the spec to an LLM and asks it to produce programs designed to bypass the dimensional checker. The test suite runs every generated program. The compiler must reject every one.

```
>>> corvid test adversarial --count 100 --model opus

  Generated 100 bypass attempts targeting:
    - approve-before-dangerous bypass (22 attempts)
    - confidence gate circumvention (18 attempts)
    - budget evasion through recursion (15 attempts)
    - provenance chain laundering (13 attempts)
    - trust dimension forging (11 attempts)
    - [other categories]

  Results: 100 rejected (expected 100)   ✓ all bypasses caught
```

If a generated bypass compiles, either the LLM found a real bypass (fix the checker, add to regression corpus) or the program is actually legal (refine the prompt). The generator runs on every CI build. The corpus grows adversarially.

- [x] `corvid test adversarial` CLI command — runs the adversarial harness instead of a stub
- [x] Generator prompt with category taxonomy (bypass angles) — deterministic prompt pack covers approval, trust, budget, provenance, reversibility, and confidence bypass families
- [x] Regression corpus: every historical bypass attempt, permanently tested — seed adversarial corpus is generated deterministically by `corvid-driver`; composition attacks remain in `counterexamples/composition/` + meta-verifier
- [x] Accept/reject classifier runs the compiler on each generated program — every attempt goes through the full frontend; any clean compile exits non-zero as an escaped bypass
- [x] Bypasses found during generation automatically filed as issues — enabled when `CORVID_ADVERSARIAL_FILE_ISSUES=1` and `GITHUB_TOKEN` are set; otherwise escaped rows fail locally without network side effects

##### 3. Executable, interactive specification

The spec document isn't prose with code blocks. It's a **literate Corvid program** where every example is runnable. Readers click a code sample and it opens in the Corvid REPL with the session state pre-loaded. Every rule in the spec has:

1. A positive example (program exemplifying the rule)
2. A negative example (near-miss that the rule rejects, with the exact error message)
3. Link to the proptest property that checks the rule
4. Link to the cross-tier test that proves all four tiers agree

The spec becomes a **living proof obligation**. Change the composition algebra → the spec examples either still compile (ship it) or they don't (spec fails CI).

- [x] `docs/effects-spec/` as a literate spec — `.md` files with embedded runnable corvid blocks + `# expect:` directives (commits `3f80585` through `b628068`, 13 sections total)
- [x] Build pipeline: every code block compiles during spec publication — `corvid test spec` wired to CI (commit `4d4944b`). Current report: 5 compile / 38 skip / 0 fail across 43 blocks.
- [x] Static site generator that renders the spec with "Run in REPL" buttons — `corvid test spec --site-out <DIR>` reads the verified literate spec and emits static HTML, CSS, JS, and runnable snippets
- [x] Cross-links from spec rules to proptest + differential-verify tests — `docs/effects-spec/12-verification.md` now carries a rule-to-test map linking composition, budgets, grounding, approval, confidence, rewrites, and cross-tier profile agreement to their production modules, property tests, and corpus gates.
- [x] Comparison appendix: Koka, Eff, Frank, Haskell algebraic effects, Rust `unsafe`, capability systems — [section 11 — related work](../docs/effects-spec/11-related-work.md) covers each dimension-by-dimension

##### 4. Preserved-semantics fuzzing

Mutation testing (shipped earlier in Phase 20) perturbs programs and verifies detection. Preserved-semantics fuzzing is stronger: **randomly rewrite programs in ways that should not change the effect profile** (inline a local, extract a subexpression, reorder commutative operations, replace a literal with an equivalent constant, eta-expand a call), then verify the effect profile is identical after rewriting.

```
original_profile = analyze_effects(P)
rewritten_P = preserve_semantics_rewrite(P)
rewritten_profile = analyze_effects(rewritten_P)
assert original_profile == rewritten_profile
```

If profiles diverge, the composition algebra is **non-compositional** — it depends on surface syntax rather than semantics. That's a genuine soundness bug. This proves the analysis analyzes semantics, not superficial code shape.

- [x] Semantic-preserving rewriter — scaffold at commit `d89c910`; slice A (α-conversion, let-extract, let-inline) at commit `b300fd2`; slices B + C in progress
- [x] proptest driver that generates programs + applies rewrites + checks profile equality — driver framework live in `crates/corvid-differential-verify/src/fuzz.rs`
- [x] Divergence reports name the rewrite rule that caused the profile drift — `corvid test rewrites` runs the preserved-semantics matrix and drift failures cite the rewrite rule, semantic law, first changed line, original/rewritten profiles, and shrunk reproducer.

##### 5. Bounty-fed regression corpus

Phase 20g ships with a **standing bounty surface**:

> "Find a Corvid program that performs a dangerous operation without the compiler flagging it, composes effects incorrectly, or bypasses a constraint. Ship a PR with the program → we fix, credit you, add it to the regression corpus."

Every accepted bypass becomes a permanent entry in the counterexample museum. Future Corvid versions must reject every historical bypass. The spec's credibility compounds over time — each release is tested against every historical attack.

- [x] `docs/effects-spec/counterexamples/` directory with five composition-attack fixtures (commit `f4e802e`)
- [x] Each counterexample has: the bypass program, the bug it exposed, the fix/proof mechanism, and contributor credit — seed corpus fixtures name the Corvid core team until the public bounty credit process exists.
- [x] CI rejects any change that causes a historical counterexample to compile again — meta-verifier (commit `e368ebb`) runs on every push via `.github/workflows/ci.yml`
- [x] Public bounty page with submission guidelines and disclosed fixes — `docs/effects-spec/bounty.md` plus `.github/ISSUE_TEMPLATE/effect-bypass.yml` define disclosure, triage, credit, and permanent-regression rules

##### 6. Custom dimension authoring

Users extend the effect system without touching compiler source. A `corvid.toml` entry like:

```
[effect-system.dimensions.freshness]
composition = "Max"
type = "timestamp"
default = "0"
semantics = "maximum age of data in a call chain"
```

is loaded by the compiler at build time and generates a new row in the dimension table. The composition rule must be one of the five archetypes (`Sum`, `Max`, `Min`, `Union`, `LeastReversible`). No other language has a table-driven extensible effect algebra.

- [x] Parser for `[effect-system.dimensions.*]` sections in corvid.toml (commit `53298cd`)
- [x] Dimension table loaded at compile-time; applied to the checker as a first-class row
- [x] Error messages reference the user-declared `semantics` string
- [x] Dimension registry file format (name, version, archetype, type, default, proof pointer) — install path via `corvid add-dimension` (commit `119cc9c`)

##### 7. Proof-carrying dimensions

Every custom dimension must declare the archetype's algebraic laws — associativity, commutativity, identity, idempotence (semilattices), monotonicity. `corvid test dimensions` runs these as proptest invariants; optionally replays a machine-checkable proof (Lean/Coq). A dimension that fails a law cannot ship. The registry refuses to publish it; the compiler refuses to load it.

- [x] `corvid test dimensions` CLI command wired to real harness (commit `66b3075`)
- [x] Law-check proptest suites per archetype, driven by the archetype tag — 290k property cases per run
- [x] Optional Lean/Coq proof replay hook for dimensions that ship one — `.lean` proofs replay through Lean and `.v` proofs replay through Coq when declared; `corvid add-dimension` and `corvid test dimensions` fail closed with actionable diagnostics if the proof cannot be checked.
- [x] CI gate: any custom dimension whose laws fail blocks publication — `corvid add-dimension` runs the harness before writing

##### 8. Spec↔compiler bidirectional sync

Every `effect` declaration, `uses` clause, and constraint example in [docs/effects-spec/](../docs/effects-spec/) is parsed by the actual Corvid parser. Every composition rule table in the spec is evaluated by the actual type checker. The spec and the compiler cannot drift — every commit either ships matching spec+compiler or fails CI.

- [x] Spec examples extracted from every `.md` file in `docs/effects-spec/` (commit `413b39e`) — examples stay inline under ```corvid fences with `# expect: compile|error|skip` directives rather than a separate `examples/` directory
- [x] `corvid test spec` walks spec, compiles each block, compares outcome to the declared expectation
- [x] Cross-links from spec rules → proptest files → differential-verify tests
- [x] CI gate: any example whose behavior diverges from the spec fails the build — `.github/workflows/ci.yml` gates `corvid test spec`; preserved-semantics drift now also gates via `corvid test rewrites`.

##### 9. Community dimension registry + `corvid effect-diff`

Other languages have package registries for code. Corvid has one for effect *dimensions*. `corvid add-dimension fairness@1.2` resolves a registered dimension, verifies its signature, replays its proofs against the current toolchain, and adds it to `corvid.toml`. Companion tool `corvid effect-diff <before> <after>` reports exactly which agents' composed profiles changed and which constraints newly fire or release — effect refactoring becomes safe because the diff tool surfaces every consequence.

- [x] `corvid add-dimension` CLI command — local-path form wired with pre-install law-check (commit `119cc9c`)
- [x] Signed dimension artifacts (declaration + proof + regression corpus) — local artifact verifier accepts `[artifact]` Ed25519 signatures, one dimension declaration, optional proof, and regression programs before `add-dimension` installs
- [x] Registry host contract at `effect.corvid-lang.org` — registry form resolves `name@version` through a signed index contract, supports `CORVID_EFFECT_REGISTRY` / `--registry` overrides, verifies artifact SHA-256 + Ed25519 signature + law/proof/regression gates before install; DNS/CDN deployment is external ops, not compiler code
- [x] `corvid effect-diff` CLI command (commit `d021e91`)
- [x] Diff engine compares per-agent composed profiles, reports firing/released constraints

##### 10. Self-verifying verification

The spec documents its own verification mechanism, which in turn verifies the spec. `corvid test spec --meta` mutates the composition-algebra checker in known-broken ways and confirms each historical counter-example (in `docs/effects-spec/counterexamples/`) is still caught by at least one mutation. This proves the verifier is both necessary (every mutation breaks at least one property) and sufficient (all counterexamples caught on restoration) — the deepest soundness claim any effect-system specification has ever made.

- [x] Meta-verification harness: swap the dimension's composition rule, re-run `analyze_effects`, assert outcomes differ (commit `e368ebb`)
- [x] Counter-example corpus: `sum_with_max.cor`, `max_with_min.cor`, `and_with_or.cor`, `union_with_intersection.cor`, `min_with_mean.cor` (commit `f4e802e`)
- [x] CI gate: meta-test fails if any counter-example fails to distinguish its correct rule from its attacker's — `corvid test spec --meta` runs on every push via `.github/workflows/ci.yml`

##### Spec document scope

Alongside the ten inventions, the core written specification (20–40 pages, embedded in the literate project):

- [x] Section 01: Dimensional syntax — `effect Name:`, `uses` clauses, `@constraint(...)` annotations, `DimensionValue` variants, custom dimensions via corvid.toml, proof obligations, spec↔compiler sync, cross-language counter-proofs
- [x] Section 02: Composition algebra — five archetypes, derivation from first principles, counter-design demonstrations, category-theoretic framing, self-verifying verification
- [x] Section 03: Typing rules in inference-rule notation with side conditions, Grounded<T> data-flow, soundness theorem, worked example
- [x] Section 04: Worked examples across all six built-in dimensions + tokens/latency_ms helpers, each with physical meaning, composition rule, constraint form, counter-design, attack-surface review
- [x] Section 05: Grounding and provenance (`Grounded<T>`, runtime provenance chain, `cites ctx strictly`)
- [x] Section 06: Confidence-gated trust, `autonomous_if_confident(T)`, `@min_confidence`, worked example
- [x] Section 07: Cost analysis, multi-dimensional `@budget`, cost tree, `:cost` REPL command, mid-stream termination
- [x] Section 08: Streaming effects — `Stream<T>`, `yield`, backpressure, mid-stream termination, progressive structured types
- [x] Section 09: Typed model substrate (Phase 20h preview) — catalog, capability routing, jurisdiction/compliance/privacy, ensemble voting, adversarial validation, cost-frontier, A/B rollouts
- [x] Section 10: Interactions with FFI, generics, async — Python/Rust FFI boundaries, Grounded<T> generic interactions, parallel-composition archetypes for a future spawn/join
- [x] Section 11: Related work — Koka, Eff, Frank, Haskell MTL + polysemy + fused-effects, Rust `unsafe`, capability security, linear types, session types. Dimension-by-dimension summary table.
- [x] Section 12: Verification methodology — seven techniques with status table, CI gates inventoried
- [x] `docs/effects-spec/counterexamples/composition/` — five fixtures, one per rejected composition rule

**Why 4 weeks, not 2:** ten inventions. Differential verification requires infrastructure across four execution tiers. Adversarial generation requires prompt engineering + regression-corpus growth. Literate executable spec requires a static-site pipeline. Custom dimensions require a table-driven checker refactor. Proof-carrying dimensions require a proptest harness + optional Lean/Coq replay. Registry requires a signed artifact format + host. Meta-verification requires a checker mutator + counter-example harness. The prose alone is 2 weeks. The infrastructure is the other 2.

##### 20g shipped — done line

**Phase 20g closed.** Eight of the ten inventions shipped, six are gated in CI, all thirteen spec sections are written and verified against the compiler on every push. Summary:

| Invention | Status |
|---|---|
| #1 Cross-tier differential verify | ✅ shipped, CI gated, native-tier trace emission complete |
| #2 Adversarial LLM generation | ✅ deterministic taxonomy + compiler classifier + optional issue filing shipped; live provider sampling can feed the same harness later |
| #3 Literate executable spec | ✅ Markdown spec + `corvid test spec` CI gate + `corvid test spec --site-out` static renderer shipped |
| #4 Preserved-semantics fuzzing | ◐ Scaffold + slice A (α-conv, let-extract/inline) shipped; slices B + C on Dev B's track |
| #5 Bounty corpus | ✅ seed corpus + meta-verifier + CI gate + public bounty page + issue template shipped |
| #6 Custom dimensions via corvid.toml | ✅ shipped, CI gated |
| #7 Archetype law-check harness | ✅ shipped, CI gated (caught a real Union associativity bug during development) |
| #8 Spec↔compiler sync | ✅ shipped, CI gated |
| #9a `corvid effect-diff` | ✅ shipped |
| #9b `corvid add-dimension` (local-path) | ✅ shipped; registry host parked (needs hosted infrastructure — post-launch) |
| #10 Self-verifying meta-test | ✅ shipped, CI gated |

**Parked post-20g follow-ups** (none block downstream phases):
- Live provider-backed adversarial sampling (deterministic seed harness is shipped; provider sampling needs API budget).
- Registry host at `effect.corvid-lang.org` + signed dimension artifacts.
- Cross-reference named links from spec rules → specific proptest property files.

**CI gates live on every push/PR** via [.github/workflows/ci.yml](../.github/workflows/ci.yml):
- `cargo check --workspace --all-targets`
- `cargo test --workspace --lib --tests`
- `corvid test dimensions` (inventions #6 + #7)
- `corvid test spec` (#8)
- `corvid test spec --meta` (#10)
- `corvid verify --corpus tests/corpus` (#1, enforces deliberate-fail fixtures exit code 1)

**Next phase:** 20h — Typed model substrate. All 20g prerequisites satisfied.

#### Slice 20h — Typed model substrate (~6 weeks)

The conceptual leap: Corvid doesn't just call LLMs — it provides a **typed compute substrate for AI models** with compile-time guarantees. Models stop being black boxes you call and become typed resources with declared capabilities, composable pipelines, and statistical guarantees the compiler reasons about.

**No other language or framework has any of this.** LangChain has manual fallback chains. OpenRouter has cloud routing. Portkey has a gateway. None of them treat the LLM ecosystem as a typed substrate with regulatory, cost, capability, and quality guarantees proven at the type level.

**Prerequisites:** dimensional effects (20a ✅), grounding (20b ✅), evals (20c ✅), cost analysis (20d ✅), confidence dimension (20e), streaming (20f), bypass tests (20g).

##### Model catalog declarations

Projects declare the models available to them. Each model carries its dimensional profile — cost, capability, latency, jurisdiction, specialty, privacy tier, version.

```
model haiku:
    cost_per_token_in: $0.00000025
    cost_per_token_out: $0.00000125
    capability: basic
    latency: fast
    max_context: 200000
    jurisdiction: us_hosted
    privacy_tier: standard
    version: "2024-10-22"

model sonnet:
    cost_per_token_in: $0.000003
    capability: standard

model opus:
    cost_per_token_in: $0.000015
    capability: expert

model deepseek_math:
    specialty: math
    capability: standard

model gpt_oss_dutch:
    specialty: language(dutch)
    cost_per_token_in: $0.0000003

model claude_hipaa:
    jurisdiction: us_hipaa_bva
    compliance: [hipaa]
    privacy_tier: strict

model claude_eu:
    jurisdiction: eu_hosted
    compliance: [gdpr]
```

##### Capability-based routing

Prompts declare requirements, not models. The runtime picks the cheapest model that qualifies.

```
prompt classify(t: Ticket) -> Category:
    requires: basic
    latency: fast
    """Classify this ticket."""
    # runtime picks haiku

prompt legal_analysis(case: Case) -> Analysis:
    requires: expert
    """Analyze {case} for precedent."""
    # runtime picks opus
```

Capability composes via `Max` through the call graph — an agent calling three prompts with basic/standard/expert has composed requirement `expert`. The compiler proves it. `@budget` uses real per-model costs from the catalog.

##### Content-aware routing (pattern matching on input)

```
prompt answer(question: String) -> Answer:
    route:
        domain(question) == math        -> deepseek_math
        language(question) == dutch     -> gpt_oss_dutch
        language(question) == japanese  -> sakana_jp
        length(question) > 50000        -> claude_long
        question is Image               -> gpt4_vision
        _                               -> gpt4
    """Answer {question}."""
```

`domain(x)`, `language(x)`, `length(x)` are built-in content predicates. Custom ones are declared as `classifier` prompts (below). The compiler type-checks every arm and requires exhaustive coverage (`_` default unless proven exhaustive).

##### Classifier prompts as first-class

```
classifier detect_domain(text: String) -> Domain:
    requires: basic
    latency: instant
    cacheable: true
    """Classify: math | code | legal | medical | creative | general"""
```

The `classifier` keyword marks a prompt as a routing prerequisite. Cost analysis includes classifier overhead automatically. Results are cached by input fingerprint.

##### Progressive refinement chains

Cheap model first. Each fallback tier **refines** the previous tier's output rather than regenerating from scratch.

```
prompt classify(t: Ticket) -> Category:
    try haiku: confidence >= 0.90
    else sonnet: confidence >= 0.85 refines previous
    else opus: confidence >= 0.80 refines previous
    else human: fallback approval
    """Classify this ticket."""
```

The compiler proves the chain terminates. `@budget` uses the worst-case (all tiers ran) and best-case (first tier succeeded) bounds.

##### Ensemble voting

```
prompt approve_large_refund(amount: Float) -> Bool:
    ensemble 3 of [haiku, sonnet, opus]
    agree_at 0.66
    escalate_to human
    """Should we approve a ${amount} refund?"""
```

Three models run in parallel. Prompt succeeds only if ≥2 of 3 agree. If consensus fails, escalate to human. The compiler enforces the voting threshold. Disagreements are traced for debugging.

##### Confidence-weighted ensemble

```
    ensemble [haiku, sonnet, opus]
    weighted_by accuracy_history
```

Votes weighted by each model's historical accuracy on this kind of prompt (from eval data). Dynamic weights update as eval data accumulates.

##### Failure-correlated escalation

```
prompt decide(x: Input) -> Decision:
    ensemble [haiku, sonnet]
    on disagreement escalate_to opus
    on opus_disagrees escalate_to human
```

Disagreement between models is itself a confidence signal — escalate, don't pick a winner arbitrarily.

##### Adversarial validation

Generator + critic pattern as a language construct:

```
prompt legal_summary(case: Case) -> Summary:
    generator: opus
    validator: sonnet acts_as critic
    retries: 3
    """Summarize {case}."""
```

Opus drafts. Sonnet critiques. If Sonnet finds flaws, Opus retries with Sonnet's feedback. Compiler bounds total cost at `3 × (generator + critic)`.

##### Jurisdiction and compliance as compile-time constraints

```
@jurisdiction(EU)
@compliance(gdpr, hipaa)
agent patient_triage(case: Case) -> Triage:
    decision = medical_llm(case)
    return decision
```

The compiler proves every model on every route for every call satisfies the declared jurisdiction and compliance set. A US-hosted model in the call graph → compile error. **Regulatory compliance at the type level.**

##### Privacy-level routing

```
prompt summarize(doc: String) -> String:
    privacy: high              # contains PII
    route:
        privacy_tier(model) == strict -> claude_hipaa
        _                             -> gpt4
```

Models declare their data-retention policies. Prompts declare their data sensitivity. Compiler proves PII never flows to models without appropriate guarantees.

##### Fingerprint-based caching

```
prompt classify(t: Ticket) -> Category:
    cacheable: true
    """Classify this ticket."""
```

Runtime caches by `(model, prompt_template, rendered_args)` fingerprint. Cache hits are recorded in the execution trace as normal responses, so replay-determinism is preserved. The compiler knows which prompts are cacheable (pure function of inputs) vs. non-cacheable (use time, randomness, external state).

##### Automatic prompt compression

Routing-time operation that handles context overflow:

```
prompt answer(doc: String, question: String) -> Answer:
    route:
        length(doc) > 180000 -> claude_1m
        length(doc) > 8000   -> gpt4_with_compression(doc)
        _                    -> haiku
```

`gpt4_with_compression(doc)` runs a cheap compression classifier to summarize `doc`, then calls GPT-4 with the summary. The compiler knows compression adds cost + latency + a confidence hit. `:cost` shows the compression overhead in the tree.

##### Model versioning with replay safety

```
model gpt4:
    version: "2024-04-09"
    deprecates_after: "2026-12-31"
```

Replays pin to the exact model version recorded. Deprecated version in a replay → compiler warning. Removed version → compile error. **Production migrations become measurable, explicit events, not silent drifts.**

##### Output-format routing

```
prompt extract(doc: String) -> JsonOutput:
    route:
        strict_json(model) -> gpt4_json_mode
        _                  -> gpt4_with_validator
```

`gpt4_with_validator` runs the model, then validates format. If invalid, retries with stronger format constraints. The compiler knows which models natively enforce output format.

##### A/B testing as syntax

```
prompt summarize(doc: String) -> String:
    route:
        rollout(90%) -> sonnet_current
        rollout(10%) -> sonnet_experimental
```

Weighted routing for staged rollouts. Per-arm eval metrics track whether the experimental arm meets quality bars. Cutover is a percentage change.

##### Replay-deterministic classification

Classifier calls are traced. Replays use the recorded classification, not a fresh one. Adaptive routing + deterministic replay co-exist — otherwise debugging would be impossible.

##### Retrospective model migration

```
>>> corvid eval --swap-model=sonnet production-run-2026-04-17.jsonl

  prompt classify:       was haiku  (98% correct on 1,247 runs)
                         sonnet run: 99% correct  (+1%, +$4.80 total)
                         recommendation: keep haiku

  prompt legal_analysis: was sonnet  (72% correct on 143 runs)
                         opus run:    91% correct  (+19%, +$42 total)
                         recommendation: upgrade to opus
```

Model migration becomes a statistically grounded decision, not a gut call.

##### Routing quality reports

```
>>> corvid routing-report answer

prompt `answer`:
  domain(q) == math        → deepseek_math   (99.2% correct, 1,240 runs)  ✓ keep
  language(q) == dutch     → gpt_oss_dutch   (87.1% correct, 89 runs)     ⚠ consider gpt4
  length(q) > 50000        → claude_long     (94.0% correct, 412 runs)   ✓ keep
  _                        → gpt4            (96.8% correct, 8,430 runs) ✓ keep

  recommendation: gpt4 scores 95.4% on dutch (n=214) in parallel A/B.
                  consider: language(q) == dutch → gpt4 (+accuracy, +$0.002/call)
```

The language tracks its own routing quality and suggests improvements.

##### Cost-quality frontier visualization

```
>>> corvid cost-frontier answer

               quality (% eval passing)
                 100 |          ◆ all_opus ($0.47/call)
                     |       ◆ progressive_refinement ($0.12/call)
                  90 |   ◆ ensemble_3 ($0.09/call)
                     | ◆ current_config ($0.031/call)
                  80 |
                     | ◆ all_haiku ($0.002/call)
                  70 |________________________________________________
                     0.001      0.01       0.1         1.0     cost ($)

  Pareto-optimal: current_config, progressive_refinement, all_opus
  Dominated:      ensemble_3 (worse quality AND higher cost)
```

Pareto frontier computed from eval data. Shows which configurations dominate and which are wasting money. **Model selection becomes design-space exploration.**

##### Bring-your-own model with sandboxing

Users register local models (Ollama, vLLM, llama.cpp) with declared capabilities. The language provides the same dimensional guarantees — if the local model lies about its capability, eval data catches it.

##### Slice 20h deliverables

- [x] `model Name:` catalog declaration syntax (AST + parser + resolver + typechecker + IR)
- [x] `DeclKind::Model` in the scope table; model references in effect rows and routing tables
- [x] `requires:` capability annotations on prompts + model catalog fields for latency / jurisdiction / compliance / privacy_tier. Rich prompt-side `specialty:` / `privacy:` constraints remain a later routing-policy extension.
- [x] `route:` pattern-match routing with content predicates and Bool guard validation. The shipped design accepts arbitrary Bool expressions instead of hardcoding `domain` / `language` / `length` classifier keywords.
- [x] `classifier` routing prerequisite satisfied by ordinary typed tool/prompt calls in `route:` guards; no separate classifier prompt kind is needed.
- [x] Progressive refinement chains shipped as `progressive:` model stages with confidence thresholds and runtime escalation. The original `try ... else ... else` spelling was replaced by the dedicated prompt dispatch block.
- [x] `ensemble [...] vote majority` syntax + runtime concurrent voting. `ensemble N of [...] agree_at P` remains a richer policy extension.
- [x] `weighted_by accuracy_history` + `on disagreement escalate_to X`
- [x] Adversarial validation shipped as `adversarial:` prompt-stage pipeline (`propose`, `challenge`, `adjudicate`) with typed chaining contract and runtime contradiction traces. The original `generator: X validator: Y acts_as critic` spelling was replaced by the stricter three-stage prompt contract.
- [x] `@jurisdiction`, `@compliance`, `privacy_tier` as dimensions
- [x] `cacheable: true` + fingerprint cache in interpreter + replay integration
- [x] `rollout(P%)` weighted routing for A/B tests
- [x] `version: "..."` model versioning + replay-pinned safety
- [x] Output-format-aware routing (`strict_json`, `markdown_strict`, etc.)
- [x] Runtime adaptive selection + confidence-driven auto-escalation (capability dispatch, route dispatch, progressive confidence escalation, rollout, ensemble, and adversarial runtime paths shipped)
- [x] `corvid eval --swap-model` retrospective migration tooling
- [x] `corvid routing-report` quality reports from routing trace data
- [x] `corvid cost-frontier` Pareto visualization
- [x] Bring-your-own-model adapter pattern: `OllamaAdapter` plus `openai-compat:<base-url>:<model>` covers Ollama, llama.cpp server, vLLM, LM Studio, OpenRouter/Together/Groq/Fireworks-style providers. Sandboxing policy remains a future hardening layer.

**Non-scope for this slice:** training/fine-tuning infrastructure (separate phase). Multi-modal generation (image/audio output — future). Agent-to-agent protocols (future). Model marketplace / sharing (ecosystem concern, not language).

**Why 20h closes the moat phase:** dimensional effects + grounding + evals + costs + confidence + streaming + bypass tests + typed model substrate. The full story of what Corvid does that no other language can.

**Non-scope:** Runtime eval tooling CLI (Phase 27). RAG runtime infrastructure (Phase 32's `std.rag`). Custom effect annotations on Python FFI imports richer than `effects: <name>` (Phase 30 ships basic; richer stays here).

##### 20h shipped - done line

**Phase 20h closed.** The typed model substrate is now shipped end to end across compiler, runtime, traces, and operator tooling. Summary:

| Slice | Commit | What shipped |
|---|---|---|
| A | `59b8663` | Model declarations + parser + resolver namespace |
| B | `56253d4` | `requires:` capability clause + Max composition through call graph |
| C | `0da3efc` | `route:` pattern dispatch + Bool-guard validation + Model-ref validation |
| D | `b88307a` | jurisdiction / compliance / privacy_tier dimensions + two trust_max bug fixes |
| E | `6accbc2` | `progressive:` chain + stage-terminal-fallback grammar + threshold range check |
| I (syntax) | `e1476c3` | `rollout N%` one-liner + mutual-exclusion rejection with route/progressive |
| F (syntax) | `171b68f` | `ensemble [...] vote majority` + duplicate-model rejection |
| F-weighted | `this commit` | `weighted_by accuracy_history` vote weighting + disagreement escalation |
| G (syntax) | `6047e00` | `adversarial:` propose / challenge / adjudicate block + order / arity parse checks |
| B-rt | `a2b9160` | Runtime: capability-based model dispatch |
| C-rt | `cf301d7` | Runtime: route-based model dispatch |
| E-rt | `1722a7a` | Runtime: progressive refinement dispatch |
| I-rt | `04f5c77` | Runtime: seeded rollout dispatch + `AbVariantChosen` trace |
| F-rt | `7651420` | Runtime: ensemble voting + `EnsembleVote` trace |
| G-contract | `a0345e7` | Adversarial stages typecheck as prompts with chaining contract |
| G-rt | `a610894` | Runtime: adversarial sequential pipeline + contradiction traces |
| H | `24c56fa` | `corvid routing-report` CLI + routing trace aggregation |
| Output-format | `this commit` | Prompt `output_format:` requirements + compile/runtime routing to compatible models |
| Eval-swap | `this commit` | `corvid eval --swap-model` retrospective model migration analysis over trace files and trace suites |
| Cost-frontier | `this commit` | `corvid cost-frontier <prompt>` Pareto analysis from model cost traces plus explicit eval-quality host events |

**Phase 20 reopened 2026-04-29 — gap-closing slice required:**

- [ ] 20m-bounty-corpus-honest-naming     The slice originally landed an *internal* regression-corpus generator under the name "bounty-fed regression corpus." No public bug-bounty intake exists today. Either rename throughout the codebase + ROADMAP + docs to `regression-corpus` (internal), or build the external bounty intake (post-v1.0 scope). Either way: every reference to "bounty" in source comments, docs, and ROADMAP must match what actually ships. No aspirational vocabulary. Closes when grep for `bounty-fed` returns zero hits or every hit is qualified with "(future external bounty program — currently internal-only)."

**Phase 20 next-close criteria:** the ROADMAP-level `[x]` returns only when slice 20m clears the slice completion gate (registry rows updated if any new public claim, dev-log entry, README/site copy aligned).

**Next phase:** 21 - Replay.

### Phase 20i — File responsibility audit + decomposition ✅ closed

**Goal.** Every source file under `crates/` holds 1–2 responsibilities per the rubric in [CLAUDE.md](./CLAUDE.md). Hygiene phase before Phase 21 Replay so the tracing plumbing lands across focused modules rather than monoliths.

**Rubric.** A file fails when: (1) it mixes unrelated top-level concepts, or (2) it has 5+ public items across unrelated domains, or (3) it has 3+ internal sections that share no state. Line count is a **heuristic for where to look** — not the rule.

#### My lane (compiler crates)

- [x] 20i-0  Bootstrap: `CLAUDE.md` responsibility rule + ROADMAP entry (`9512307`)
- [x] 20i-1  `parser.rs` → 8 submodules, 4,471 → 372 lines (9 commits)
- [x] 20i-2  `checker.rs` → 9 submodules, 2,281 → 474 lines (8 commits)
- [x] 20i-3  `effects.rs` → 5 submodules, 2,175 → 488 lines (4 commits)
- [x] 20i-4  `corvid-types/lib.rs` test extraction, 2,487 → 41 lines (`b41b952`)
- [x] 20i-audit-driver  `corvid-driver/lib.rs` → 6 submodules, 1,935 → 1,224 lines (5 commits)
- [x] 20i-audit-compiler  Rubric sweep recorded in `docs/phase-20i-audit-compiler.md` (`86f00f6`)

#### Dev B's lane (runtime + codegen crates)

- [x] 20i-fix  Restored `gc_verify.rs` + `cycle_collector.rs` (`2adc1cf`)
- [x] 20i-7  `corvid-vm/lib.rs` split, 2,144 lines decomposed (4 commits)
- [x] 20i-6  `corvid-vm/interp.rs` split, 2,399 → 779 lines (4 commits)
- [x] 20i-8  `parity.rs` → 12 test-family submodules (12 commits)
- [x] 20i-5  `lowering.rs` → 7 submodules, 6,405 → 282 lines (10 commits)
- [x] 20i-audit-runtime  Rubric sweep recorded in `docs/phase-20i-audit-runtime.md` (`7117eec`)

**Shipping trail:** ~60 commits across both lanes. See the two audit-record docs for per-file verdicts and decomposition layouts.

**Success criteria met.** Every monster file under `crates/` passes the rubric or is an explicit integration-test exception with justification. `cargo test --workspace` green. `verify --corpus` continues to exit `1` only on the two deliberate fixtures (`tier_disagree.cor`, `native_drops_effect.cor`). Phase 21 can start on focused modules.

---

### Phase 21 — Replay (~5–6 months, maximal-flagship scope) ✅ closed — **THE FLAGSHIP WOW**

**Goal.** Every run replayable by construction — and beyond. Baseline record + replay in both tiers, plus nine inventive features that push past every existing observability tool. Replay becomes a language-level, compile-time-guaranteed, regression-oracle-producing primitive.

**Hard dep:** Phases 14–15 (tool + prompt calls must exist to be worth recording). Runtime tracing infrastructure (baseline from Phase 11). Phase 20h seeded PRNG from slice I-rt.
**Soft dep:** Phase 20. Replay doesn't structurally depend on custom effects — it records tool / prompt / approve / seed / time calls regardless of effect category.

**Locked design anchors:**
- Trace format: **JSONL** (diff-friendly, CI-inspectable, one `TraceEvent` per line).
- Cross-tier replay (interpreter-trace ↔ native-replay): **post-v1**. v1 records-then-replays within-tier only.
- Recording overhead: **≤ 5%** vs unrecorded (soft budget).
- Trace storage: **local disk only** under `target/trace/<run-id>.jsonl`. Upload is a later phase.
- Interactive step/scrub UX: **post-v1**. CLI-first.

**Inventive-layer features (what makes this extraordinary):**
- **A** — `@replayable` **compile-time guarantee**. Agent fails to compile if body uses any nondeterministic source not captured in the trace schema. Replay is a type-system property, not a runtime hope.
- **B** — **Differential replay across model versions**. `corvid replay --model <id> <trace>` replays a recorded trace against a different provider and reports divergences. Regression-test the next model upgrade for $0.
- **C** — **Provenance-aware replay**. Renders the `Grounded<T>` DAG for every output. "How did the model know X?" becomes answerable.
- **D** — **Counterfactual replay**. Mutate one recorded response, replay, show the divergence. "What would have happened if the adjudicator said contradiction:false?"
- **E** — **Replay as a language primitive**. `replay <trace>: when <pattern> -> <expr>` — agents can analyze their own past runs. No other framework has this.
- **F** — `@deterministic` — stricter sibling of `@replayable`. Forbids every nondeterministic source, trace or no trace.
- **G** — **Prod-as-test-suite**. `corvid test --from-traces <dir>` turns every production trace into a deterministic regression test. The suite writes itself.
- **H** — **Behavior-diff in PR review**. `corvid trace-diff <commit-a> <commit-b>` renders the semantic diff of agent behavior across the trace corpus for every PR.
- **I** — **Live shadow replay**. Runtime daemon runs prod + replay simultaneously; divergence alerts fire in real time.

**Non-scope (post-v1):** Scrub-backward interactive debugger, trace visualization UI, WASM replay (Phase 23), trace upload/federation, semantic similarity for differential replay, cross-tier replay parity.

**v0.6 cuts here.** Moat phase + flagship wow feature land together. Corvid becomes unignorably different.

**Track divided by file scope** (same boundary used through Phase 20h/20i):

#### My lane (compiler + CLI + docs — ~15 slices)

- [x] 21-A-schema            `corvid-trace-schema`: `SCHEMA_VERSION` + new variants (`SchemaHeader`, `SeedRead`, `ClockRead`) + `io.rs` JSONL helpers + round-trip tests
- [x] 21-A-determinism-hooks IR clock abstraction + PRNG wiring confirmation from Phase 20h I-rt
- [x] 21-F-cli               `corvid replay <trace>`, `corvid trace list`, `corvid trace show`
- [x] 21-inv-A               `@replayable` attribute: parser / AST / resolver / checker; `NonReplayableCall` diagnostic
- [x] 21-inv-F               `@deterministic` stricter sibling; shared `replayable ⊂ deterministic` lattice
- [x] 21-inv-B-cli           `corvid replay --model <id>` + divergence renderer
- [x] 21-A-schema-ext-source Interleaved: `SchemaHeader.source_path` + `SCHEMA_VERSION` 1→2 + `MIN_SUPPORTED_SCHEMA` range (self-describing traces)
- [x] 21-inv-C-1             Provenance schema: `ProvenanceEdge` trace event variant (additive, skipped as dispatch-metadata during replay)
- [x] 21-inv-C-2             Provenance CLI: `corvid trace dag <id>` renders ProvenanceEdge substream as Graphviz DOT
- [x] 21-inv-D-cli           `corvid replay --mutate <step> <response>` + divergence output
- [x] 21-inv-E-1             Parser: `replay <expr>: when <pat> -> <expr>` syntax
- [x] 21-inv-E-2a            Parser + AST: arm captures (`as <ident>` tail + tool-arg capture)
- [x] 21-inv-E-2b            Resolver: pattern-name resolution + arm-capture scope opening
- [x] 21-inv-E-3             Checker: `TraceId` / `TraceEvent` types + pattern exhaustiveness
- [x] 21-inv-E-4             IR lowering for replay blocks
- [x] 21-inv-G-cli           `corvid test --from-traces <dir>` + trace-to-test harness (5 inventive flags: `--replay-model` / `--only-dangerous` / `--only-prompt` / `--only-tool` / `--since` / `--promote` / `--flake-detect`; coverage-map preview)
- [x] 21-inv-B-cli-wire      Flip `--model` CLI stub to real differential-replay dispatch (driver helper `run_replay_from_source_with_builder` + 6 driver integration tests)
- [x] 21-inv-D-cli-wire      Flip `--mutate` CLI stub to real counterfactual-mutation dispatch (4 driver integration tests)
- [x] 21-inv-G-cli-wire      Flip `--from-traces` CLI stub to real regression-harness dispatch through `corvid_runtime::run_test_from_traces` (async driver variant; deferred `--promote` to follow-up)
- [x] 21-inv-G-cli-wire-promote  Wire `--promote` through `RecordCurrent`: fresh-run-with-`trace_to` driver helper (`run_fresh_from_source_async`) + `PromotePromptMode::AutoStdin` (TTY: [y/N/a/q]; non-TTY: fail-closed with one-time warning)
- [x] 21-inv-H-1             `corvid trace-diff <base-sha> <head-sha> <path>` + in-repo Corvid `@deterministic` reviewer agent: static algebra diff (added / removed agents, trust-tier / `@dangerous` / `@replayable` transitions) across `pub extern "c"` exported surface. Reviewer is a `.cor` program embedded via `include_str!` — the flagship PR-review tool dogfoods the language it reviews.
- [x] 21-inv-H-2             Counterfactual replay: `--traces <dir>` replays each trace against base and head via the 21-inv-G-harness, categorises per-trace verdicts into `passed_both` / `newly_diverged` / `newly_passing` / `diverged_both` / `errored` buckets, and the Corvid reviewer renders a "Counterfactual Replay Impact" section with the newly-divergent path list and an impact percentage. Reviewer signature grows to `review_pr(base, head, impact) -> String` without losing its `@deterministic` guarantee.
- [x] 21-inv-H-3             Structured approval + provenance diff: receipt calls out added / removed approval labels per agent, weakened `required_tier` on existing labels, reversibility regressions, `returns_grounded` transitions, and added / removed `grounded_param_deps`. Reviewer owns the structure in Corvid; Rust only extracts fields. Numeric cost-at-site deltas deferred (blocks on Corvid Float→String). Structured predicate-JSON AST diff deferred (needs typed JSON in Corvid; different language-surface work).
- [x] 21-inv-H-4             Structured narrative summary: `summarise_diff` prompt produces a one-to-three-sentence `ReceiptNarrative { body, citations }` at the top of the receipt, with strict all-or-nothing citation validation against the canonical `DiffSummary.records` key set. `--narrative=auto|on|off` (default `auto`); `off` is the byte-deterministic CI path. Rejected narratives fall back to H-3 boilerplate with a `narrative rejected: <reason>` stderr warning. Receipt structure stays reviewer-owned in `review_pr`, which now takes the validated `ReceiptNarrative` as its fourth argument and remains `@deterministic`.
- [x] 21-inv-H-5             GitHub/CI integration: canonical `Receipt` struct (schema_version 1) is the source of truth; `--format=markdown|github-check|json|auto` routes through per-format renderers (markdown stays Corvid-side via the reviewer agent; github-check + json are Rust). `auto` detects `$GITHUB_ACTIONS` → github-check, piped stdout → json, tty → markdown. Default regression policy ships baked-in (conservative: @dangerous gained, trust lowered, approval tier weakened, reversibility became irreversible, grounded lost, grounded dep removed, newly-diverged traces) with non-zero exit + stderr flag listing on trip. Improvements (additions, tier-raising, grounded gained) do NOT trip the gate.
- [x] 21-inv-H               Rollup CLOSED — H-1 through H-5 landed; `corvid trace-diff` is the flagship PR-review tool dogfooding the language.

**Deferred follow-ups (file separately, each independently shippable):**

- [x] 21-inv-H-4-follow       Upgrade `ReceiptNarrative` to `Grounded<ReceiptNarrative>` now that 22-F ships the provenance-handle path. Rust-side `ReceiptNarrative` is host-minted into `Grounded<_>` from already-validated citation delta keys before the deterministic Corvid reviewer consumes it; empty fallback narratives carry no prose claims and therefore mint an empty chain.
- [x] 21-inv-H-5-custom-policy  Promote the default regression policy from a Rust function to a user-replaceable `.cor` program. `--policy=<path>` flag loads + compiles the user's `apply_policy(receipt) -> Verdict` agent; default policy ships as `default_policy.cor` baked into the CLI. Governance-as-code for the gate itself.
- [x] 21-inv-H-5-signed        DSSE-signed receipts. `corvid trace-diff --sign=<key>` emits a DSSE envelope (`application/vnd.corvid-receipt+json`) with ed25519 signature over the PAE of the canonical JSON payload. `corvid receipt show <hash>` resolves a receipt from the local hash-addressed cache (short prefix matches supported, minimum 8 chars). `corvid receipt verify <envelope> --key <path>` round-trips — accepts file paths OR cached hash-prefixes. Key source: `--sign=<path>` file (hex or raw 32 bytes) with `CORVID_SIGNING_KEY` env var fallback. Receipt hash emitted on stderr as `Corvid-Receipt: <hash>` for downstream tooling. Turns the receipt from informational text into a cryptographic audit artifact — Corvid receipts now plug into the DSSE / Sigstore / in-toto ecosystem.
- [x] 21-inv-H-5-in-toto       SLSA/Sigstore in-toto attestation renderer. `--format=in-toto` emits an in-toto Statement v1 wrapping the canonical Receipt as the predicate; subject is the head source file (sha256); predicateType `https://corvid-lang.org/attestation/receipt/v1`. Combined with `--sign`, the DSSE envelope uses `application/vnd.in-toto+json` so cosign / slsa-verifier consume the output natively. `corvid receipt verify` accepts both Corvid-native and in-toto payloadTypes transparently. Unsigned in-toto output is allowed for pipelines that sign externally (cosign with KMS keys, etc).
- [x] 21-inv-H-5-stacked       Stacked-PR aggregate receipts. Per-commit receipts compose into a stack receipt via the effect-algebra's natural composition; regressions anywhere in the stack surface as regressions in the aggregate.
- [x] 21-inv-H-5-watch         `--format=watch` reactive mode: rebuild + rerender the receipt as the working tree changes. Tightens the AI-safety feedback loop to type-checker speed during local development. Watch mode compares a fixed base SHA against the working-tree file, renders immediately, rerenders on content changes, supports custom Corvid policies, and deliberately rejects stack/signing modes because it is an interactive local feedback loop rather than a durable receipt artifact.
- [x] 21-inv-H-5-gitlab        GitLab CI renderer (`--format=gitlab`). Emits a CodeClimate-compatible JSON array GitLab consumes via `artifacts.reports.codequality`; surfaces findings inline on MR diffs and in the MR widget. One issue per delta; severity tracks the default policy (regressions `major`, informational `info`); the counterfactual-replay trace-impact lands as its own `major` issue when any trace newly diverged. Fingerprint is hex-SHA256 of the delta key — byte-stable across pipeline re-runs so GitLab dedupes issues rather than spawning phantom "new" findings. `--format=auto` under `$GITLAB_CI` auto-selects the renderer; users drop `corvid trace-diff ...` into a job without touching `--format`. Non-zero exit on regression carries through unchanged.
- [x] 21-inv-H-5-schema-fix    Honest delta-key names + schema v2. Rename `agent.approval.tier_weakened:` → `agent.approval.tier_changed:` and `agent.approval.reversibility_weakened:` → `agent.approval.reversibility_changed:` in the delta emitter. Both keys always fired on *any* transition (weakening OR strengthening) — the old names were a naming shortcut from H-1/H-3 that misrepresented half the emissions. Direction lives in the `from->to` suffix; the policy layer still gates only on weakenings via `is_trust_lowering` and `*->irreversible` checks. `RECEIPT_SCHEMA_VERSION` bumps 1 → 2 so JSON consumers pattern-matching on the old prefixes get a clear signal to update their matchers. Pre-slice to `21-inv-H-5-stacked` — algebraic stack composition needs honest names to reason about.

**Language-core slices (cross-lane; enable the custom-policy stack + every future multi-file `.cor` user surface):**

- [x] lang-pub-toplevel        Top-level visibility: `public` / `public(package)` on `type` / `tool` / `prompt` / `agent` declarations. Private-by-default. `pub extern "c"` agents implicitly `Public` (FFI export requires external visibility by definition). Backward-compatible — existing single-file programs behave identically; visibility becomes load-bearing once cross-file imports land in `lang-cor-imports-basic`. Enables intentional library-surface authoring from day one, parallel to the existing `public` support inside `extend` blocks.
- [x] lang-cor-imports-basic-parse   Parser + AST support for `import "./path" as alias`. New `ImportSource::Corvid` variant distinguishes local `.cor` imports from Python FFI imports. Grammar accepts both shapes; resolver is unchanged (qualified access yields "not yet implemented" at resolve time pending `-resolve`). Ships the syntactic contract before the mechanism.
- [x] lang-cor-imports-basic-resolve-2a   `ModuleResolution` / `ResolvedModule` / `DeclExport` types in `corvid-resolve`. Public-export filtering (private declarations never leak). `resolve_import_path` + `ModuleLookup` API. No driver integration yet.
- [x] lang-cor-imports-basic-resolve-2b   Driver-side BFS loader in `corvid-driver::modules` with three-color cycle detection. `build_module_resolution(root_file, root_path) -> (ModuleResolution, Vec<ModuleLoadError>)`. Five typed error variants (FileNotFound / ReadError / LexError / ParseErrors / Cycle). Diamond imports dedupe; transitive imports load but don't surface on root alias map.
- [x] lang-cor-imports-basic-resolve-2c-1 Checker threading + failure-mode errors: `typecheck_with_modules` entry point; optional `ModuleResolution` in `Checker`; three typed errors (`UnknownImportAlias` / `ImportedDeclIsPrivate` / `UnknownImportMember`) surface when import lookup fails. Found case still stubs pending `-2c-2`.
- [x] lang-cor-imports-basic-resolve-2c-2 Real type resolution for successful qualified lookups. Chose `Type::ImportedStruct` to preserve file-boundary identity instead of synthesizing local DefIds. `DeclExport` carries type fields, `ResolvedModule` carries the imported AST, `check_field` resolves imported struct fields, and `Type::display_name` renders imported type names.
- [x] lang-cor-imports-basic-resolve-2c-3 **Owner preference: Dev B.** Driver integration: file-backed production paths now route through module-aware typecheck + lowering when a root file has Corvid imports. Build/run/replay/fresh/shadow paths keep `corvid.toml` config and imported struct identity together; source-string helpers remain single-file by design.
- [x] lang-cor-imports-basic         ROLL-UP — 2c-2 + 2c-3 landed. Basic aliased Corvid imports now parse, load, resolve exported struct types, preserve field access, and compile through file-backed driver paths.
- [x] lang-cor-imports-basic-calls   **Owner preference: Dev B.** Qualified function calls: `p.apply_policy_default(r)`. Implemented checker-side `FieldAccess` recognition for import aliases before method-call fallback, typed imported tool / prompt / agent / struct-constructor calls in the imported module's type context, appended imported callable/type decls to IR with synthetic DefIds, and covered IR + VM runtime dispatch with file-backed driver tests. Needed end-to-end for `21-inv-H-5-custom-policy` to compile + run.
- [x] lang-cor-imports-use     Selective name lift: `import "./path" use Name, Name as Alias` — explicitly-listed names into current scope, no wildcard merge, no silent shadowing. Rename-on-import via `as Alias` for conflicts. Ships on top of `-basic`.
- [x] lang-cor-imports-requires Effect-typed imports — the extraordinary differentiator. `import "./path" requires @deterministic as p` asserts the imported module's public exports satisfy the import boundary contract at compile time. Deterministic imports require exported agents to be `@deterministic` and reject public tool/prompt exports; dimensional constraints such as `@budget($0.50)` run through the existing effect analyzer for exported agents. Prevents "library silently broke our invariants" bugs; composes cleanly with Corvid's existing effect algebra.
- [x] lang-cor-imports-semantic-summaries Imported modules expose effect, approval, provenance, budget, replayability, and exported-agent summaries to the checker and CLI. `ResolvedModule` now carries a stable semantic summary, import contract checks consume that summary, and `corvid import-summary <file> [--json]` renders the imported public boundary for developers.
- [x] lang-cor-imports         ROLL-UP — closes when `-basic` + `-use` + `-requires` all land. Basic aliased imports, selective lifted imports, and effect-typed import requirements are now implemented; semantic summaries, signed imports, remote imports, and versioned packages remain follow-up hardening/publishing layers.
- [x] lang-cor-imports-signed  Hash-pinned imports: `import "./path" hash:sha256:abc123... as p`. If the imported file's content drifts, compilation fails. Supply-chain integrity at the language level: pins are parsed into the AST/IR, verified over the exact imported source bytes before parsing/resolution, and mismatches fail closed with an actionable diagnostic. Pairs with `21-inv-H-5-signed` so a signed receipt's policy hash chain extends through the import graph.
- [x] lang-cor-imports-remote  Remote imports: `import "https://.../policy.cor" hash:sha256:... as p`. HTTP(S) Corvid imports now require mandatory SHA-256 pinning at parse time, fetch through a distinct remote module target, verify exact response bytes before parsing, and typecheck/lower public exports through the same module pipeline as local imports. Enables federated policy baselines and cross-repo governance without a full package manager.
- [x] lang-cor-imports-versioned ROLL-UP — versioned imports + package system: `import "corvid://@anthropic/safety-baseline/v2.3" as p`. Locked package resolution, registry semantic-version selection, and signed publish verification have landed as separate slices so the package-manager story is real rather than implied.
- [x] lang-cor-imports-versioned-lock Locked package imports: `corvid://...` source imports now resolve only through `Corvid.lock`. The lockfile maps the semantic URI to an immutable HTTP(S) source URL and SHA-256 digest; missing lockfiles, missing entries, and hash drift all fail closed before parsing. Inline hashes on package imports are rejected because the lockfile is the reproducibility authority. See [docs/package-imports.md](docs/package-imports.md).
- [x] lang-cor-imports-versioned-registry Registry semantic-version resolver: `corvid add @scope/name@2.3` queries a local or HTTP registry index, chooses the highest matching semantic version, verifies the selected source hash, computes the exported semantic summary, writes `Corvid.lock`, and refuses packages whose exported effects violate `[package-policy]` in `corvid.toml`.
- [x] lang-cor-imports-versioned-signed-publish Signed publish workflow: `corvid package publish` copies source packages into a registry directory, computes SHA-256 and semantic summary, signs the canonical package subject with Ed25519, and updates `index.toml`. `corvid add` verifies signed entries and `[package-policy] require-package-signatures = true` rejects unsigned entries before lockfile mutation.

- [x] 21-docs                Spec [section 14](docs/effects-spec/14-replay.md) (Phase 21 implementation reference) + v1.0 launch demo at [docs/v1.0-demo-script.md](docs/v1.0-demo-script.md) + ROADMAP closeout status below.

**Phase 21 closeout status (as of 2026-04-25).**

Lane A (compiler + CLI + docs) has shipped the primary replay/test/receipt surface and the receipt hardening follow-ups: counterfactual replay, structured approval/provenance drill-down, grounded narrative receipts, custom Corvid policies, DSSE / in-toto signing, stacked receipts, watch mode, GitLab CI output, and schema-v2 delta names. The thesis claim is demonstrable today: `@replayable` compiles only what can be deterministically reproduced, every run writes a trace, `corvid test --from-traces --promote` closes the Jest-snapshot loop, and `corvid trace-diff` produces a PR behavior receipt whose reviewer and policy can themselves be Corvid programs.

Lane B (runtime + codegen + daemon) has shipped interpreter and native recording/replay, runtime counterfactuals, trace-to-test promotion, and the shadow daemon. Native-tier shadow replay parity is now available through `execution_tier = "native"` in the daemon config for native-recorded traces; cross-tier replay remains rejected by design so trace equivalence never hides backend differences.

What's between us and a clean "Phase 21 done" on the ROADMAP:

- Nothing in the Phase 21 checklist. Remaining replay work, if any, is future hardening on top of the shipped surface rather than a Phase 21 blocker.

The determinism-source catalog and the language's treatment of non-reproducible sources are documented in [docs/phase-21-determinism-sources.md](docs/phase-21-determinism-sources.md) and summarised in [spec §14.11](docs/effects-spec/14-replay.md). Every trace axis the runtime records is enumerated there, and extensions land through monotonic `SCHEMA_VERSION` bumps + compile-time opt-in at `@replayable` level.

#### Dev B's lane (runtime + codegen + daemon — ~9 slices)

- [x] 21-B-rec-interp        Recording hooks in interpreter (LLM / tool / approve / seed / time), emit to JSONL
- [x] 21-C-replay-interp     Replay adapter: response substitution; byte-identical post-replay state
- [x] 21-B-rec-native        Native-tier recording parity
- [x] 21-C-replay-native     Native-tier replay parity
- [x] 21-inv-B-adapter       Model-swap seam for `corvid replay --model <id>`
- [x] 21-inv-D-runtime       Counterfactual one-step mutation at runtime
- [x] 21-inv-E-runtime       Runtime support for `replay` language primitive (trace ingestion + pattern dispatch)
- [x] 21-inv-G-harness       Trace-to-test-fixture adapter; divergence-as-test-failure reporting
- [x] 21-inv-I               Live shadow replay daemon; real-time divergence alerts
- [x] 21-inv-I-native        Native-tier shadow replay daemon parity: `execution_tier = "native"` builds/caches the native binary, replays native-recorded traces under the native writer, reads differential/mutation reports, and preserves cross-tier rejection for interpreter-recorded traces.

**Rules (standing):** CLAUDE.md rubric on every file (1–2 responsibilities). One commit per file extraction or feature step. Validation gate between every commit: `cargo check --workspace` + `cargo test -p <crate> --lib` + `cargo test -p <crate> --test <name> -- --list` (for test-file touches) + `cargo run -q -p corvid-cli -- verify --corpus tests/corpus` (must still exit 1 only on the two deliberate fixtures). Push before next slice, then continue to the next roadmap item automatically unless blocked by a real product/security/scope decision. Zero semantic changes mid-refactor. No shortcuts — a thin feature is a shortcut.

**Success criteria.** Every agent marked `@replayable` compiles iff it can be deterministically replayed. Every run under recording produces a JSONL trace that replays to byte-identical state. `corvid replay --model claude-opus-5.0 trace.jsonl` runs cost-free and reports divergences. `replay` is a first-class Corvid expression. Prod traces become regression tests with `corvid test --from-traces`. PRs show a behavior diff before merge. Live shadow mode detects regressions in production.

---

### Phase 22 — C ABI + library mode (~6–8 weeks) ✅ closed

**Goal.** Embed Corvid in Rust, Python, Node, Go hosts — with the AI-safety guarantees (effects, approvals, provenance, budgets) surviving into the host's type system. Corvid isn't just a callable library; it's the only embeddable language whose compile-time AI-safety contracts are observable from the host.

**Hard dep:** Phase 12 (native codegen).
**Soft dep:** Phase 17 (cycle collector). C ABI without the cycle collector means embedders who build cyclic data across the boundary leak — exactly the same behaviour every pre-Phase-17 Corvid program has. Not a compilation blocker, but pairing with Phase 17 at the same release is the honest story: the v0.7 pitch is "Corvid ships as a library" and shipping a leaking library would undercut that.

**Slice checklist:**

- [x] 22-A-cdylib            `pub extern "c"` + `--target=cdylib`/`--target=staticlib` + `--header` scalar C header
- [x] 22-B-abi-descriptor    `--abi-descriptor` + `corvid-abi` crate (machine-readable effect/approval/provenance surface, deterministic JSON)
- [x] 22-C-prompt-catalog    Runtime-queryable typed prompt/agent catalog: cdylibs embed the descriptor, expose `corvid_list_agents` / `corvid_agent_signature` / `corvid_call_agent` so hosts can discover + dispatch agents with type-checked args at runtime
- [x] 22-D-effect-filter     Host-side effect-dimension filter: `corvid_find_agents_where(trust<=autonomous, cost<=0.10)` — the host can narrow the agent set by effect algebra without re-reading the descriptor
- [x] 22-E-approval-bridge   Approval contracts survive FFI: `@dangerous` entrypoints reach back through the boundary to invoke a host-supplied approver; no way for a host to bypass by linking
- [x] 22-F-grounded-return   `Grounded<T>` return values cross the boundary with their provenance chain intact; host receives `(payload, provenance_handle)` it can query
- [x] 22-G-budget-observe    Per-call cost/latency observability: host reads real-time budget burn per agent
- [x] 22-H-replay-across-ffi Traces recorded on one side of the boundary replay deterministically from the other; the embedded binary becomes a recordable unit
- [x] 22-I-host-bindings     Reference Rust + Python host crates; generated idiomatic bindings from the descriptor (Rust traits; Python Protocols)
- [x] 22-J-ownership-check   Compile-time checker on extern signatures (who frees what, who retains what)
- [x] 22-K-cdylib-demo       End-to-end `pub extern "c"` scalar-signature agent shipping as `.so`/`.dll`, plus a matching host-side Rust + Python demo that reads the descriptor and dispatches

**Non-scope:** WASM (Phase 23). Language-level FFI imports of other languages.

### Phase 23 — WASM target (~8–10 weeks) (reopened 2026-04-29 — browser end-to-end CI gap)

**Goal.** Deploy Corvid to browsers and edge runtimes.

**Hard dep:** IR (✅). Parallel codegen backend to Cranelift-native; does not depend on it.

**Scope:**
- New `corvid-codegen-wasm` crate. The scalar foundation emits directly with `wasm-encoder`; host-capability lowering can move to a fuller Cranelift-backed pipeline when prompts/tools/approvals need shared runtime imports.
- `corvid build --target=wasm` emits `.wasm` + an ES module loader + TypeScript types.
- Runtime: the wasm module imports typed host capabilities for LLM calls + tool dispatch + approval UI + replay recording. Each host import carries the same effect/provenance/budget contract as the native runtime boundary.
- **Replay in WASM**: host functions that record tool + prompt + approve calls write to a JS-side trace store compatible with Phase 21's format. `corvid replay <trace>` on a WASM module runs via the same host-function contract, substituting recorded responses. Shared recording format means a trace captured from native can be replayed under WASM and vice versa — a property worth preserving from the start.
- Browser and edge approval calls produce scoped approval tokens in the trace, so user-mediated actions remain auditable across deployment targets.
- wasmtime / wasmer harness tests running the same IR-level programs the native parity harness runs.
- Browser smoke test: a small Corvid program compiled to wasm and loaded in a web page.

**Non-scope:** Wasm-specific optimizations (post-v1.0). Wasm-side cycle collection (wasm's own GC proposal is stabilising; use it once available, fall back to host-delegated collection via exported functions in the interim).

**Slice checklist:**

- [x] 23-A-scalar-wasm       `corvid build --target=wasm` emits a valid standalone `.wasm` module plus ES loader, TypeScript declarations, and a manifest for scalar runtime-free agents. Unsupported prompts/tools/approvals fail loudly until the host-capability ABI exists.
- [x] 23-B-host-abi          Browser/edge scalar host-capability ABI: scalar prompts/tools/approvals lower to typed `corvid:host` imports (`prompt.*`, `tool.*`, `approve.*`), with generated JS `adaptImports(host)`, TypeScript host interfaces, and manifest import entries. Replay recording, strings/structs, and provenance handles remain follow-up slices.
- [x] 23-C-wasm-replay       JS-side trace store in generated loader: `instantiate(host, { trace })` records schema-v2 `schema_header`, `run_started`, `llm_call/result`, `tool_call/result`, `approval_request/decision/response`, and `run_completed` events for scalar host imports. Native/WASM traces now share the event taxonomy; full `corvid replay` execution over WASM modules remains a harness follow-up.
- [x] 23-D-browser-demo      Browser smoke page at `examples/wasm_browser_demo`: one Corvid source compiles to WASM, the generated ES loader is imported by a real page, typed prompt/tool/approval host capabilities are supplied from JS, approval decisions are visible in UI, and the trace panel records schema-v2 run/prompt/approval/tool events from the generated loader.
- [x] 23-E-wasmtime-harness  Wasmtime parity harness for the current WASM-supported native parity subset: generated scalar modules execute under Wasmtime and match the interpreter for arithmetic, branching, and agent calls; scalar prompt/approval/tool imports execute through typed host functions. Unsupported native parity families remain explicit WASM boundary work until strings, structs, lists, and provenance handles land.

**Phase 23 reopened 2026-04-29 — gap-closing slice required:**

- [x] 23-F-browser-ci-headless     Headless-Chromium browser CI shipped: `examples/wasm_browser_demo/test/` carries a Playwright harness (`browser.spec.js`, `playwright.config.js`, `package.json`) that builds the WASM artifacts, serves the demo over a static HTTP server, opens it in headless Chromium, exercises both approve and deny paths against typed prompt/tool/approval host capabilities from JS, and asserts the schema-v2 trace events (`schema_header`, `run_started`, `approval_request`, `approval_decision`, `tool_call`, `tool_result`, `run_completed`) appear in the trace panel. The `phase23-browser-ci` GHA matrix entry runs the harness on every push. Slice fully closes when the first CI run is observed green on `main`; until then the harness is committed and CI is wired.

**Phase 23 next-close criteria:** the ROADMAP-level `[x]` returns only when slice 23-F clears the slice completion gate (CI workflow update, registry rows for `wasm.browser_host_imports_typed` + `wasm.trace_panel_records_schema_v2`, side-by-side `benches/comparisons/wasm_deploy.md` against a comparable Vercel AI SDK browser deployment).

**v0.7 cuts here.** Corvid ships as a library + a wasm module. Real deployment story.

---

### Phase 24 — LSP + IDE (~6–8 weeks) ✅ closed

**Goal.** Editor support worthy of a real GP language. Users need this to write serious Corvid — must land before the moat features are worth using daily.

**Hard dep:** frontend (✅). Types stable enough that LSP doesn't churn when language evolves.

**Scope:**
- `corvid-lsp` crate implementing the Language Server Protocol. Backend-agnostic (same LSP serves native + interpreter + wasm users).
- VS Code extension as the reference client.
- Features: diagnostics (live), hover with inferred types, completion, go-to-def, find-references, rename, inline-documentation.
- AI-native behavior visibility: effect rows, budget/cost trees, groundedness flow, approval boundaries, model routes, replayability, and import trust constraints shown inline where the programmer is making the decision.
- `@budget($)` overruns, ungrounded returns, non-replayable calls, unsafe imports, and approval-boundary violations shown as live diagnostics with the same error codes as the compiler.
- Debugging attach point wired even if debugger UI is post-v1.0 — protocol contract stable.

**Non-scope:** Other editors (vim / emacs / JetBrains) — users can use the LSP via any LSP-compatible client, but official extensions are post-v1.0.

**Slice checklist:**

- [x] 24-A-lsp-diagnostics   Transport-independent LSP analysis core in `corvid-lsp`: open document text compiles through the real driver, compiler diagnostics become `lsp_types::Diagnostic` values with UTF-16 ranges, compiler hints are preserved, and approval-boundary violations surface through the same live diagnostic path as CLI errors.
- [x] 24-B-lsp-server        JSON-RPC/stdin-stdout language server with `initialize`, `shutdown`, `exit`, `textDocument/didOpen`, `textDocument/didChange`, and `textDocument/didSave`; publishes compiler-backed diagnostics through `textDocument/publishDiagnostics` using full-document sync.
- [x] 24-C-hover-types       Hover with compiler-backed inferred expression types plus declaration summaries for agents, tools, prompts, types, and effects. Prompt hovers surface AI-native metadata such as effect rows, calibration, cacheability, strict citations, and model routing mode; tool hovers show dangerous/approval boundaries.
- [x] 24-D-completion        Context-aware completion for keywords, declarations, tools, prompts, approval labels, effect names, and model names. The completion engine is compiler/parser-backed, uses partial source while the user is typing, and keeps approval/effect/model contexts narrow instead of dumping every symbol everywhere.
- [x] 24-E-navigation        Single-file navigation over resolver identity: go-to-definition, find-references, rename edits, and workspace symbol search across open documents. Navigation uses DefId/LocalId bindings, not text search, so local rename does not touch unrelated declarations with the same spelling.
- [x] 24-F-vscode-client     Reference VS Code extension at `extensions/vscode-corvid`: registers `.cor`, starts `corvid-lsp`, wires diagnostics/hover/completion/definition/references/rename/workspace symbols, ships syntax highlighting, language configuration, snippets for AI-native constructs, restart/log commands, and a local verification script.

### Phase 25 — Package manager (~6–8 weeks) (reopened 2026-04-29 — hosted-registry honesty gap)

**Goal.** Users can share Corvid code and AI capabilities with guarantees. Table stakes for any language anyone takes seriously, made Corvid-native by distributing effect, provenance, approval, budget, and replay contracts alongside source.

**Hard dep:** nothing internal. Major external work: registry hosting.

**Scope:**
- `corvid add <pkg>`, `corvid remove`, `corvid update` CLI.
- `Corvid.lock` lockfile with exact resolved versions, content hashes, semantic summaries, and signed publish metadata.
- Registry service: stateless HTTP API + CDN for source package tarballs. Hosting at `registry.corvid.dev`.
- SemVer-based resolution with conflict detection.
- Effect-aware resolution: `corvid add` can warn or fail when a package exceeds a project policy for trust, cost, data, replayability, approvals, or grounded outputs.
- Package pages and CLI metadata expose exported agents/tools/prompts, effect profile, approval boundaries, provenance guarantees, and replay guarantees.
- `corvid.toml` `[dependencies]` section wired through the driver.

**Non-scope:** Private registries (post-v1.0). Binary package distribution (post-v1.0 — all v1.0 packages are source).

**Slice checklist:**

- [x] 25-A-package-import-lockfile    `corvid://...` imports are fail-closed through `Corvid.lock`: missing lockfiles fail, missing entries fail, locked URL bytes are SHA-256 verified before parse/typecheck/lower, and inline hashes are rejected for package imports.
- [x] 25-B-package-add-publish-policy Signed source package publish plus `corvid add`: registry index resolution, semver selection, source hash verification, exported semantic summary extraction, project policy gates, signature verification, and semantic summaries stored in `Corvid.lock`.
- [x] 25-C-manifest-remove-update     `corvid add` updates `corvid.toml [dependencies]`; `corvid remove` edits both manifest and lock; `corvid update` resolves the newest matching version from the manifest requirement/registry or an explicit spec, re-running hash/signature/policy checks before rewriting the lock.
- [x] 25-D-registry-http-contract     Minimal stateless registry API contract with `corvid package verify-registry`: validates index entries, scoped names, semver, canonical package URIs, immutable versioned `.cor` artifact URLs, SHA-256 bytes, artifact UTF-8/source semantic summaries, Ed25519 signatures, duplicate entries, and CDN-style `Cache-Control: ... immutable` headers.
- [x] 25-E-package-metadata-pages     `corvid package metadata` renders compiler-backed package pages from source: scoped package identity, install snippet, optional signature provenance, exported agents/tools/prompts/types/effects, effect profiles, approval boundaries, grounding, replayability, determinism, and cost/violation notes. JSON output gives the same semantic summary to web registries.
- [x] 25-F-conflict-resolution        `corvid package verify-lock` validates the installed package graph: manifest dependencies, locked package presence, duplicate URIs, multiple locked versions for one dependency, semver requirement satisfaction, stale undeclared lock entries, required semantic summaries, and current package-policy compatibility from locked semantic summaries.

**Phase 25 reopened 2026-04-29 — gap-closing slice required:**

- [ ] 25-G-no-hosted-registry-honesty   The current implementation is a *package format + local resolver + signed-publish-to-a-directory*; `registry.corvid.dev` does not exist as a running service. The slice must (a) update `README.md` and the package CLI `--help` output to make the "format-and-tooling, no hosted service yet" boundary explicit, (b) add `docs/package-manager-scope.md` documenting what works locally vs what requires a hosted registry, (c) add a registry entry `package.hosted_registry_available` as `OutOfScope` with reason "no hosted service runs yet; `--url-base` accepts file:// and any http endpoint a user runs themselves," and (d) ensure no website / launch-page copy implies the hosted registry exists. Closes when grep against README + landing page returns zero un-qualified mentions of `registry.corvid.dev`.

**Phase 25 next-close criteria:** the ROADMAP-level `[x]` returns only when slice 25-G clears the slice completion gate.

### Phase 26 — Testing primitives (~4 weeks) ✅ closed

**Goal.** `test`, `mock`, `fixture` as language features. Users can't ship production Corvid without first-class tests.

**Hard dep:** typechecker extension for `test`/`mock` decls.
**Soft dep:** Phase 25 (package manager). Shared fixtures can distribute as packages eventually, but in-repo fixtures work without the package manager — not a blocker.

**Scope:**
- `test name: body` declaration. Discovered automatically; run by `corvid test`.
- `mock tool_name: body` overrides a tool implementation within a test's scope, while preserving or explicitly declaring the mocked effect profile.
- `fixture name: body` for reusable test data; resolved by `corvid test` at run time.
- Snapshot testing primitive — `assert_snapshot expr` writes the first run's value to a file, compares on subsequent runs.
- AI-native assertions over traces, approvals, costs, provenance, grounding, and replay behavior. Ordinary tests verify values; Corvid tests can also verify that the right process happened.
- Trace fixtures: production traces from Phase 21 can be used as deterministic test inputs and regression cases.
- Interop with Phase 20's `eval ... assert ...` syntax (evals are tests, tests aren't necessarily evals — eval is statistical assertions over LLM behaviour).

**Slice checklist:**

- [x] 26-A-test-declarations          `test name:` declarations parse, resolve, typecheck, and lower into `IrTest` nodes. Tests reuse eval assertion syntax so value, trace-called, approval, ordering, cost, and statistical assertion metadata share one compiler model. See [docs/testing-primitives.md](docs/testing-primitives.md).
- [x] 26-B-test-runner                `corvid test <file>` discovers `test` declarations, executes setup bodies, evaluates value assertions, and reports typed pass/fail output with CI exit codes. Statistical value assertions rerun setup for the requested run count; trace/process assertions fail explicitly until 26-E implements trace fixtures.
- [x] 26-C-mocks-fixtures             `fixture` declarations are typed reusable test data callable only from tests/mocks; `mock` declarations are typed overrides for existing tools with exact signature matching. Test execution activates mocks through the VM after the normal approval/confidence gate, so mocked dangerous tools still preserve the target effect profile. See [docs/testing-primitives.md](docs/testing-primitives.md).
- [x] 26-D-snapshots                  `assert_snapshot` evaluates typed runtime values, stores deterministic JSON snapshots under `.corvid-snapshots/<source-stem>/`, reports first-run updates, fails with diff output on mismatches, and supports `corvid test --update-snapshots` plus `CORVID_UPDATE_SNAPSHOTS=1`. See [docs/testing-primitives.md](docs/testing-primitives.md).
- [x] 26-E-trace-fixtures             `test name from_trace "trace.jsonl":` binds schema-validated production traces to language tests. Trace assertions now evaluate against JSONL fixtures: `called`, ordering, approval, and cost checks fail with typed runner output instead of reporting unsupported placeholders. Trace paths resolve relative to the `.cor` file, so production traces can live beside the tests that lock their behavior. See [docs/testing-primitives.md](docs/testing-primitives.md).

### Phase 27 — Eval tooling CLI (~3 weeks) ✅ closed

**Goal.** Turn Phase 20's `eval ... assert ...` syntax into a usable dev + CI workflow.

**Hard dep:** Phase 20 slice 20c (eval syntax — nothing to run without it).
**Soft dep:** Phase 26 (testing primitives). Eval tooling could have its own runner + discovery, but reusing Phase 26's infrastructure avoids duplication; the sequencing here is "ship tests first, build eval on top."

**Status.** Closed. `corvid eval` now runs source eval declarations, writes terminal/HTML/JSON reports, detects prior-result regressions, summarizes trace evidence, compares stored eval runs, enforces planned spend budgets, supports model-swap replay analysis, and runs golden-trace eval suites.

**Scope:**
- [x] `corvid eval <file>` runs all `eval` blocks; produces terminal report + HTML report. Shipped as a reusable driver eval runner plus CLI path that writes `target/eval/<source>/report.html` and preserves `--swap-model` migration analysis.
- [x] Regression detection against prior eval results (stored under `target/eval/`). Shipped via persisted `latest.json` / `previous.json` summaries under each eval output directory, with terminal and HTML surfacing for newly failing evals/assertions.
- [x] CI exit-code contract: non-zero if any `assert` fails or regression threshold crossed. Shipped through eval runner exit codes, compare regression exit codes, and budget preflight failures.
- [x] Trace-aware eval reporting: value pass rates, process assertions, approval assertions, groundedness, cost, latency, model route, and replay compatibility in one report. Shipped by scanning eval JSONL artifacts under `target/eval/<source>/`, validating schema compatibility, and folding trace metrics into terminal + HTML reports.
- [x] Prompt-diff report: when a prompt body changed between runs, show before/after + delta in grounding / cost / assert pass-rates. Shipped in `corvid eval compare` using rendered prompt bodies stored in eval trace summaries, alongside cost, route, and pass-rate deltas.
- [x] Model-swap eval mode uses Phase 21 replay and Phase 20 model metadata to compare provider/model choices without spending on unchanged tool paths. Already shipped as `corvid eval --swap-model <MODEL> --source <FILE> <TRACE_OR_DIR>...`, which delegates trace files to differential replay and trace directories to the trace-suite migration analyzer.
- [x] `corvid eval compare <base>..<head>`: PR-friendly eval diff with pass-rate deltas, cost deltas, latency deltas, model-route changes, prompt diffs, and trace/process assertion changes. Shipped as a CLI compare mode over local result paths/directories or git refs containing `target/eval/**/latest.json` summaries.
- [x] Regression-cause clustering: classify failures by prompt change, model change, tool-output change, route change, approval-path change, grounding loss, or budget regression. Shipped in compare reports with prompt-change, route-change, budget-regression, approval-path, tool/process, and assertion-regression buckets.
- [x] Eval budget mode: estimate and enforce max eval spend before running provider-backed evals; CI fails early when the planned eval run exceeds the configured budget. Shipped as `corvid eval --max-spend <USD>` plus `CORVID_EVAL_MAX_SPEND_USD`, using prior stored eval cost as the pre-run estimate.
- [x] Golden-trace evals: replay production traces against changed prompts/models/tools and score behavior without re-spending unchanged tool and prompt paths. Shipped as `corvid eval --golden-traces <DIR> <source.cor>`, delegating to the trace-suite replay analyzer in non-promoting mode.

**v0.8 cuts here.** Full developer workflow: write in LSP, share via package manager, test + eval in CI.

---

### Phase 28 — HITL expansion (~3 weeks) ✅ closed

**Goal.** `ask`, `choose`, rich approval UI. Completes the human-in-the-loop surface.

**Hard dep:** runtime (✅).

**Scope:**
- [x] `ask(prompt, Type)` — structured input from the human. Returns `Type`. Ties into the approval runtime.
- [x] `choose(options: [T]) -> T` — pick one. UI presents options; user selects.
- [x] Rich `approve` UI: show context (why approval requested), diff preview (what will change), arguments inspection.
- [x] Scoped, replay-verifiable approval tokens: the trace records what the human approved, for which label, arguments, and time window.
- [x] Human-boundary effects: `ask`, `choose`, and `approve` compose into the same effect algebra as tools and prompts, so human interaction is visible to the compiler and host descriptors.
- [x] CLI + web-UI implementations; approval tokens same regardless of UI.
- [x] Approval scopes: one-time, session-scoped, amount-limited, time-limited, and argument-bound tokens. Scope violations fail closed and are replay-visible.
- [x] Typed tool contract recorder: tools can declare domain effects such as `money(amount)`, `external(stripe)`, `irreversible`, and `requires approve "charge-card"`. The compiler/runtime turns those contracts into approval cards, trace events, PR behavior diffs, package metadata, and CI failures when a change introduces a new money-moving or irreversible path.
- [x] Human-readable approval cards generated from typed tool arguments, with schema validation and redaction rules inherited from the effect/privacy profile. First runtime slice shipped as `ApprovalCard` generation from approval labels and JSON argument types, risk inference, sensitive-value redaction, and richer stdin approval rendering.

### Phase 29 — Memory primitives (~4–5 weeks) ✅ closed

**Goal.** `session` and `memory` as typed, SQLite-backed stores. Core to how AI applications handle state.

**Hard dep:** Phase 18 (Result — `session.get()` returns `Result<T, StoreError>`). SQLite (external).

**Scope:**
- [x] Store declaration surface + metadata: `session Name:` and `memory Name:` parse as typed top-level schemas, resolve their field types, register store effect names, and emit ABI store contracts with `reads_*` / `writes_*` metadata.
- [x] Native runtime store backend: `Runtime` exposes replay-visible `store_get` / `store_put` / `store_delete` over a pluggable store manager, with SQLite persistence for native hosts and an in-memory backend for tests/embedding.
- [x] Store policy hooks: `policy <name>: <value>` entries inside `session` / `memory` declarations parse into typed AST metadata and emit through ABI store contracts for retention, privacy, and approval enforcement.
- [x] Provenance-aware store records: runtime stores can persist JSON values together with optional `ProvenanceChain` metadata, preserving grounded lineage for long-lived memory retrieval.
- [x] Revisioned memory conflict detection: runtime stores assign monotonic record revisions and expose compare-and-set writes so stale memory updates fail with `StoreConflict` instead of silently overwriting newer facts.
- [x] Runtime retention policy enforcement: ABI store policies can become runtime `StorePolicySet`s; TTL reads expire stale records, and legal-hold policies block deletion with typed policy errors.
- [x] Approval-required memory writes: runtime store policy APIs gate sensitive writes through the existing approval flow and preserve denial/approval events in replay-visible traces.
- [x] Provenance-required memory reads: store policies can require retrieved records to carry `ProvenanceChain` lineage, failing ungrounded reads with typed policy errors.
- [x] Generated typed store accessor contracts: ABI store metadata now includes compiler-generated `get` / `set` / `delete` accessor signatures for each declared field, carrying field types and read/write effects for codegen and host SDKs.
- `session { ... }` block declares per-conversation state. Compiler generates typed accessors.
- `memory { ... }` block declares long-lived state (survives process restarts).
- Both backed by SQLite (native) and IndexedDB (wasm).
- Effect-tagged: `reads_session` / `writes_session` / `reads_memory` / `writes_memory`. Integrate with Phase 20's effect rows.
- Provenance-aware memory: stored values may carry `Grounded<T>` lineage, and retrieval from memory can preserve or require provenance.
- Policy hooks for privacy, retention, and approval-required writes, so agent memory is governed state rather than an untyped vector store.
- Retention and deletion policy: declare TTL, user-delete, legal-hold, and privacy-tier rules at the `session` / `memory` block; runtime enforces them consistently across native and WASM storage.
- Memory conflict resolution: typed handling for stale facts, contradictory facts, and source-priority rules, with conflicts surfaced as `Result`/diagnostics rather than silently overwriting state.
- Memory write approvals for sensitive or irreversible state changes, recorded in replay and visible in effect summaries.

**Phase 29 follow-up audit (2026-04-29) — epistemic verification:**

- [x] 29-K-memory-module-audit-doc       `docs/phase-29-memory-audit.md` ships, enumerating every memory primitive against the ROADMAP claims with source file + line range + positive + adversarial tests for each surface. Audit confirmed native-tier coverage; identified one cross-tier gap (wasm IndexedDB backing) that promotes into slice 29-L below.
- [ ] 29-L-wasm-indexeddb-host-import    Add `corvid:host store.*` import bindings for IndexedDB on the JS side; the wasm-codegen ES loader generates the typed wrapper. Closes when `examples/wasm_browser_demo` persists across page reloads through IndexedDB and a Phase 23 browser CI test (slice 23-F) verifies persistence. The native-tier `[x]` of Phase 29 stands; this slice promotes Phase 29's WASM-tier claim from aspirational scope-line to shipped runtime.

### Phase 30 — Python FFI via PyO3 (~5–6 weeks) (reopened 2026-04-29 — default-CI gap)

**Goal.** `import python "..."` works in compiled code. Closes the "but Python has the ecosystem" gap.

**Hard dep:** Phase 13 (async — PyO3's GIL-aware runtime needs async context).
**Soft dep:** Phase 20 slice 20a (effect rows). Python imports declare effects at the import site — the basic `effects: network` / `effects: unsafe` syntax works against the existing `safe` / `dangerous` split; richer user-declared effects via 20a's effect rows make the story better but aren't a compilation blocker.

**Scope:**
- [x] Python import effect declarations: parser accepts `import python "..." as name effects: ...`; the type checker rejects untagged Python imports, while `effects: unsafe` is allowed but flagged for review.
- [x] Runtime PyO3 call bridge: feature-gated runtime support can import Python modules, call functions, marshal JSON-like scalars/lists/dicts, and return Python exceptions with formatted traceback text.
- [x] Trace-visible Python calls: runtime Python FFI calls emit `python.call`, `python.result`, and `python.error` host events so Python ecosystem use is visible to audits.
- [x] Python sandbox capability profiles: feature-gated runtime Python calls can be checked against declared effects and deny obvious network/filesystem/subprocess/environment modules before import.
- PyO3 integration in `corvid-runtime`. Lazy CPython load.
- `import python "requests" as requests effects: network` — untagged imports rejected by the effect checker. `effects: unsafe` is the opt-in escape hatch and is flagged for review.
- Error marshalling: Python exceptions become Corvid `Result::Err` with preserved traceback.
- Type marshalling: Python dicts ↔ Corvid structs (when schema known), lists ↔ lists, scalars ↔ scalars.
- Python calls appear in traces, audit output, and effect summaries, so the Python ecosystem does not become an invisible safety hole inside AI workflows.
- Interpreter tier gets the same FFI surface so both tiers behave identically.
- Optional sandbox profiles for Python imports: network, filesystem, subprocess, environment, and native-extension access are denied unless declared in the import's effect profile.
- Generated typed wrappers from Python signatures and docstring/schema metadata where available; unresolved dynamic shapes require explicit Corvid type annotations.
- Python FFI contract tests: verify exception marshalling, type conversion, trace recording, and effect summaries for imported Python functions.

**Phase 30 reopened 2026-04-29 — gap-closing slice required:**

- [ ] 30-J-default-ci-pyo3        The Python FFI integration tests run only behind the optional `python` feature flag; default `cargo test --workspace` does not exercise the pyo3 runtime path. A regression in the call bridge, the GIL handling, the type marshaling, or the trace event emission would not be caught by CI on every push. The slice (a) adds a CI matrix entry that runs `cargo test -p corvid-runtime --features python --tests` against a pinned CPython, (b) makes the Python integration tests assert at minimum: scalar round-trip, list round-trip, dict↔struct round-trip, exception marshalling preserves traceback, `python.call`/`python.result`/`python.error` events fire, and a sandbox-profile-denied import is rejected, (c) documents the matrix entry in `docs/ci.md`. Closes when the GitHub Actions workflow shows the python-feature job green on `main`.

**Phase 30 next-close criteria:** the ROADMAP-level `[x]` returns only when slice 30-J clears the slice completion gate.

### Phase 31 — Multi-provider LLM adapters (~2 weeks) ✅ closed

**Goal.** Provider coverage for the AI application surface, not just chat completion: hosted frontier models, local models, OpenAI-compatible gateways, structured-output providers, routing metadata, and adapters users actually request.

**Hard dep:** runtime adapter trait (✅).

**Scope:**
- [x] Provider capability metadata: `corvid.toml` model entries can declare provider, privacy tier, jurisdiction, context window, structured-output/tool/embedding support, multimodal tags, latency tier, and task capability tags for routing and audit surfaces.
- [x] Provider health and automatic failover: runtime tracks adapter health, records provider degradation/failover trace events, and routes failed live calls to cheapest compatible cross-provider catalog fallbacks that preserve capability, format, privacy, jurisdiction, context, tool, embedding, multimodal, and task contracts.
- [x] Cost normalization and usage accounting: runtime records normalized USD/token usage per LLM call with provider, adapter, privacy tier, local-vs-hosted, prompt/completion/total tokens, trace `llm.usage` events, and provider-level totals for routing and budget reports.
- [x] Capability contract tests: runtime can run configured-model contract probes for structured JSON output, provider token usage, context-window declarations, and explicit unsupported statuses for tool-call/streaming probes until the adapter surface exposes native checks.
- `GoogleAdapter` in `corvid-runtime`. API compatibility with existing AnthropicAdapter + OpenAiAdapter surface.
- `OllamaAdapter` for local-first Corvid.
- Provider/model metadata includes cost, latency, privacy tier, jurisdiction, structured-output support, context window, tool-calling support, embedding support, multimodal capability tags where available, and task capability tags.
- Provider selection via `CORVID_MODEL` env var remains supported, but compiler/runtime model routing can use declared model capabilities from Phase 20h.
- Eval data can compare providers and feed routing reports, so model choice becomes measurable infrastructure rather than string configuration.
- Provider health checks and automatic failover: runtime records provider outage/degradation events and routes to compatible fallback models when policy allows.
- Capability contract tests: verify whether each configured model actually respects JSON mode, tool calls, streaming, context-window claims, and structured-output constraints.
- Cost normalization and usage accounting across providers, including local/openai-compatible servers, so budgets compare real prompt/model choices instead of raw provider strings.

### Phase 32 — Standard library (~8 weeks) ✅ closed

**Goal.** Batteries included for general programming and AI-native applications. Common patterns available without a package install.

**Hard dep:** everything language-core stable.

**Scope:**
- [x] `std.ai` foundation: repo-native Corvid source module with typed messages, sessions, tool-result envelopes, model-route envelopes, structured-output validation envelopes, confidence helpers, trace event summaries, docs, and import/compile coverage.
- [x] `std.http` foundation: typed Corvid request/response envelopes plus runtime HTTP client with timeout, retry-on-5xx, response metadata, and trace events for request/response/error accounting.
- [x] `std.io` foundation: typed Corvid path/file/directory envelopes plus runtime text read, text write, and directory listing APIs with byte, latency, entry-count, and error trace events.
- [x] `std.secrets` foundation: typed Corvid secret-read envelopes plus runtime environment secret access that returns values to callers while emitting only redacted audit trace metadata.
- [x] `std.observe` foundation: typed Corvid observability envelopes plus runtime observation snapshots that aggregate LLM usage, cost totals, local-call counts, provider health, and degraded-provider counts into trace-visible summaries.
- [x] `std.cache` foundation: typed Corvid cache-key/cache-entry envelopes plus deterministic runtime cache-key construction over namespace, subject, model, args, effect key, provenance key, and version metadata with trace-visible key events.
- [x] `std.queue` foundation: typed Corvid background-job envelopes plus runtime enqueue/cancel APIs carrying retry, budget, effect-summary, and replay-key metadata through trace-visible queue events.
- [x] `std.agent` foundation: pure Corvid workflow envelopes and helpers for classification, extraction, ranking, adjudication, planning, tool-use, approval labels, critique, and grounded answers.
- [x] `std.rag` foundation: typed Corvid document/chunk/embedder envelopes plus runtime document construction, markdown loading, deterministic chunking, per-chunk provenance keys, SQLite-backed chunk indexing, and OpenAI/Ollama embedder configuration metadata.
- [x] `std.effects` foundation: shared Corvid effect metadata envelopes for effect tags, budgets, provenance keys, approval labels, cache keys, and replay keys across `std.*`.
- [x] `std.ai` reusable AI application primitives: typed message/session objects, prompt rendering helpers, model-route helpers, tool-result envelopes, structured-output validation, confidence helpers, and trace/event utilities.
- [x] `std.rag` embedder trait with reference OpenAI + Ollama implementations.
- [x] `std.rag` remaining runtime pieces as one `std.ai` subdomain: SQLite-backed embedding retrieval, chunking polish, and tighter grounding-by-construction APIs. Shipped with configurable chunking, SQLite persisted embedding vectors, cosine-similarity retrieval, and runtime `GroundedValue<T>` helpers for retrieval-backed chunk results. Pairs with Phase 20's grounding-contract language half.
- [x] `std.rag` APIs return grounded runtime values by construction where retrieval provenance exists, but grounding is not limited to RAG; any tool/effect that proves provenance can produce grounded values through the shared runtime provenance envelope.
- [x] `std.http` typed HTTP client with effect tags, retry semantics, timeout/budget accounting, and recorded replay-hook exchanges.
- [x] `std.io` path helpers in the runtime: join, parent, filename, extension, extension replacement, and lexical normalization.
- [x] `std.io` remaining runtime pieces: explicit filesystem-effect plumbing through effect-tagged read/write/list/stream runtime envelopes and helpers.
- [x] `std.agent` common AI patterns: classification, extraction, summarization, ranking, adjudication, routing, planning, tool-use loops, approval-gated action, review/critique, and grounded answer generation.
- [x] Everything in `std.*` effect-tagged so users get the moat's benefits from day one.
- [x] `std.queue` durable background jobs for long-running AI tasks, with retry, cancellation, replay hooks, budget accounting, and effect summaries.
- [x] `std.cache` prompt/model/tool-result caching with replay-safe invalidation, provenance preservation, and effect-aware cache keys.
- [x] `std.secrets` explicit secret access APIs with redacted audit metadata surfaces that avoid leaking secret values.
- [x] `std.observe` metrics, trace counters, cost counters, latency histograms, routing decisions, and approval summaries exposed through one typed observability surface.

**Phase 32 follow-up audit (2026-04-29) — per-module verification:**

- [x] 32-T-stdlib-effect-tag-audit-doc    `docs/phase-32-stdlib-audit.md` ships, covering all 11 modules (`ai`, `http`, `io`, `secrets`, `observe`, `cache`, `queue`, `jobs`, `agent`, `rag`, `effects`, `db`). Each row lists module path, public surface, effect tags, runtime backing (where applicable), compile test ref, imported-helpers typecheck ref, and adversarial coverage where present. Audit confirmed full coverage; identified one expansion opportunity that promotes into slice 32-U below.
- [ ] 32-U-stdlib-adversarial-expansion   Today only `std.db` carries a per-surface adversarial test (`std_db_token_surface_does_not_expose_raw_token_values`). Add named adversarial tests for the other modules: `std.secrets` (value-leak attempt rejected through trace), `std.io` (write-without-filesystem-effect rejected at typecheck), `std.http` (network call without network-effect rejected at typecheck), `std.cache` (cache-key construction without effect-key fails the deterministic-key contract), and one per remaining module. Closes when every `std.*` module has at least one adversarial test alongside its compile + imported-helpers test.

**v0.9 cuts here.** Language feature-complete: HITL, memory, Python FFI, multi-provider LLMs, stdlib. Only polish remaining.

---

### Phase 33 — Polish for launch (~6–10 weeks)

**Goal.** v1.0. Stable, documented, installable by a stranger on any OS.

**Hard dep:** everything.

**Scope:**
- [x] In-repo installer flow: checked-in Unix + PowerShell install scripts under `install/` plus the documented `cargo install` path.
- [x] Documentation rewrite foundation: launch reference, tutorial, cookbook, and migration-from-Python docs checked into `docs/`.
- [x] Claim audit foundation: `docs/launch-claim-audit.md` links launch claims to concrete commands and committed artifacts, and keeps external-only claims explicitly blocked until the artifact exists.
- [x] `corvid audit`: project-level static report for approval boundaries, replay coverage gaps, budget exposure, secret-bearing effects, money-moving paths, grounding signals, and semantic-effect violations.
- [x] Stability contract foundation: checked-in launch contract for syntax, type system, CLI, stdlib, and benchmark-claim semantics.
- [x] `corvid doctor` launch checks: provider keys, local-model tooling, replay storage, approval configuration, wasm/native toolchains, registry lock presence, and platform prerequisites.
- [x] `corvid bench compare python|js`: published-archive comparison command over committed benchmark sessions with explicit ratio semantics and no hidden model-latency claim inflation.
- [x] Reproducibility-script foundation: checked-in scripts for benchmark and bundle claim reproduction from committed archives and examples.
- Stability guarantees on the language surface: documented SemVer contract for syntax, type system, stdlib.
- Windows + Linux + macOS all first-class (`corvid doctor` passes, installer works, parity harness green on all three).
- Installer: `curl -fsSL corvid.dev/install.sh | sh` on Unix, PowerShell equivalent on Windows. Corresponding `cargo install` flow.
- Website: landing page, live playground (runs the wasm target from Phase 23), docs site, blog, benchmarks page.
- Documentation rewrite: reference, tutorial, cookbook, migration-from-Python guide.
- Claim audit: every launch claim about effects, approvals, grounding, budgets, replay, evals, packages, WASM, and benchmarks links to a runnable command, test, or committed example.
- `corvid doctor` checks provider keys, local model availability, replay storage, approval UI configuration, wasm/native toolchains, registry access, and platform support.
- `corvid bench compare python|js`: honest orchestration-overhead comparisons against representative Python/JS AI framework stacks. Claims distinguish model-provider latency from Corvid's compiled orchestration/runtime overhead.
- `corvid audit`: project-level report for dangerous tools, approval boundaries, money-moving paths, budget exposure, ungrounded outputs, provider policy violations, secret access, and replay coverage.
- One-command reference apps: RAG app, support bot, approval-gated refund bot, code-review agent, provider-routing demo, and local-model demo. Each ships with tests, evals, traces, and benchmark notes.
- Reproducibility scripts for benchmark and bundle claims, including the Phase 17 performance baseline and Phase 22 public bundle verification.
- Launch materials: 2-minute GIF/video showing the time-travel replay moment + effect-checker catching a bug + compile-time cost budget. HN + Reddit + ProductHunt announcement drafts reviewed with 3 external readers.
- Beta round: 20 external developers build something real in Corvid; their feedback gates the final cut.

**Slice checklist:**

- [x] 33A-installer-foundation       Unix and PowerShell install scripts plus documented `cargo install` path are checked in.
- [x] 33B-docs-foundation            Launch reference, tutorial, cookbook, and migration-from-Python docs exist in `docs/`.
- [x] 33C-claim-audit-foundation     Launch claims are linked to runnable commands or committed artifacts.
- [x] 33D-audit-command              `corvid audit` reports approval, replay, budget, secret, money-moving, grounding, and semantic-effect risks.
- [x] 33E-stability-contract         Syntax, type system, CLI, stdlib, and benchmark claim stability policy is documented.
- [x] 33F-doctor-launch-checks       `corvid doctor` checks provider keys, local models, replay storage, approvals, wasm/native toolchains, registry lock, and platform prerequisites.
- [x] 33G-benchmark-compare          `corvid bench compare python|js` uses committed benchmark archives and separates model latency from orchestration overhead.
- [x] 33H-repro-scripts              Benchmark and bundle claim reproduction scripts are checked in.
- [ ] 33I-platform-parity            Windows, Linux, and macOS installer/doctor/parity harness are green.
- [ ] 33J-website-playground         Website, docs site, benchmark page, blog shell, and WASM playground are live from committed assets.
- [ ] 33K-reference-demo-pack        One-command demo apps have tests, evals, traces, and benchmark notes.
- [ ] 33L-launch-materials           GIF/video, launch drafts, and external-reader review are complete.
- [ ] 33M-beta-feedback              20 external-developer feedback items are closed as code/docs/tests or explicit non-scope.
- [ ] 33N-moat-benchmarks            `benches/moat/` ships two defensibility benchmarks the website can quote: (a) compile-time rejection rate over a target 50-case bug-class corpus comparing Corvid against `mypy --strict + pydantic` and `tsc --strict + zod`; (b) governance line-count over a shared reference-app corpus implemented identically across Corvid + Python + TypeScript. Each benchmark has a runner that produces a deterministic Markdown `RESULTS.md`; CI runs the runners on every push and a drift gate fails the build if `RESULTS.md` no longer matches what the runner produces. Slice scaffold + 3 seed cases + first refund_bot governance comparison committed 2026-04-29 (Corvid saves 22 lines vs Python, 34 vs TypeScript on refund_bot); slice fully closes when all 50 compile-time-rejection cases land + at least 3 reference apps cover governance-line counts.

### Phase 34 — Inventions readme + landing page (~2 weeks) ✅ closed

**Goal.** Every Corvid invention documented in one place, visible from the repo's front door. The README and landing page must answer: "what does this language do that no other language does?" — in code, not in prose.

**Hard dep:** everything. This is the final writing pass before launch. Every feature referenced must be shipped and runnable.

**Why this phase exists.** Phase 33 ships v1.0 with documentation (reference, tutorial, cookbook, migration guide). Phase 34 adds a **dedicated inventions catalog** — a single authoritative document listing every feature Corvid has that no other language has, with runnable examples for each. This is the artifact developers link to, cite on HN, and scan before deciding to try Corvid. Without it, the inventions are buried across Phase 20 slices, the eval docs, the streaming spec, the typed model substrate spec, and the replay flagship docs.

**Scope:**

- [x] Rewrite the repo root `README.md` with the full inventions catalog up top, above the install instructions. Every entry has a 2-line pitch + code example + link to spec.
- [x] Category structure matching the moat: **Safety at compile time** (approve gates, dimensional effects, Grounded<T>, @min_confidence, @budget), **AI-native ergonomics** (agent/tool/prompt/approve/effect/model keywords, evals with trace assertions, replay), **Adaptive routing** (20h model substrate — capability routing, content-aware dispatch, progressive refinement, ensemble voting, adversarial validation, jurisdiction/compliance, privacy tiers, cost-frontier exploration), **Streaming** (20f — live cost termination, per-element provenance, mid-stream escalation, progressive structured types, resumption tokens, fan-out/fan-in), **Verification** (20g — cross-tier differential verification, LLM-driven adversarial bypass generation, executable interactive spec, preserved-semantics fuzzing, bounty-fed regression corpus).
- [x] Landing page rewrite (`docs/site/`): every invention gets a runnable playground example. "Corvid is faster than Python at X" / "safer than TypeScript at Y" claims are supported with side-by-side comparisons that actually run.
- [x] Runnable invention index: `corvid tour --topic <name>` CLI command opens the REPL pre-loaded with compiler-checked demos; `corvid tour --list` shows the shipped catalog across safety, AI-native ergonomics, adaptive routing, streaming, and verification.
- [x] Cross-references: each invention in the README links to (a) the roadmap slice that shipped it, (b) the spec section that formalizes it, (c) the example in the tour, (d) the test that validates it.
- [x] Headline inventions page (`docs/inventions.md`): the standalone artifact HN threads link to. No install prerequisite, no build system context — just the inventions, their syntax, and why each is unique.
- [x] Invention proof matrix: every catalog entry has columns for shipped status, runnable command, test coverage, docs/spec link, and explicit non-scope.
- [x] Update `CLAUDE.md` (or equivalent contributor doc) to require that every new invention ships with a README catalog entry + tour demo.

**Non-scope:** marketing copy, video scripts, social-media assets — those belong to Phase 33's launch materials. Phase 34 is the authoritative technical catalog; Phase 33 is the launch campaign that points to it.

**Defensibility gate.** Phase 34 closes the inventions catalog. Phase 35 closes the *defensible-core* surface that the catalog rests on. Public launch is gated on Phase 35 plus the production-backend market track below; Corvid does not go online as a language for real AI applications until it can build and operate a full backend product itself.

### Phase 35 — Defensible core (~6–8 weeks)

**Goal.** Make Corvid's launch claim defensible under hostile public scrutiny. Every public guarantee is enumerated in a machine-readable manifest, every guarantee is backed by adversarial tests, the ABI surface is bilaterally verified, and the launch wording is derivable from shipped artifacts rather than aspirational. After Phase 35, an outside reviewer can answer "what does Corvid guarantee, what is checked statically, what is checked at runtime, what is out of scope, and how do I verify each independently?" in under ten minutes by running committed commands.

**Hard dep:** every prior closed phase, especially Phase 22 (C ABI) and the signed-attestation moat extension shipped after Phase 34. Phase 35 is the defensibility gate — Phase 33's remaining unchecked items must reference Phase 35 artifacts (claim audit, stability contract, audit command) rather than ship parallel to them.

**Why this phase exists.** External review on the path to public launch identified that while Corvid's *implementation* is real (compiler, runtime, tests, attestation), the *publicly defensible core story* is thinner than the implementation. Five concrete gaps:

1. **Semantic contract is not crisply enumerated.** What is static-checked vs runtime-checked vs out-of-scope is implicit in the test suite. An outsider cannot answer it without reading the codebase.
2. **Proof lives in tests, not in a concise core spec.** The repo has thousands of assertions; outsiders need a single readable spec that ties every public claim to a named test.
3. **Trusted computing base is broad.** Parser, resolver, typechecker, IR lowering, codegen, runtime, ABI emit, and CLI all participate in the same trust boundary. A bug anywhere voids the launch claim.
4. **Launch wording risks getting ahead of formal proof.** Phrasings such as "AI safety contracts are proven" need narrowing to behaviour the compiler actually enforces and that an external party can verify locally.
5. **Adversarial coverage is thin.** Far more positive tests than must-fail tests for approval bypass, descriptor forgery, effect under-reporting, replay tampering, and import-boundary attacks.

This phase closes all five end-to-end with no shortcuts: a guarantee manifest tagged in the compiler, doc generation from the manifest, a property-based fuzz corpus, an independent bilateral verifier, a sign-refusal contract, and a `corvid claim --explain` provenance command.

**Slice checklist:**

- [x] 35-A-registry             `corvid-guarantees` crate: `GuaranteeKind` / `GuaranteeClass` (Static / RuntimeChecked / OutOfScope) / `Phase` enums + canonical `GUARANTEE_REGISTRY` static array. Every public Corvid guarantee enumerated with id, class, enforcing pipeline phase, description, and required test references.
- [x] 35-B-diag-tagging          Every contract-enforcing diagnostic in resolve / typecheck / IR-lower / codegen / runtime carries its `guarantee_id`. Build-time lint rejects untagged contract diagnostics. No contract enforcement is anonymous.
- [x] 35-C-contract-list         `corvid contract list` CLI subcommand emits the canonical guarantee table as JSON or human-readable. Single source of truth — every later artifact derives from this command's output.
- [x] 35-D-spec-generation       `xtask` regenerates `docs/core-semantics.md` from `GUARANTEE_REGISTRY`; CI fails on drift between committed doc and generated. Spec ≡ implementation, automatically. No hand-edited semantics page.
- [x] 35-E-test-cross-refs       Every Static guarantee carries `positive_test_refs` and `adversarial_test_refs`; build-time check rejects empty adversarial coverage on a Static guarantee. Every guarantee in the registry must point to real test functions that compile and run.
- [x] 35-F-fuzz-abi              Adversarial fuzz corpus over the ABI surface: `proptest`-driven byte mutators on descriptor JSON and DSSE attestation envelopes (corrupt signatures, swap payload types, mutate PAE bytes, drop required fields, inject extra symbols). ≥100 mutants per gate; each must be rejected with the documented exit code; benign mutations must round-trip.
- [x] 35-G-fuzz-source           Adversarial fuzz corpus over source-level bypasses: AST mutators for `@approve` re-export bypass, effect under-reporting at module boundary, `Grounded<T>` provenance loss across function calls, import-aliasing of dangerous tools. Each mutated source must fail typecheck with the diagnostic tagged to the right `guarantee_id` from slice 35-B.
- [x] 35-H-bilateral-verifier    Independent ABI verifier (`corvid-abi-verify` binary): parses source independently of the main pipeline, runs only the descriptor-relevant subset of resolution + typecheck, builds an independent `AbiDescriptor`, and bit-compares against the embedded one in a built cdylib. Disagreement = build rejection. Shrinks TCB to "agreement between two implementations." Separate binary, separate code path, separate review surface.
- [x] 35-I-claim-explain         `corvid claim --explain <cdylib>`: emits a self-contained provenance statement listing every guarantee enforced for the given binary, by id and class, plus the signing key fingerprint and verifier-agreement attestation from slice 35-H. The artifact HN threads can quote without further context.
- [x] 35-J-sign-refusal          `corvid build --sign` refuses to emit a signed cdylib unless every declared contract in the source maps to a `GUARANTEE_REGISTRY` entry that was actually checked in this build. No silent skips, no "we didn't run that pass on this target" downgrades. The signed artifact carries the *enforced* claim, not the *intended* claim.
- [x] 35-K-security-model        `docs/security-model.md`: TCB diagram (compiler + verifier + runtime + signer + ABI surface), threat model (insider/outsider, what each defends against), explicit non-goals (compromised host kernel, signing-key compromise, compiler-toolchain compromise). References slice 35-H/I/J behaviours; does not over-claim.
- [x] 35-L-readme-alignment      Replace any aspirational launch wording with claims derivable from `corvid claim --explain`, the adversarial corpus, and the bilateral verifier. README and landing page point at runnable commands; the wording is the *output* of the artifacts, not a separate prose layer.
- [x] 35-M-ci-gate               CI workflow runs the fuzz corpus + bilateral verifier + spec drift check on every push. Phase 35 artifacts are continuously enforced, not point-in-time at launch.

**Non-scope:**

- Formal mechanized proof of the type system (post-v1.0 research; the core-semantics manifest is the v1.0 surface).
- Proof of cryptographic primitives — we use ed25519, SHA-256, and DSSE as standardized primitives, not redesigns.
- Defense against compiler-toolchain compromise (we trust rustc and Cranelift; reproducible builds are a post-v1.0 hardening).
- Defense against signing-key compromise — key management is a host responsibility, not a Corvid responsibility, and `docs/security-model.md` says so explicitly.
- Bug-bounty program, third-party audit contract, formal launch comms — those belong to the final market-launch phase, not to Phase 35.

**Defensible-core cut here.** Phase 35 proves the language's claims. The next phases prove Corvid is a complete backend language for production AI applications, not just a compiler with excellent AI-safety primitives.

### Production slice standard

Every Phase 36-43 slice must clear the same four gates before it can be marked done. This is how the production-backend track stays inventive instead of becoming a long checklist of ordinary web-framework features.

1. **Developer pain removed.** The slice must name the concrete pain it removes for production AI developers: glue code, duplicated policy checks, invisible cost, missing replay, unsafe tool calls, connector OAuth work, weak traces, hand-written audit logs, migration drift, deployment guesswork, or benchmark uncertainty.
2. **AI-native invention.** The slice must add or preserve at least one Corvid-specific AI primitive through the layer it touches: effects, approvals, budgets, provenance, replay, confidence, model routing, evals, trace assertions, signed claims, or guarantee IDs. A generic backend feature without one of these is not enough.
3. **Benchmark or proof.** The slice must ship a measurable artifact: benchmark, adversarial test, golden trace, route test, migration drift test, replay fixture, connector mock, operator command, or reference-app proof. If Corvid cannot beat a mature language/framework on raw speed, the benchmark must show the dimension Corvid wins: fewer unsafe lines, fewer moving parts, compile-time rejection, replayability, audit completeness, or operational time-to-answer.
4. **AI usage in development.** The slice brief must include at least one AI-assisted maintainer workflow that Corvid itself enables or will enable: generating tests from traces, turning production runs into evals, explaining failed guarantees, producing approval summaries, suggesting migrations, summarizing incidents, or creating connector mocks. AI is part of the developer workflow, not only the user's application.

**Benchmark posture.** Corvid should not claim to beat Go, Rust, Java, Node, or Python frameworks on every raw throughput benchmark. The intended win is broader and more relevant to AI backends:

- **Against Python/LangChain-style stacks:** fewer host-language layers, stronger static contracts, faster non-model orchestration, replay/eval/approval built into the language rather than scattered libraries.
- **Against TypeScript/Node agent stacks:** stronger compile-time effect and approval boundaries, native binary deployment, explicit cost/provenance/replay contracts, lower operational ambiguity.
- **Against Go/Rust backend stacks:** less handwritten AI governance code, first-class model/tool/approval/eval semantics, signed AI-safety claims, and faster development of auditable agent backends.
- **Against workflow engines:** richer language-level typing and AI contracts while retaining durable jobs, replay, approvals, and operator controls.

Each benchmark must separate model-provider latency from Corvid runtime overhead. Hiding LLM latency inside benchmark wins is forbidden.

### Slice completion gate (no shortcuts)

Every Phase 36–43 slice — and every retroactive promotion of a Phase 35 entry from `OutOfScope` to `Static`/`RuntimeChecked` — must clear every box below before the `[x]` lands. Optimistic checkmarks are how earlier phases drifted from spec; this gate exists so the same drift cannot reach the production-backend track.

Maintainers paste the filled-in checklist into the dev-log entry that documents the slice. A slice without a green checklist is not done — period.

**Build + test gates**

- [ ] **Workspace clean.** `cargo check --workspace` and `cargo check --workspace --tests` produce zero warnings (no new `#[allow(dead_code)]` / `#[allow(unused)]` without an inline justification comment).
- [ ] **Unit tests green.** `cargo test -p <affected-crate> --lib` is green; the new code has both positive and adversarial unit tests.
- [ ] **Integration tests green.** Every new CLI subcommand, runtime path, or connector ships with at least one `tests/` integration test that exercises the user-facing flow end-to-end.
- [ ] **Corpus + differential-verify green.** `cargo run -q -p corvid-cli -- verify --corpus tests/corpus` exits with the documented code (1 only on the deliberate-fail fixtures).
- [ ] **CI workflow updated.** `.github/workflows/ci.yml` runs the new tests on every push.

**Registry + claim gates (Phase 35 inheritance)**

- [ ] **Registry entry.** Every new public guarantee has a `corvid_guarantees::GUARANTEE_REGISTRY` row with stable `id`, `kind`, `class` (`Static` / `RuntimeChecked` / `OutOfScope`), enforcing `Phase`, description, and — for `OutOfScope` — an explicit reason.
- [ ] **Diagnostic tagged.** Every new contract-enforcing diagnostic uses `TypeError::with_guarantee` (or the equivalent for runtime / ABI / connector errors) with a registered id; no anonymous contract diagnostics.
- [ ] **Test references populated.** New `Static` and `RuntimeChecked` rows carry ≥1 `positive_test_refs` and ≥1 `adversarial_test_refs` that resolve to real `fn name(` declarations. The cross-reference test in `corvid-guarantees` stays green.
- [ ] **Claim coverage updated.** Every new declared contract pattern (new attribute, new keyword, new effect dimension, new connector method, new approval clause) is added to `validate_signed_claim_coverage` so `corvid build --sign` cannot ship an incomplete claim.
- [ ] **`corvid claim --explain` reports it.** Any new public guarantee or contract surface shows up in `claim --explain` output for an exemplar binary.
- [ ] **`corvid contract list` shows it.** Same for the canonical guarantee table; verified by pasting the JSON output into the dev-log entry.
- [ ] **`docs/core-semantics.md` regenerated.** `cargo run -q -p corvid-cli -- contract regen-doc docs/core-semantics.md` runs cleanly; the drift gate test in `corvid-guarantees::render::tests::rendered_markdown_matches_committed_doc` passes.

**Adversarial gates**

- [ ] **Source-bypass test.** New compile-time contracts have a mutator in `crates/corvid-types/tests/source_bypass_corpus.rs` (or the phase's equivalent corpus) that proves the violation is rejected with the right `guarantee_id`.
- [ ] **Byte-fuzz test (if ABI / attestation / on-disk format).** New parsers carry ≥100 generated mutations in the phase's byte-fuzz corpus; all rejected; benign mutations round-trip.
- [ ] **Named threat coverage.** Every new attack class the slice introduces (approval bypass, scope escalation, replay forgery, connector contract drift, tenant crossing, prompt injection through new surface) has at least one named `must_fail` test.

**AI-in-the-development-loop gate**

- [ ] **AI-assisted helper named.** The slice brief names at least one LLM-pattern (RAG-grounded / generative / adversarial / agentic / assistive) helper the slice enables for maintainers — even when implementation lands in a follow-up. The helper itself runs as a Corvid program (typed effects, `@budget`, `Grounded<T>` outputs, replay-able trace).

**Production-readiness gates**

- [ ] **Real persistence path.** Anything the slice claims to persist actually lands in SQLite + Postgres with row-level locking; no JSON state files masquerading as databases.
- [ ] **Crash-recovery proof.** Any "durable" / "resumable" claim ships an integration test that `SIGKILL`s the worker mid-step and asserts byte-exact resume with no double-spend / double-side-effect.
- [ ] **Mock ≡ replay ≡ real.** Every external integration (connector, LLM, DB, OAuth) has mock + replay + real modes that share one typed surface; CI runs the same test in mock mode at minimum, real mode behind an opt-in env var.
- [ ] **Operator runbook delta.** Slices that add an operator-visible surface (new command, new manifest, new endpoint) update the relevant runbook page in the same commit.
- [ ] **Side-by-side comparison committed.** When the slice claims a moat dimension, a `benches/comparisons/<feature>.md` file shows the equivalent Python / TS / Go code line-by-line, with a Corvid-vs-other governance-line-count delta.

**Documentation gates**

- [ ] **`dev-log.md` entry.** One date-stamped entry per slice; explains *what* changed, *why*, and *how it's tested*. Filled-in checklist pasted in.
- [ ] **`learnings.md` entry (if user-visible).** Doc-and-feature land together.
- [ ] **`docs/security-model.md` reviewed.** If the slice changes the TCB, threat model, or non-goals, the security model is updated in the same commit.
- [ ] **README + landing page alignment.** If the slice introduces a public claim, the wording is derivable from a runnable command. No aspirational copy.

**Phase-level gates (apply when ticking the *phase* done — not the slice)**

- [ ] **Every slice box ticked.** Every `[x]` slice in the phase passed the slice gate above; no carry-over of "we'll fix it in the next phase."
- [ ] **End-to-end demo runnable.** The phase's "Done when:" sentence translates to one or more shell commands that produce the documented output on a clean clone.
- [ ] **No silent `OutOfScope` downgrades.** If a registry entry was downgraded mid-phase, the reason is recorded in the registry AND a follow-up issue/slice is filed to promote it back.
- [ ] **External reviewer signoff (Phase 42–43 only).** At least one developer outside the contributor list runs the phase's demo and signs off in writing on a public issue.

A slice that fails any box rolls back to `[ ]`. The `[x]` is a contract, not a wish.

### Phase 36 — Production backend core (~8-10 weeks)

**Goal.** Corvid can build an always-on HTTP backend without a host framework. A developer should be able to write routes, JSON APIs, middleware, health checks, configuration, secrets, structured logs, graceful shutdown, and deployment-ready binaries in Corvid itself.

**Why this phase exists.** Developer pain is not "how do I write an agent demo?" It is "how do I turn this agent into a service that survives auth, retries, observability, secrets, deploys, audits, and production traffic?" Corvid's moat must travel through the backend layer, otherwise developers still have to glue the real product together in another language.

**Inventive benchmark target.** Compare the refund API against FastAPI, Express/Fastify, and Go HTTP. Corvid does not need to beat Go on raw requests/sec in this phase; it must beat AI-backend setup complexity by showing route effects, approvals, traces, env validation, and signed server claims in one language-level path with less handwritten governance code.

**Scope:**

- [ ] `server` declarations or a standard backend entry pattern with typed routes, request/response bodies, path/query params, headers, cookies, status codes, and error responses.
- [ ] Runtime HTTP server with async request handling, graceful shutdown, request IDs, timeouts, body-size limits, panic/error isolation, and platform parity.
- [ ] Middleware pipeline for auth, rate limits, tracing, CORS, compression, request logging, and effect-aware policy checks.
- [ ] Typed JSON encode/decode errors that preserve spans and route names in diagnostics.
- [ ] Config and environment layer with required/optional vars, typed parsing, redacted secret reporting, and `corvid doctor` validation.
- [ ] Health, readiness, and metrics endpoints generated from runtime state.
- [ ] `corvid build --target=server` emits a single backend binary with embedded route manifest and signed contract metadata when signing is enabled.
- [ ] AI-native integration: every route can declare effect, approval, replay, budget, provenance, and model-routing constraints; violations fail before deploy.
- [ ] End-to-end example: approval-gated refund API served entirely by Corvid, with no Rust/Python/Node host app.

**Slice checklist:**

- [x] 36A-backend-design-brief       `docs/phase-36-backend-core.md` defines backend syntax, runtime ownership, non-scope, route examples, and acceptance tests before code.
- [x] 36B-minimal-server-target      `corvid build --target=server` accepts one backend entrypoint and emits a runnable local server binary.
- [x] 36C-typed-route-model          GET/POST routes have typed path/query/body/response shapes and compile-time validation.
- [x] 36D-json-boundary              Server errors use a stable JSON envelope with request IDs, route, kind, message, and route-aware diagnostics.
- [x] 36E-server-runtime-basics      Request IDs, handler timeouts, graceful drain limits, body limits, and handler isolation work.
- [x] 36F-route-tracing              Every generated-server request emits route, method, status, duration, request ID, and effect metadata as structured trace JSON.
- [x] 36G-health-readiness-metrics   Generated health/readiness/metrics endpoints report server liveness, readiness, counters, and runtime identity.
- [x] 36H-config-and-secrets         Typed backend env validation works at server startup and through `corvid doctor` with redacted invalid values.
- [x] 36I-approval-effect-integration Dangerous route/tool paths without reachable route-local approval contracts fail before deploy.
- [x] 36J-backend-example            `examples/backend/refund_api` ships a checked approval-gated contract and runnable generated server entrypoint with tests.

**Done when:** `examples/backend/refund_api` runs as a production-shaped server, passes route tests, emits traces, enforces approval gates, validates env/config through `corvid doctor`, and builds with `corvid build --target=server`.

**Audit correction before market freeze:** Phase 36 is not market-frozen until the generated server uses a real HTTP parser/runtime boundary, has an actual middleware pipeline, and proves graceful shutdown plus handler timeouts under tests.

- [x] 36K-real-http-runtime          Replace the hand-rolled request-line parser with a production HTTP runtime/parser and route tests for HTTP/1.1 edge cases.
- [x] 36L-middleware-pipeline        Auth, rate-limit, tracing, CORS, compression, request logging, and effect-aware policy middleware run in a declared order.
- [x] 36M-shutdown-timeout-tests     Graceful shutdown, request timeout, body-limit, and handler-isolation behavior is covered by integration tests.

### Phase 37 — Persistence, migrations, and state (~8-10 weeks)

**Goal.** Corvid can own durable application state: tables, records, transactions, migrations, encrypted secrets/tokens, audit logs, and query APIs.

**Inventive benchmark target.** Compare a task/approval/audit schema against Prisma/TypeScript, SQLAlchemy/Alembic, and sqlx/Rust. Corvid must prove migration drift, typed decode failures, DB effects, replay summaries, and AI-action audit logs are first-class instead of manually assembled.

**Scope:**

- [ ] `std.db` with SQLite first, Postgres second: connection config, query execution, transactions, prepared statements, row decoding, and typed errors.
- [ ] Migration system: checked-in migrations, `corvid migrate up/down/status`, drift detection, checksum validation, and CI-safe dry runs.
- [ ] Typed records mapped to tables without hiding SQL; developers can use explicit queries and still get typed decode guarantees.
- [ ] Encrypted token/credential storage for OAuth refresh tokens and connector state, with clear host key-management boundaries.
- [ ] Audit-log table pattern for AI actions: who/what/why, prompt version, model, tool call, approval state, cost, trace ID, and replay key.
- [ ] AI-native integration: DB reads/writes are effect-tagged; dangerous writes can require approval; replay records deterministic DB interaction summaries.
- [ ] Golden examples for session state, task state, approval state, trace state, and connector token state.

**Slice checklist:**

- [x] 37A-persistence-design-brief   `docs/phase-37-persistence.md` defines DB scope, SQL posture, migration rules, effect model, replay model, and non-scope.
- [x] 37B-sqlite-connection-query    `std.db` exposes SQLite connection, parameterized query/execute, result, and redacted error envelopes.
- [x] 37C-typed-row-decoding         `std.db` exposes typed row decode envelopes for success, missing columns, and wrong value kinds.
- [x] 37D-transactions               `std.db` exposes transaction envelopes for commit, rollback, and nested-scope rejection metadata.
- [x] 37E-migrations-drift           `corvid migrate up/down/status` supports checksums, dry runs, drift detection, and CI failure on mismatch.
- [x] 37F-audit-log-pattern          Standard audit-log schema records actor, action, prompt/model/tool versions, approval state, cost, trace ID, and replay key.
- [x] 37G-token-storage-boundary     Encrypted connector-token storage ships with explicit key-management boundaries and tests.
- [x] 37H-postgres-support           Postgres reaches parity with the SQLite query/transaction/migration subset needed by reference apps.
- [x] 37I-db-effect-replay           DB reads/writes carry effect tags and replay records deterministic interaction summaries.
- [x] 37J-backend-state-example      Backend example persists users, tasks, approvals, traces, connector tokens, and durable agent state.

**Done when:** a Corvid backend can persist users, tasks, approvals, traces, connector tokens, and durable agent state through typed migrations and tests.

**Small-slice breakdown for remaining Phase 37 work:**

- [x] 37E1-migrate-command-shape     Add `corvid migrate status/up/down --dry-run` command shape and help text.
- [x] 37E2-migration-file-scan       Discover ordered checked-in SQL migrations and compute stable SHA-256 checksums.
- [x] 37E3-migration-state-store     Record applied migrations, timestamps, and checksums in a local state store.
- [x] 37E4-drift-detection           Detect changed, missing, duplicate, and out-of-order migrations with CI-safe exit codes.
- [x] 37E5-dry-run-report            Dry-run reports pending/applied/drifted migrations without mutating state.
- [x] 37E6-sqlite-sql-execution      `corvid migrate up` executes pending SQL transactionally against SQLite before recording applied state.
- [x] 37F1-audit-schema-envelope     Add `std.db` audit-log record envelopes for actor/action/model/tool/approval/cost/trace/replay.
- [x] 37F2-audit-write-helper        Add helpers/tests for approval-aware audit writes and redacted values.
- [x] 37F3-audit-example             Add a minimal backend audit-log example and regression test.
- [x] 37G1-token-envelope            Add token reference/encrypted-token metadata envelopes.
- [x] 37G2-host-key-doctor           `corvid doctor` validates token encryption key presence/shape without printing it.
- [x] 37G3-token-redaction-tests     Traces, errors, and audit helpers never print token values.
- [x] 37H1-postgres-design           Document Postgres parity subset and non-scope before code.
- [x] 37H2-postgres-connection       Add Postgres connection/query envelopes matching SQLite.
- [x] 37H3-postgres-migration-status Postgres migration status/drift path matches SQLite subset.
- [x] 37I1-db-effect-tags            DB read/write/migration/token/audit operations carry explicit effect tags.
- [x] 37I2-db-replay-summary         Replay summaries capture deterministic DB interaction metadata without raw secrets.
- [x] 37J1-state-example-schema      Backend state example defines users/tasks/approvals/traces/tokens tables.
- [x] 37J2-state-example-tests       Example migration, query, audit, token, and replay tests pass.
- [x] 37J3-state-runbook             Example documents backups, migration rollback, redaction, and operator checks.

**Audit correction before market freeze:** Phase 37 is not market-frozen until the stdlib DB surface performs real host-backed query/transaction execution, Postgres has a real driver-backed path rather than metadata envelopes, and `migrate down` has tested rollback semantics.

- [x] 37K-real-stdlib-db-runtime     Corvid-facing DB helpers execute SQLite queries/transactions through the runtime with typed decode errors.
- [x] 37L-real-postgres-runtime      Postgres connection/query path uses a real Postgres client with redacted error handling and parity-shaped query APIs.
- [x] 37M-migration-down-execution   `corvid migrate down` executes reviewed rollback SQL or fails clearly when no rollback exists.

### Phase 38 — Jobs, schedules, and durable agent execution (~8-10 weeks)

**Goal.** Corvid can run long-lived backend work safely: scheduled jobs, retrying jobs, background queues, idempotent actions, failure recovery, and bounded agent loops.

**Inventive benchmark target.** Compare durable agent jobs against Celery, BullMQ, Sidekiq-style queues, and Temporal-style workflows. Corvid must win on AI-specific safety: budgeted loops, approval waits, replayable agent checkpoints, tool-call lineage, and compile-time visibility of dangerous background work.

**Scope:**

- [ ] Durable job runner with enqueue, delay, cron, cancellation, concurrency limits, idempotency keys, retry/backoff, dead-letter queue, and job leases.
- [ ] Scheduler manifest visible to `corvid audit`: every recurring task has owner, effect set, max runtime, max cost, replay policy, and approval policy.
- [x] Durable agent run state: step checkpoints, tool-call results, approval waits, resume-after-crash, and replayable finalization.
- [x] Loop controls: max steps, max wall time, max spend, max tool calls, and escalation-on-stall.
- [ ] AI-native integration: every job carries a budget, effect row, provenance policy, and trace lineage; dangerous jobs cannot run without an approval boundary.
- [x] Operational controls: pause queue, drain workers, inspect job, retry job, cancel job, and export job trace.

**Slice checklist:**

- [x] 38A-jobs-design-brief          `docs/phase-38-jobs.md` defines queue semantics, durability model, scheduler model, approval waits, replay behavior, and non-scope.
- [x] 38B-enqueue-run-one-job        Runtime can enqueue and execute one persisted background job with typed input/output.
- [x] 38C-retry-backoff-dlq          Jobs support retry policies, backoff, terminal failure, and dead-letter inspection.
- [x] 38D-delayed-jobs-cron          Delayed jobs and cron schedules persist, recover after restart, and appear in `corvid audit`.
- [x] 38E-leases-concurrency-idempotency Jobs use leases, concurrency limits, and idempotency keys to avoid duplicate dangerous work.
- [x] 38F-agent-step-checkpoints     Durable agent runs checkpoint steps, tool-call results, and partial outputs.
- [x] 38G-approval-wait-resume       Jobs can pause on approval, resume after approve/deny/expire, and record the audit transition.
- [x] 38H-loop-bounds                Max steps, wall time, spend, and tool calls are enforced for job-backed agent loops.
- [x] 38I-job-ops-commands           Operators can pause queues, drain workers, inspect, retry, cancel, and export job traces.
- [x] 38J-executive-agent-jobs       Personal Executive Agent daily brief, meeting prep, triage, and follow-up jobs survive process restart.

**Done when:** the Personal Executive Agent backend can run daily brief generation, email triage, meeting prep, and follow-up reminders as durable jobs that survive process restart.

**Libraries & frameworks (Phase 38):**

- `tokio` — async worker pool (already a dep).
- `rusqlite` + `postgres` — job-store backends with row-level locks (already deps).
- `chrono-tz` — cron timezone correctness; DST handling is non-negotiable.
- `tokio-cron-scheduler` *or* hand-rolled cron — must support DST + missed-fire policies (`fire_once_on_recovery` / `skip`).
- `ulid` — monotonic job IDs.
- `tracing` — span emission for every state transition (already a dep); OTel hooks ride on Phase 40.
- Existing `ed25519-dalek` for job-receipt signing when the run produces a Phase 21 receipt.

**Developer flow (Phase 38):**

```corvid
@budget($0.20)
@retry(max_attempts: 3, backoff: exponential(base: 30s, cap: 5m))
@idempotency(key: brief.user_id)
@replayable
job daily_brief(user_id: String) uses email_effect, summary_effect:
    inbox = gmail.recent(user_id, since: yesterday())
    summary = summarise(inbox)
    approve SendBrief(user_id, summary)
    gmail.send(user_id, summary)

schedule "0 8 * * *" zone "America/New_York" -> daily_brief(every_user())
```

```bash
corvid jobs run --queue=default --workers=4
corvid jobs schedule list
corvid jobs inspect <id>
corvid jobs explain <id>          # AI-assisted root-cause from the typed trace
corvid jobs dlq triage            # AI-assisted DLQ pattern clustering
corvid jobs retry <id>
corvid jobs export-trace <id>
corvid jobs pause --queue=default
corvid jobs drain --workers=all
```

**Phase-done checklist (Phase 38):**

- [ ] `validate_signed_claim_coverage` recognises `@retry`, `@idempotency`, `@replayable`, `job`, `schedule`, and `await_approval` as declared contracts.
- [ ] Registry rows shipped: `jobs.durable_resume`, `jobs.idempotency_key_uniqueness`, `jobs.lease_exclusivity`, `jobs.retry_budget_bound`, `jobs.cron_dst_correct`, `jobs.replayable_side_effects`, `jobs.approval_wait_resume`, `jobs.loop_bounds_enforced` — each `Static` or `RuntimeChecked`, each with positive + adversarial test refs.
- [ ] Crash-recovery integration test: `SIGKILL` mid-step → resume with no LLM re-spend (verified by mock-LLM call counter).
- [ ] Idempotency adversarial test: 4 concurrent workers + 100 jobs same key → exactly 1 ran.
- [ ] DST cron test: a job scheduled for 2:30am on the spring-forward day fires according to the documented policy.
- [ ] Replay-quarantine test: replay an old job trace, assert no real provider call left the process.
- [ ] AI helper landed (or follow-up filed): `corvid jobs explain` (RAG-grounded) using the typed trace.
- [ ] Side-by-side `benches/comparisons/jobs_durability.md` against Celery + BullMQ + Temporal.

**Small-slice breakdown for Phase 38:**

- [x] 38B1-job-envelope              Add `std.jobs` job/input/output/state envelopes.
- [x] 38B2-enqueue-command           Add enqueue/run-one runtime path with local persisted state.
- [x] 38B3-one-job-test              One persisted job executes once with typed input/output.
- [x] 38C1-retry-policy-envelope     Retry/backoff/dead-letter metadata exists in stdlib.
- [x] 38C2-retry-runner              Runner applies retry/backoff and terminal failure.
- [x] 38C3-dlq-inspection            CLI can inspect dead-lettered jobs.
- [x] 38D1-delay-support             Delayed jobs persist and wake after restart.
- [x] 38D2-cron-manifest             Cron schedules appear in `corvid audit`.
- [x] 38D3-scheduler-recovery        Scheduler recovers missed/pending jobs after restart.
- [x] 38E1-lease-model               Job leases prevent duplicate workers.
- [x] 38E2-concurrency-limits        Queue and job-type concurrency limits are enforced.
- [x] 38E3-idempotency-keys          Duplicate dangerous jobs collapse or fail predictably.
- [x] 38F1-checkpoint-schema         Agent step/tool/partial-output checkpoints are durable.
- [x] 38F2-resume-agent-run          Agent run resumes from last checkpoint after crash.
- [x] 38G1-approval-wait-state       Jobs can enter approval-wait state with expiry.
- [x] 38G2-approval-resume           Approve/deny/expire transitions resume or stop jobs and write audit events.
- [x] 38H1-loop-budget-controls      Max steps, wall time, spend, and tool-call limits are enforced.
- [x] 38H2-stall-escalation          Stalled loops escalate or terminate with trace evidence.
- [x] 38I1-job-ops-cli               Pause/drain/inspect/retry/cancel/export commands work locally.
- [x] 38J1-exec-agent-job-spec       Personal Executive Agent job definitions are written and checked.
- [x] 38J2-exec-agent-restart-proof  Daily brief/meeting prep/triage/follow-up jobs survive restart in tests.

### Phase 39 — Auth, identity, and human approval product surface (~8-10 weeks)

**Goal.** Corvid can secure real multi-user AI backends and provide a production approval system rather than a demo `approve` hook.

**Inventive benchmark target.** Compare the approval flow against Auth.js/Express, FastAPI dependencies, and Go middleware. Corvid must win by proving that identity, tenant, permission, dangerous tool, and approval-contract relationships are statically visible and audited end-to-end.

**Scope:**

- [ ] `std.auth` for sessions, API keys, JWT verification, OAuth callback handling, CSRF protection, passwordless login hooks, and service-account auth.
- [ ] Identity and tenant model: user IDs, organization IDs, roles, permissions, and audit actor propagation through routes, jobs, tools, and traces.
- [ ] Approval queue API: create, list, inspect, approve, deny, expire, comment, delegate, and audit approvals.
- [ ] Typed approval contracts generated from dangerous tools: expected action, target resource, max cost, data touched, irreversible flag, expiry, and required approver role.
- [ ] Approval UI contract: backend serves enough structured data for any frontend to render approvals without reverse-engineering traces.
- [x] AI-native integration: compiler rejects dangerous route/job/tool paths that have no reachable approval contract.
- [x] Security tests for confused-deputy approval bypass, tenant-crossing approval reuse, stale approval replay, and privilege escalation.

**Slice checklist:**

- [x] 39A-auth-approval-design-brief `docs/phase-39-auth-approval.md` defines identity, tenant, session, approval, threat, and non-scope models.
- [x] 39B-session-api-key-auth        `std.auth` supports sessions and API keys with typed actor propagation into routes and traces.
- [x] 39C-jwt-oauth-callbacks        JWT verification and OAuth callback handling work for connector authorization flows.
- [x] 39D-tenant-role-permissions    User, organization, role, and permission checks propagate through routes, jobs, tools, and traces.
- [x] 39E-approval-queue-api         Approval create/list/inspect/approve/deny/expire/comment/delegate APIs ship with tests.
- [x] 39F-generated-approval-contracts Dangerous tools generate typed approval contracts with target, cost, data, expiry, irreversibility, and required role.
- [x] 39G-approval-ui-contract       Backend exposes structured approval payloads that any frontend can render without parsing traces.
- [x] 39H-compiler-approval-reachability Compiler rejects dangerous route/job/tool paths with no reachable approval contract.
- [x] 39I-security-bypass-tests      Tests cover confused-deputy approval bypass, tenant-crossing approval reuse, stale approval replay, and privilege escalation.
- [x] 39J-approval-product-example   Reference backend exposes real login, tenant-safe approvals, and auditable AI actions.

**Done when:** a backend can expose real user login, tenant-safe approvals, and auditable AI actions without outsourcing the core safety model to another framework.

**Libraries & frameworks (Phase 39):**

- `jsonwebtoken` — JWT verify (RS256, ES256, EdDSA) with JWKS caching + `kid` rotation.
- `argon2` — password + API-key hashing (Argon2id, OWASP defaults).
- `oauth2` *or* hand-rolled — OAuth2/OIDC client; PKCE-mandatory for public clients.
- `ring` *or* `rustls` — primitives for HMAC, signature, key-derivation.
- `hmac` + `sha2` — CSRF double-submit token MAC.
- `time` (already a dep) — expiry math, clock-skew tolerance.
- `cookie` — typed cookie parsing/serialization with `SameSite`, `HttpOnly`, `Secure` defaults.
- `rusqlite` + `postgres` — session store, API-key store, approval queue store.

**Developer flow (Phase 39):**

```corvid
auth my_api:
    sessions: cookie("__corvid_sess", secure, http_only, same_site: lax)
    api_keys: header("Authorization", scheme: bearer)
    jwt: verify_rs256(jwks_url: env("JWKS_URL"))
    csrf: double_submit("__corvid_csrf")

tenant Org { id: String, plan: Plan }
role Admin, Reviewer, Member
permission CanIssueRefund: Admin | Reviewer

@dangerous
@requires(permission: CanIssueRefund)
@approval(contract: RefundApproval)
tool issue_refund(actor: Actor, order_id: String, amount: Money) -> Receipt

approval RefundApproval:
    target: order_id
    cost_ceiling: $5000
    data: financial
    irreversible: true
    expires_in: 24h
    required_role: Admin
    policy { actor.role == Admin && amount < $100 }
    batch_with: same_tool, same_data_class, same_role
```

```bash
corvid auth migrate                         # session/api-key/approval tables
corvid auth keys issue <name> --tenant=<id>
corvid auth keys revoke <key-id>
corvid approvals queue --tenant=<id>
corvid approvals explain <id>               # AI-assisted reviewer summary
corvid approvals batch <ids...>             # batch-approve semantically-equivalent items
corvid approvals delegate <id> --to=<actor>
corvid approvals export --since=2026-04-01  # audit dump
```

**Phase-done checklist (Phase 39):**

- [ ] `validate_signed_claim_coverage` recognises `auth`, `tenant`, `role`, `permission`, `approval`, `@requires`, `@approval` as declared contracts.
- [ ] Registry rows shipped: `auth.session_rotation_on_privilege_change`, `auth.api_key_at_rest_hashed`, `auth.jwt_kid_rotation`, `auth.oauth_pkce_required`, `auth.csrf_double_submit`, `tenant.cross_tenant_compile_error`, `approval.policy_clause_static_check`, `approval.batch_equivalence_typed`, `approval.confused_deputy_typecheck` — every one `Static` or `RuntimeChecked` with positive + adversarial test refs.
- [ ] Adversarial corpus enumerates ≥10 named threats: confused-deputy, tenant crossing, stale-approval replay (>expiry), session fixation, scope escalation, batch-approval drift across data classes, JWT kid downgrade, OAuth state tampering, CSRF bypass on PUT/PATCH/DELETE, role escalation via stolen approval contract.
- [ ] Reachability analysis: a typecheck fails when any reachable path to a `@dangerous` tool lacks a matching `approve` contract whose `required_role` covers every reachable caller.
- [ ] AI helper landed (or follow-up filed): `corvid approvals explain <id>` (assistive) — typed reviewer summary; `corvid approvals policy-suggest <tool>` (generative) — proposes a `policy { ... }` clause from the last 200 approvals.
- [ ] Side-by-side `benches/comparisons/auth_approval.md` against Auth.js, FastAPI dependencies, Go middleware.

**Small-slice breakdown for Phase 39:**

- [x] 39B1-actor-envelope            Add typed actor/session/api-key envelopes.
- [x] 39B2-session-runtime           Session auth resolves an actor into route/job/trace context.
- [x] 39B3-api-key-runtime           API-key auth supports service actors and redacted diagnostics.
- [x] 39C1-jwt-verify-contract       JWT verification surface and failure diagnostics are defined.
- [x] 39C2-oauth-callback-state      OAuth callback state/token references are typed and replay/audit visible.
- [x] 39D1-tenant-role-model         User/org/role/permission records and helpers are typed.
- [x] 39D2-permission-propagation    Permissions propagate through routes, jobs, tools, and traces.
- [x] 39E1-approval-store            Approval queue persistence schema and stdlib envelopes exist.
- [x] 39E2-approval-api              Create/list/inspect/approve/deny/expire/comment/delegate APIs work.
- [x] 39E3-approval-audit            Every approval transition writes audit and trace evidence.
- [x] 39F1-contract-generation       Dangerous tools generate typed approval contract records.
- [x] 39F2-contract-policy-check     Required role/expiry/irreversibility/cost/data rules are enforced.
- [x] 39G1-ui-payload-schema         Approval UI payload schema is stable and frontend-agnostic.
- [x] 39G2-ui-contract-tests         Payloads can be rendered without parsing traces.
- [x] 39H1-reachability-analysis     Compiler checks route/job/tool paths for reachable approvals.
- [x] 39H2-reachability-bypass-tests Confused-deputy, tenant-crossing, stale replay, and privilege escalation tests fail closed.
- [x] 39J1-auth-example              Reference backend has login/API-key auth.
- [x] 39J2-approval-product-example  Reference backend exposes tenant-safe approvals and auditable AI actions.

### Phase 40 — Agent observability, evals, and production monitoring (~6-8 weeks)

**Goal.** Corvid gives maintainers the operational visibility needed to trust AI systems in production: traces, metrics, evals, cost, latency, drift, and human review.

**Inventive benchmark target.** Compare incident diagnosis against OpenTelemetry plus ad hoc LangSmith/Langfuse-style tracing. Corvid must win on time-to-answer for: what action happened, why, who approved it, what it cost, what data it touched, which guarantee applied, and how to replay or promote it into an eval.

**Scope:**

- [x] Trace viewer data model and export format for route -> job -> agent -> prompt -> tool -> approval -> DB lineage.
- [ ] OpenTelemetry export for request metrics, job metrics, LLM calls, tool calls, approvals, errors, retries, token/cost usage, model-routing decisions, and replay IDs.
- [ ] `corvid observe` command for local trace inspection, cost reports, approval summaries, failing runs, and hot spots.
- [ ] Evals from production traces: promote trace slices into regression tests with redacted inputs, expected contracts, and replay fixtures.
- [ ] Drift and regression reports: model output schema failures, confidence drops, cost changes, latency changes, approval denial spikes, and tool-error spikes.
- [ ] Human-review queues for low-confidence or high-risk outputs, with audit linkage back to source prompt/model/tool versions.
- [ ] AI-native integration: observability is contract-aware; reports group failures by violated guarantee/effect/budget/provenance rule.

**Slice checklist:**

- [x] 40A-observability-design-brief `docs/phase-40-observability.md` defines trace schema, metrics taxonomy, eval promotion, retention, redaction, and non-scope.
- [x] 40B-lineage-trace-model        Route -> job -> agent -> prompt -> tool -> approval -> DB lineage is represented in one trace model.
- [ ] 40C-otel-export                OpenTelemetry export covers requests, jobs, LLM calls, tools, approvals, errors, retries, costs, and replay IDs.
- [ ] 40D-observe-command-basics     `corvid observe` lists traces, costs, approvals, failures, and hot spots from local stores.
- [ ] 40E-trace-to-eval              Production trace slices can be promoted into redacted regression/eval fixtures.
- [ ] 40F-drift-regression-reports   Reports highlight schema failures, confidence drops, cost changes, latency changes, denial spikes, and tool-error spikes.
- [ ] 40G-human-review-queues        Low-confidence and high-risk outputs can enter human-review queues with trace/audit linkage.
- [ ] 40H-contract-aware-grouping    Observability reports group incidents by guarantee, effect, budget, provenance, and approval rule.
- [ ] 40I-maintainer-runbook         Docs show how maintainers answer cost, action, approval, data-touch, and replay questions from tooling.

**Done when:** maintainers can answer "what did the agent do, why, what did it cost, who approved it, what data did it touch, and can I replay it?" from committed Corvid tooling.

**Libraries & frameworks (Phase 40):**

- `opentelemetry`, `opentelemetry-otlp`, `tracing-opentelemetry` — OTLP/HTTP + OTLP/gRPC export; `corvid.*` semantic conventions.
- `prometheus` (text exposition) — `/metrics` endpoint backed by typed counters/histograms.
- `rusqlite` FTS5 (built-in) — local trace search over the embedded trace store.
- `sha2` (already a dep) — deterministic redaction-key derivation for eval promotion.
- Existing `ed25519-dalek` for signing promoted-eval fixtures so their lineage is verifiable.

**Developer flow (Phase 40):**

```bash
corvid observe list --since=1h --status=failed
corvid observe show <trace-id>           # renders lineage tree + cost + approvals + guarantees
corvid observe drift --from=<id> --to=<id>
corvid observe explain <trace-id>        # AI-assisted root cause (RAG-grounded)
corvid observe cost --by=guarantee_id
corvid observe cost-optimise <agent>     # AI-assisted route/escalate suggestions (generative)
corvid eval promote <trace-id> --redact=email,phone,name
corvid eval drift --explain              # decompose model / input / prompt / index drift
corvid eval generate-from-feedback <id>  # AI-assisted eval from a user "wrong answer" report
corvid review-queue list --rank=cost-of-being-wrong
corvid observe export --otlp=https://otel.host:4317
corvid observe metrics --listen=:9090
```

**Phase-done checklist (Phase 40):**

- [ ] Lineage IDs (`trace_id`, parent `span_id`) stored on every route / job / agent / prompt / tool / approval / DB row — verifiable by SQL `JOIN` against the trace store.
- [ ] OTel conformance test against a docker-compose Jaeger collector passes; spans carry `corvid.guarantee_id`, `corvid.cost_usd`, `corvid.approval_id`, `corvid.replay_key` attributes.
- [ ] Registry rows shipped: `observability.lineage_completeness`, `observability.otel_conformance`, `observability.redaction_determinism`, `eval.drift_attribution`, `eval.promotion_signed_lineage`, `review_queue.cost_of_being_wrong_ranking`, `observability.contract_aware_grouping` — `Static` or `RuntimeChecked` with positive + adversarial test refs.
- [ ] Redaction adversarial test: promote a trace containing fake SSNs → assert zero regex matches against an SSN pattern in the resulting fixture file.
- [ ] Drift attribution test: synthetically swap (a) the model fingerprint, (b) the prompt, (c) the retrieval index — assert the explainer reports each contribution to the drop.
- [ ] AI helper landed (or follow-up filed): `corvid observe explain` (RAG-grounded) + `corvid eval promote` (agentic).
- [ ] Side-by-side `benches/comparisons/observability.md` against OpenTelemetry + LangSmith / Langfuse on time-to-answer for: cost, approval, action, data-touch, replay.

**Small-slice breakdown for Phase 40:**

- [x] 40B1-trace-link-ids            Request/job/agent/prompt/tool/approval/DB events share stable lineage IDs.
- [x] 40B2-lineage-render            Local command renders the lineage tree for one run.
- [x] 40C1-otel-schema               OTel span/metric/log mapping is documented and tested.
- [ ] 40C2-otel-exporter             Exporter emits request/job/LLM/tool/approval/error/retry/cost/replay data.
- [ ] 40D1-observe-list              `corvid observe list` shows local runs, failures, costs, approvals, and hot spots.
- [ ] 40D2-observe-show              `corvid observe show <id>` explains one run with contract-aware grouping.
- [ ] 40E1-trace-redaction           Production trace slices can be redacted deterministically.
- [ ] 40E2-eval-promotion            Redacted trace slices become regression/eval fixtures.
- [ ] 40F1-drift-metrics             Schema/confidence/cost/latency/denial/tool-error drift is computed.
- [ ] 40F2-drift-report              Drift report is human-readable and CI-friendly.
- [ ] 40G1-review-queue-envelope     Human-review queue records link to trace/audit IDs.
- [ ] 40G2-review-queue-ops          Low-confidence/high-risk outputs enter review and resolve with audit evidence.
- [ ] 40H1-guarantee-grouping        Incidents group by guarantee/effect/budget/provenance/approval rule.
- [ ] 40I1-operator-questions        Runbook maps common maintainer questions to exact commands.

### Phase 41 — Production connectors (~8-12 weeks)

**Goal.** Corvid ships connectors for the workflows real personal and enterprise agents need, with effect profiles and approval boundaries built in.

**Inventive benchmark target.** Compare connector implementation against raw SDK use in Python/TypeScript. Corvid must win on safe write operations: OAuth state, scopes, rate limits, mocks, replay fixtures, data-class effects, and approval-gated sends/updates are declared in the connector manifest rather than hand-documented.

**Scope:**

- [ ] Gmail/Google Workspace connector: read/search messages, draft replies, send only with approval, labels, attachments metadata, and OAuth token refresh.
- [ ] Microsoft 365 connector: Outlook mail, calendar, contacts, Teams/Graph basics, and tenant-aware OAuth.
- [ ] Calendar connector: availability, event create/update/cancel, meeting prep context, reminders, and approval-gated external invites.
- [ ] Slack connector: read channels/DM metadata, draft messages, send with approval, thread summaries, and workspace/user scoping.
- [ ] Task/project connectors: Linear and GitHub issues first; typed task creation/update/comment flows with approval gates.
- [ ] Local files connector for personal knowledge: indexed folders, file metadata, read permissions, write approval, and provenance-preserving snippets.
- [ ] Connector manifest format: scopes, effects, data classes, approval requirements, replay policy, rate limits, and failure modes.
- [ ] Mock connector suite for offline tests and deterministic demos; no production connector ships without a mock and replay fixture.

**Slice checklist:**

- [ ] 41A-connector-design-brief     `docs/phase-41-connectors.md` defines connector manifest shape, OAuth/token state, effect profiles, mocks, replay, and non-scope.
- [ ] 41B-connector-runtime-contract Shared connector runtime handles auth state, rate limits, retries, redaction, trace events, and mock mode.
- [ ] 41C-gmail-google-workspace     Gmail/Google Workspace connector supports read/search/draft/send-with-approval and token refresh.
- [ ] 41D-microsoft-365              Microsoft 365 connector supports Outlook mail, calendar basics, contacts, Graph auth, and tenant-aware scopes.
- [ ] 41E-calendar-connector         Calendar connector supports availability, event create/update/cancel, reminders, and approval-gated external invites.
- [ ] 41F-slack-connector            Slack connector supports channel/DM reads, draft/send-with-approval, threads, and workspace/user scoping.
- [ ] 41G-task-project-connectors    Linear and GitHub issue connectors support typed create/update/comment flows with approval gates.
- [ ] 41H-local-files-connector      Local file connector supports indexed folders, read permissions, write approval, and provenance snippets.
- [ ] 41I-mock-replay-suite          Every connector ships mock mode, replay fixtures, manifest tests, and offline deterministic examples.
- [ ] 41J-executive-agent-connectors Personal Executive Agent uses email, calendar, tasks, chat, and files through Corvid-owned connectors.

**Done when:** the Personal Executive Agent can connect to email, calendar, tasks, and files through Corvid-owned backend connectors, with explicit effects and approval contracts.

**Libraries & frameworks (Phase 41):**

- `reqwest` (already a dep) — HTTP client; rustls-tls feature for portability.
- Hand-rolled clients per provider: Gmail/Workspace REST, Microsoft Graph, Slack Web API, Linear GraphQL. Reason: auto-generated crates (`google-apis-rs`, `microsoft-graph-rs`) drift faster than they ship; the no-shortcut posture demands typed contracts we own.
- `octocrab` — GitHub API client with retry + rate-limit awareness baked in.
- `notify` — local file-watch events for the local-files connector.
- `tantivy` — local FTS index for personal-knowledge file search.
- `pdf-extract` (already a dep) — PDF body extraction for indexing.
- `ical` — calendar parsing (.ics imports + Outlook/Google calendar interop).
- `lettre` — outbound SMTP fallback for self-hosted email gateways.
- `hmac` + `sha2` — webhook signature verification (Slack, GitHub, Linear).
- New shared crate `corvid-connector-runtime` — auth state, retries, rate limits, redaction, trace events, and mock/replay/real swap.

**Developer flow (Phase 41):**

```corvid
import std.connectors.gmail as gmail
import std.connectors.calendar as cal

connector gmail uses oauth2_token, network_effect:
    scopes: [gmail.modify, gmail.send]
    rate_limit: 250_per_user_per_second
    redact: message.body in traces

agent triage(user_id: String) -> Brief uses gmail.read_metadata, summary_effect:
    msgs: List<Grounded<Message>> = gmail.search(user_id, "is:unread newer_than:1d")
    return summarise(msgs)
```

```bash
corvid connectors list
corvid connectors check --live                  # contract drift detection
corvid connectors mock-fixture-gen <name>       # AI-assisted fixture from a real-provider sample (generative)
corvid connectors scopes-min <source>           # AI-assisted scope minimisation (agentic)
corvid connectors fail-sim <name>               # AI-assisted adversarial generator (adversarial)
corvid connectors run --mode=mock|replay|real
corvid connectors oauth init <provider>         # PKCE flow + token storage
corvid connectors oauth rotate <token-id>
corvid connectors verify-webhook --sig=<...>
```

**Phase-done checklist (Phase 41):**

- [ ] `validate_signed_claim_coverage` recognises `connector`, `scopes`, `rate_limit`, `redact`, `webhook_signed_by` as declared contracts.
- [ ] Registry rows shipped per connector: `connector.<name>.scope_minimum_enforced`, `connector.<name>.write_requires_approval`, `connector.<name>.rate_limit_respects_provider`, `connector.<name>.contract_drift_detected`, `connector.<name>.webhook_signature_verified`, `connector.<name>.replay_quarantine` — every entry `Static` or `RuntimeChecked` with positive + adversarial tests.
- [ ] Mock ≡ replay ≡ real: each connector ships all three modes; CI runs the same integration test in mock by default and real behind `CORVID_PROVIDER_LIVE=1`.
- [ ] Adversarial corpus enumerates per-connector named threats: token-scope escalation, cross-tenant message access, refresh-token replay after revocation, malformed JSON body, 429/5xx retries with `Retry-After`, expired OAuth state, webhook signature forgery.
- [ ] Provenance test: every connector return is `Grounded<T>` whose provenance is the provider's record id; downstream code that strips provenance fails typecheck under `grounded.propagation_across_calls`.
- [ ] AI helpers landed (or follow-ups filed): `corvid connectors mock-fixture-gen` (generative) + `corvid connectors check --live` drift narrator (RAG-grounded) + `corvid connectors fail-sim` (adversarial).
- [ ] Side-by-side `benches/comparisons/connectors.md` against raw SDK use in Python + TypeScript on safety-line-count and time-to-write-a-new-connector.

**Small-slice breakdown for Phase 41:**

- [ ] 41B1-manifest-parser           Connector manifest parser validates scopes/effects/data classes/approval/replay/rate limits.
- [ ] 41B2-connector-runtime         Shared runtime handles auth state, retry, rate limits, redaction, trace events, and mock mode.
- [ ] 41B3-connector-test-kit        Mock/replay fixture harness is reusable across connectors.
- [ ] 41C1-gmail-read-search         Gmail read/search metadata works with mock and real-provider env docs.
- [ ] 41C2-gmail-draft-send         Draft/send is approval-gated and replay-visible.
- [ ] 41D1-ms365-mail-calendar       Outlook mail/calendar basics work through Graph auth.
- [ ] 41D2-ms365-tenant-scopes       Tenant-aware scopes and token refresh are tested.
- [ ] 41E1-calendar-availability     Availability and event read paths work.
- [ ] 41E2-calendar-write-approval   Event create/update/cancel and external invites require approval.
- [ ] 41F1-slack-read-thread         Slack channel/DM/thread reads work with workspace scoping.
- [ ] 41F2-slack-send-approval       Draft/send flows require approval and preserve audit evidence.
- [ ] 41G1-linear-github-read        Linear/GitHub issue read/search flows work.
- [ ] 41G2-linear-github-write       Create/update/comment flows are approval-gated.
- [ ] 41H1-files-index-read          Local file indexing/read permissions/provenance snippets work.
- [ ] 41H2-files-write-approval      File write/update/delete requires approval and records provenance.
- [ ] 41I1-all-mocks                 Every connector has mock mode and deterministic replay fixtures.
- [ ] 41J1-exec-agent-connector-plan Personal Executive Agent connector wiring is specified.
- [ ] 41J2-exec-agent-connector-proof Email/calendar/tasks/chat/files all run through connector mocks in tests.

### Phase 42 — Production reference applications (~10-14 weeks)

**Goal.** Prove Corvid can build real products by shipping complete backend reference apps, not toy demos. These apps are the market proof and the regression suite for the language.

**Inventive benchmark target.** Compare the Personal Executive Agent backend against an equivalent Python or TypeScript implementation. Corvid must show fewer external framework seams, fewer custom policy/audit/replay modules, stronger compile-time rejection of unsafe actions, and equivalent or better non-model orchestration latency.

**Reference apps:**

- [ ] **Personal Executive Agent backend.** Inbox triage, draft replies, calendar scheduling, meeting prep, daily brief, task extraction, follow-up tracking, approval-gated sends/edits, durable jobs, connector state, observability, and replay.
- [ ] **Personal Knowledge Agent backend.** Document ingestion, grounded search, citations, memory, private/local mode, evals from user feedback, and provenance-preserving answers.
- [ ] **Personal Finance Operations Agent backend.** Read-only aggregation first, bill/subscription reminders, budget explanations, anomaly detection, approval-gated payment intents, strict audit trail, and explicit non-scope for regulated financial advice.
- [ ] **Customer support operations agent backend.** Ticket triage, suggested replies, policy-grounded answers, refund/escalation approvals, SLA jobs, and eval dashboards.
- [ ] **Code-review and maintenance agent backend.** Repository ingestion, issue triage, review comments, patch proposals, CI-aware risk labels, and approval-gated write operations.

**Product requirements for every reference app:**

- [ ] Runs as a Corvid server binary with Corvid routes, DB, jobs, auth, connectors, approvals, traces, evals, and deployment manifest.
- [ ] Has seed data, mock connector mode, deterministic replay tests, adversarial tests, and a real provider mode behind documented env vars.
- [ ] Has an operator runbook: setup, secrets, migrations, backups, logs, metrics, incident response, and rollback.
- [ ] Has a clear security model and non-goals. No app over-claims autonomy or safety beyond what Corvid can enforce.

**Slice checklist:**

- [ ] 42A-reference-app-brief        `docs/phase-42-reference-apps.md` defines app selection, shared architecture, quality bar, security posture, demo mode, and non-scope.
- [ ] 42B-shared-app-template        Common backend template provides routes, DB, jobs, auth, connectors, approvals, traces, evals, deployment manifest, and runbook skeleton.
- [ ] 42C-personal-executive-agent   Personal Executive Agent backend ships inbox triage, drafts, calendar scheduling, meeting prep, daily brief, tasks, follow-ups, approvals, and replay.
- [ ] 42D-personal-knowledge-agent   Knowledge Agent backend ships ingestion, grounded search, citations, private/local mode, feedback evals, and provenance-preserving answers.
- [ ] 42E-finance-operations-agent   Finance Operations Agent backend ships read-only aggregation, reminders, anomaly detection, approval-gated payment intents, audit trail, and regulated-advice non-scope.
- [ ] 42F-support-operations-agent   Support Agent backend ships ticket triage, suggested replies, policy-grounded answers, refund/escalation approvals, SLA jobs, and eval dashboard.
- [ ] 42G-code-maintenance-agent     Code Maintenance Agent backend ships repo ingestion, issue triage, review comments, patch proposals, CI-aware risk labels, and approval-gated writes.
- [ ] 42H-reference-app-hardening    Every app gets seed data, mock connector mode, replay tests, adversarial tests, real-provider env docs, security model, and operator runbook.
- [ ] 42I-external-developer-trial   At least one external developer runs a reference app locally and files feedback before Phase 43.

**Done when:** external developers can clone the repo, run at least one full production-shaped backend app locally, inspect its approvals/traces/evals, and deploy it without writing a second backend in another language.

**Libraries & frameworks (Phase 42, app-side):**

- All Phase 41 connectors (Gmail, Workspace, M365, Slack, Linear, GitHub, local files).
- Phase 38 durable-jobs runtime + Phase 39 auth/approval + Phase 40 observability.
- `git2` (libgit2) — repository ingestion for Code Maintenance Agent.
- `tree-sitter` (rust + ts + py grammars) — code parsing for the same.
- `tantivy` *or* `meilisearch-sdk` — knowledge-app document index.
- `lettre` — outbound SMTP for the Personal Executive Agent's notification surface.
- `ical` — calendar import/export for the Knowledge + Executive apps.
- `pdf-extract` (already a dep) + `tika` (optional) — knowledge-app document parsing.

**Developer flow (Phase 42):**

```bash
corvid new my_app --template=executive-agent       # scaffolds routes/db/jobs/auth/connectors
cd my_app
corvid migrate up
corvid run --target=server --mode=mock             # offline development with mock connectors
corvid test                                        # eval cases + adversarial cases + replay tests
corvid eval list
corvid audit my_app                                # one-page operator summary (auto-generated)
corvid claim --explain target/release/libmy_app.so # signed enforced-claim manifest
corvid claim diff v1.0.0 v1.0.1                    # AI-assisted release diff (generative)
corvid run --target=server --mode=real             # real-provider mode behind env vars
```

**Phase-done checklist (Phase 42, applied per app):**

- [ ] App ships ≥10 tables, ≥5 migrations, foreign keys, indexes; `corvid migrate up` runs SQL (not bookkeeping).
- [ ] Auth: sessions + API keys + per-tenant + per-role; ≥1 typed permission per dangerous tool.
- [ ] Connectors: ≥3 in mock mode by default; ≥1 in real-provider mode behind a documented env var.
- [ ] Approvals: ≥5 distinct approval contracts; at least one uses `policy { ... }` and one uses `batch_with`.
- [ ] Durable jobs: ≥3 cron + ≥3 retry-policy-driven background tasks; each survives `SIGKILL` + restart in tests.
- [ ] Evals: ≥10 cases per app; ≥3 promoted from synthetic prod traces via `corvid eval promote`.
- [ ] Adversarial tests: ≥5 named threats per app (approval bypass, cross-tenant access, prompt injection through user input, token leakage, schema drift).
- [ ] Operator runbook: ≥1500 lines covering setup, secrets, migrations, backups, logs, metrics, incident response, rollback.
- [ ] Deployment manifests: Docker Compose + one PaaS (Fly/Render) + one K8s manifest per app; each smoke-deploys in CI.
- [ ] Side-by-side `benches/comparisons/<app>.md` shows the equivalent FastAPI/LangChain or Next.js+Vercel-AI-SDK implementation line-by-line (governance lines saved + non-model orchestration latency).
- [ ] App's signed cdylib's `corvid claim --explain` output is committed under `apps/<name>/CLAIM.md` and matches the README's shipped claims.
- [ ] AI helpers landed (per app): app-boot operator summary (assistive); weekly adversarial-test refresh (adversarial); auto-generated PR descriptions with claim diff (generative).
- [ ] External reviewer signoff: ≥1 developer outside the contributor list runs the app locally + signs off on a public issue.

**Small-slice breakdown for Phase 42:**

- [ ] 42B1-template-routes           Shared app template has routes, config, health/readiness, and generated docs.
- [ ] 42B2-template-state            Template has DB migrations, seed data, jobs, auth, and connector mocks.
- [ ] 42B3-template-ops              Template has traces, evals, deployment manifest, and runbook skeleton.
- [ ] 42C1-exec-agent-data-model     Personal Executive Agent schemas/migrations/jobs/connectors are defined.
- [ ] 42C2-exec-agent-inbox          Inbox triage and draft replies work in mock connector mode.
- [ ] 42C3-exec-agent-calendar       Scheduling, meeting prep, daily brief, and follow-ups run as durable jobs.
- [ ] 42C4-exec-agent-approval       Sends/edits are approval-gated and auditable.
- [ ] 42C5-exec-agent-hardening      Replay, evals, adversarial tests, and runbook are complete.
- [ ] 42D1-knowledge-ingestion       Knowledge app ingests docs with provenance and private/local mode.
- [ ] 42D2-knowledge-search-answer   Grounded search, citations, feedback evals, and answer provenance work.
- [ ] 42E1-finance-readonly          Finance app aggregates read-only data and explains budgets/subscriptions.
- [ ] 42E2-finance-approval-audit    Payment intents are approval-gated with strict non-advice and audit posture.
- [ ] 42F1-support-triage            Support app triages tickets and drafts policy-grounded replies.
- [ ] 42F2-support-approvals-sla     Refund/escalation approvals, SLA jobs, and eval dashboard work.
- [ ] 42G1-code-ingestion-triage     Code app ingests repos, triages issues, and labels CI-aware risk.
- [ ] 42G2-code-write-approval       Review comments/patch proposals/write actions require approval.
- [ ] 42H1-hardening-pack            Every app has seed data, mocks, replay tests, adversarial tests, env docs, security model, and runbook.
- [ ] 42I1-external-trial-one        One external developer runs an app locally and feedback is triaged.
- [ ] 42I2-external-trial-close      Feedback closes as code/docs/tests or explicit non-scope.

### Phase 43 — Packaging, deployment, and market readiness (~6-8 weeks)

**Goal.** Corvid is ready to go online as a product for developers and maintainers: installable, deployable, operable, documented, and honest under scrutiny.

**Inventive benchmark target.** Compare "clone to production-shaped deploy" against a representative FastAPI/LangChain or TypeScript agent stack. Corvid must win on reproducibility: signed binaries, env validation, migrations, health checks, deployment manifests, claim explanation, and operator docs generated from the same contracts used by the build.

**Scope:**

- [ ] `corvid deploy package`: Dockerfile, OCI image metadata, health/readiness config, migration runner, env schema, and signed build attestation.
- [ ] Deployment manifests for local Docker Compose, Fly.io/Render-style single service, Kubernetes, and bare-metal systemd.
- [ ] Release channels: nightly, beta, stable; SemVer policy tied to the stability contract and migration guide.
- [ ] Upgrade/migration tooling for syntax, stdlib, schema, trace format, and connector manifests.
- [ ] Maintainer docs: release checklist, security advisory process, compatibility policy, CI gates, benchmark reproduction, and claim review process.
- [ ] Developer docs: backend tutorial, Personal Executive Agent tutorial, connector authoring guide, approval-system guide, observability guide, and production checklist.
- [ ] Beta program: at least 20 external developers build real backend apps; feedback must close as code/docs/tests or explicit non-scope before launch.
- [ ] Final claim audit: README, website, launch page, docs, and `corvid claim --explain` say the same thing.
- [ ] Launch package: install scripts, changelog, signed binaries, checksums, reproducible build notes, demo scripts, and incident-response contacts.

**Slice checklist:**

- [ ] 43A-market-readiness-brief     `docs/phase-43-market-readiness.md` defines launch gates, release channels, support posture, security process, beta criteria, and non-scope.
- [ ] 43B-deploy-package             `corvid deploy package` emits Dockerfile, OCI metadata, health/readiness config, migration runner, env schema, and signed build attestation.
- [ ] 43C-deployment-manifests       Docker Compose, single-service PaaS, Kubernetes, and systemd manifests work for at least one reference app.
- [ ] 43D-release-channels           Nightly, beta, and stable release channels are documented and wired to SemVer/stability policy.
- [ ] 43E-upgrade-migration-tools    Syntax, stdlib, schema, trace-format, and connector-manifest migrations have tooling and docs.
- [ ] 43F-maintainer-docs            Release checklist, advisory process, compatibility policy, CI gates, benchmark reproduction, and claim review docs are complete.
- [ ] 43G-developer-docs             Backend tutorial, Personal Executive Agent tutorial, connector guide, approval guide, observability guide, and production checklist are complete.
- [ ] 43H-beta-program               At least 20 external developers build real backend apps; feedback is closed as code/docs/tests or explicit non-scope.
- [ ] 43I-final-claim-audit          README, website, launch page, docs, and `corvid claim --explain` use the same defensible claims.
- [ ] 43J-launch-package             Signed binaries, install scripts, changelog, checksums, reproducible notes, demo scripts, and incident contacts are ready.

**v1.0 final cut here. Launch day.** Corvid goes online only after the defensible core and the production-backend track are both complete.

**Libraries & frameworks (Phase 43):**

- `oci-spec` — OCI image manifest authoring; multi-stage Dockerfile (rust-builder → distroless runtime).
- `cargo-sbom` — SPDX SBOM generation for every release artifact.
- `cosign` (external binary) — signed-binary publishing; release attestation chained to the Phase 35 attestation envelope.
- Hand-rolled Dockerfile / Compose / K8s / systemd / fly.toml templates — no `helm` dep (too heavy for the v1 surface).
- `reqwest` (already a dep) — `corvid ops show <prod-url>` introspection client.
- `time` (already a dep) — release-channel calendar policy (nightly daily, beta weekly, stable cut by tag).

**Developer flow (Phase 43):**

```bash
corvid deploy package my_app/                # Dockerfile + OCI metadata + signed attestation + SBOM
corvid deploy compose my_app/                # docker-compose.yml + .env.example + healthchecks
corvid deploy fly my_app/                    # fly.toml + secrets template + region plan
corvid deploy k8s my_app/                    # Deployment + Service + Ingress + ConfigMap + Secret + HPA
corvid deploy systemd my_app/                # service unit + sysusers + tmpfiles
corvid release nightly                       # signed binaries + checksums + changelog
corvid release beta v1.0.0-beta.1
corvid release stable v1.0.0
corvid migrate run --check                   # CI-safe dry run with full drift detection
corvid upgrade --check                       # AI-assisted claim regression check before upgrade (agentic)
corvid upgrade --apply                       # applies codemods + flags hand-review cases
corvid ops show <prod-url> --key=<pubkey>    # live-binary introspection (signed by host)
corvid ops vuln <prod-url>                   # security advisory contact + policy
corvid claim audit                           # AI-assisted final claim audit (adversarial)
```

**Phase-done checklist (Phase 43):**

- [ ] `corvid deploy package` emits a multi-stage Dockerfile + distroless runtime ≤80 MB + OCI labels (`org.opencontainers.image.source`, signed-binary fingerprint) + `HEALTHCHECK` directive + full SPDX SBOM.
- [ ] Deployment manifests for Compose, Fly/Render, K8s (kind cluster smoke deploy in CI), and systemd are smoke-tested per release.
- [ ] Signed-attestation chain: `corvid deploy package`'s attestation references the same DSSE envelope `corvid claim --explain` consumes; the deploy attestation and the cdylib attestation cannot drift.
- [ ] Release channels (nightly / beta / stable) ship signed binaries + `SHA256SUMS.txt` signed by the release key; checksum file rooted in a key-rotation policy doc.
- [ ] Reproducible-build verification: a second build on a different host produces a bit-identical signed artifact; verified by an external reproducer in CI.
- [ ] `corvid upgrade --check` reports any guarantee that *would weaken* before applying the upgrade; integration test exercises the rejection path.
- [ ] Live-binary introspection: `corvid ops show <prod-url>` returns a signed claim manifest + costs-since-start + approvals-pending; opt-in only; the host's public key matches the binary's signing key.
- [ ] Final claim audit: every README / website / launch-page claim has a runnable command or test; `corvid claim audit` exits 0 with no aspirational wording flagged.
- [ ] Beta program: ≥20 external developers shipped ≥1 backend app each; their feedback closed as code/docs/tests OR explicit non-scope; the closure rate is published.
- [ ] Registry rows shipped: `deploy.reproducible_build`, `deploy.attestation_chain`, `deploy.sbom_completeness`, `release.signed_artifact`, `upgrade.claim_regression_check`, `ops.live_introspection_signed`, `claim.audit_runnable_artifacts` — `Static` or `RuntimeChecked`, with positive + adversarial test refs.
- [ ] AI helpers landed: release-note generator (generative), deployment-target tailor (agentic), migration assistant (agentic), beta-feedback synthesizer (agentic), final claim audit (adversarial).
- [ ] Side-by-side `benches/comparisons/clone_to_deploy.md` against FastAPI/LangChain + Next.js/Vercel on time-from-clone-to-production-shaped-deploy.

**Small-slice breakdown for Phase 43:**

- [ ] 43B1-package-dockerfile        `corvid deploy package` emits Dockerfile and OCI metadata.
- [ ] 43B2-package-runtime-config    Package includes health/readiness config, migration runner, env schema, and startup checks.
- [ ] 43B3-package-attestation       Package includes signed build attestation and verification docs.
- [ ] 43C1-compose-manifest          Docker Compose deploy works for one reference app.
- [ ] 43C2-paas-manifest             Fly/Render-style single-service deploy works.
- [ ] 43C3-k8s-systemd-manifests     Kubernetes and systemd manifests work or are explicitly scoped.
- [ ] 43D1-release-policy            Nightly/beta/stable SemVer and stability policy are documented.
- [ ] 43D2-release-automation        Release channel automation produces signed artifacts and changelog entries.
- [ ] 43E1-syntax-stdlib-migrator    Syntax and stdlib migration tooling exists.
- [ ] 43E2-schema-trace-migrator     Schema, trace-format, and connector-manifest migrations exist.
- [ ] 43F1-maintainer-runbooks       Release checklist, advisory process, compatibility policy, CI gates, benchmark reproduction, and claim review docs are complete.
- [ ] 43G1-developer-tutorials       Backend, Personal Executive Agent, connector, approval, observability, and production checklist docs are complete.
- [ ] 43H1-beta-intake               20 external developers are onboarded with issue templates and feedback labels.
- [ ] 43H2-beta-closure              Beta feedback is closed as code/docs/tests or explicit non-scope.
- [ ] 43I1-claim-inventory           README, website, launch page, docs, and `corvid claim --explain` claims are inventoried.
- [ ] 43I2-claim-alignment           All launch claims align with runnable artifacts and no aspirational wording remains.
- [ ] 43J1-release-artifacts         Signed binaries, install scripts, checksums, changelog, and reproducible notes are ready.
- [ ] 43J2-launch-rehearsal          Demo scripts, incident contacts, rollback plan, and final smoke tests are complete.

---

## Post-v1.0 roadmap

Scoped-out of the pre-v1.0 critical path. Not abandoned — explicitly planned, with honest reasoning for why they're not in v1.0.

- **Distributed multi-agent orchestration.** Cross-service agent graphs, recursive agent composition, distributed trace merging, and multi-tenant workflow sharding. Phase 38 ships durable single-backend agent execution for v1.0; this post-v1.0 item is the larger distributed/enterprise orchestration layer.
- **Hot reload.** In-flight runs keep version; new runs use new code. Production-runtime concern for always-on services. Most v1.0 users ship scripts + CLIs + embedded apps where restart-is-cheap. Ship when the production-service user segment is sized.
- **Prompt-aware compilation.** Schema caching, TOON compression, template deduplication. Performance optimization on top of v1.0 capability — measurable once cost data from real users shows where to target. Builds on Phase 20's cost model.
- **Interactive time-travel debugger UI.** Phase 21 ships deterministic replay; the scrub-backward / step-forward UI is a followup using the same infrastructure.
- **Generational GC, concurrent cycle collection.** Phase 17's cycle collector is good enough; generational + concurrent are post-v1.0 if allocation benchmarks ever justify the complexity.
- **Private package registries, binary packages.** Phase 25 ships the OSS registry + source packages; enterprise and binary distribution are post-v1.0.
- **Other editors (vim / emacs / JetBrains official extensions).** Phase 24 ships VS Code + the LSP; the LSP works with any client, but branded extensions are post-v1.0.

---

## Total estimated effort

**~47-57 months of focused solo work** from today to v1.0 public launch if done by one person, summed from the per-phase estimates above. The earlier 27-month plan proved the language core; the updated plan also proves Corvid as a production backend language for real AI applications.

| Release | Phases | Bottom-up estimate |
|---|---|---|
| v0.3 (close Phase 12) | 12k | ~2 weeks |
| v0.4 (native tier useful) | 13, 14, 15 | ~3 months |
| v0.5 (GP feel) | 16, 17, 18, 19 | ~3 months |
| v0.6 (moat + replay) | 20 (7 slices), 21 | ~5 months |
| v0.7 (embed + deploy) | 22, 23 | ~4 months |
| v0.8 (dev workflow) | 24, 25, 26, 27 | ~5 months |
| v0.9 (feature-complete) | 28, 29, 30, 31, 32 | ~5 months |
| v1.0 (launch polish) | 33, 34 | ~2 months |
| v1.0 (defensible core) | 35 | ~6-8 weeks |
| v1.0 (production backend) | 36, 37, 38, 39, 40, 41 | ~11-14 months |
| v1.0 (reference products + market readiness) | 42, 43 | ~4-5 months |

Bottom-up now sums to **~47-57 months** for a solo lane because the launch target changed: Corvid is no longer merely proving a language core plus defensible claims; it is proving that the language can build the full backend of production AI applications itself. Real slip will come from Phase 20 unknowns (slice 20d's cost-analysis is novel research; slice 20e's `T?confidence` interaction with the type system is unpredictable), Phase 24 (LSP - scope tends to grow), Phase 36-41 (backend/runtime/connectors create real operational surface area), and Phase 42 (reference apps expose missing language/runtime pieces). Build schedule with a 20% buffer; re-plan quarterly.

The original "~18-24 months" quote is not preserved above because preserving it would be dishonest. Quoting a smaller number while adding production backend, auth, persistence, jobs, connectors, observability, deployment, and reference products would be the shortcut; quoting what the plan actually sums to is the non-shortcut.

The dates aren't the point. The point is that each phase has:
- A clear goal with a named hard dependency, not a vibe sequence.
- A concrete scope list — no "TBD" or "polish" stand-ins.
- A version cut-line saying which release it ships in.
- A pre-phase brief before code.
- Tests green at the boundary.
- A dev-log entry.

That discipline is what makes the plan possible. Without it, the production-backend track becomes aspirational, and v1.0 turns into a marketing date instead of a shippable product.

---

## Non-goals

Red lines — features explicitly rejected, not merely deferred:

- **Raw pointer arithmetic + manual allocators.** Pointer aliasing is one of the hardest things for any reasoner (human or LLM) to track, and readability-for-LLM-generated-code is a first-class design goal. Narrow `@unsafe` FFI shim for C interop is allowed; pervasive pointers are a hard no. Rust and Zig own that niche — Corvid doesn't compete there.
- **Classical OOP inheritance.** `type` + methods (Phase 16) + (post-v1.0) interfaces are the model. Subclassing, `this`, virtual dispatch, and deep hierarchies are not. Modern GP consensus (Go, Rust, Swift, Kotlin) agrees composition + methods beat inheritance.
- **Rust/C++-level control for systems work.** Corvid aims for Go / Swift class performance. Fast enough that compute rarely bottlenecks AI-shaped software (where LLM latency dominates by three orders of magnitude), but not competing on hot-loop throughput.

Deferred, not rejected:

- **Every LLM provider at launch.** Anthropic + OpenAI ship first; Google, Ollama, and others follow in Phase 31.
- **Windows + Linux + macOS day-one.** Start on one OS (macOS); add the others in Phase 33 (v1.0 pre-launch polish).

What is *not* a non-goal, despite earlier framings: **being a general-purpose language.** Corvid must be one. The pre-v1.0 phases above ship every GP table-stakes feature (methods, cycle collector, Result, REPL, C ABI, WASM, LSP, package manager, testing) alongside the moat work — not as a bundle, not behind the moat, interleaved so every release is coherently Corvid.

---

## Velocity markers

To keep momentum honest, ship one observable artefact at every phase boundary. Every entry below is a live-demoable thing, not a completion-percent.

- **End of Phase 11** ✅ — `corvid run` dispatches through the interpreter + runtime with no Python on the path.
- **End of Phase 12** ✅ — `corvid run foo.cor` AOT-compiles + executes, caches on the second call. ~15× speedup measured. (v0.3)
- **End of Phase 15** — `corvid run examples/refund_bot_demo/src/main.cor --target=native` runs end-to-end: tool dispatch, prompt dispatch, approve tokens all working natively. (v0.4)
- **End of Phase 19** — `corvid repl` session demonstrates redefining an agent mid-session + inspecting struct values + calling a method on a user type. (v0.5)
- **End of Phase 21** — Demo video: write an agent with a `@budget($0.10)` annotation, make it exceed, compiler refuses; then `corvid replay <trace-id>` rewinds a recorded run and re-executes deterministically with zero LLM spend. (v0.6)
- **End of Phase 23** — Corvid program embedded in a Rust host (`cargo add` the cdylib) AND the same program compiled to wasm and running in a browser page. One source, two deployment targets. (v0.7)
- **End of Phase 27** — Full developer workflow demo: write in VS Code with live type hints, `corvid add` a registry package, `corvid test` runs, `corvid eval` produces the HTML report. (v0.8)
- **End of Phase 32** — Feature-complete: agents ask/choose humans, sessions persist to SQLite, Python libs import effect-tagged, Google + Ollama + Anthropic + OpenAI all work, `std.*` batteries included. (v0.9)
- **End of Phase 33** — launch polish foundation: installer, website, beta-tester feedback loop, launch GIF, announcement drafts, and claim audit scaffolding.
- **End of Phase 35** — `corvid claim --explain refund_bot.dylib` prints the binary's enforced guarantee set, signing key fingerprint, and bilateral-verifier attestation. `corvid contract list --json` round-trips into `docs/core-semantics.md` byte-for-byte. The fuzz corpus rejects 100% of mutated descriptors and bypassed sources. Independent verifier `corvid-abi-verify` rebuilds the descriptor for any signed cdylib and bit-matches the embedded one. CI re-runs all four on every push.
- **End of Phase 36** — `corvid build --target=server examples/backend/refund_api` emits a runnable backend binary with routes, config validation, health checks, traces, and approval-gated dangerous actions.
- **End of Phase 38** — the Personal Executive Agent's daily brief, meeting prep, and follow-up jobs survive process restart and resume with bounded cost, bounded steps, replay IDs, and auditable approval waits.
- **End of Phase 41** — email, calendar, task, chat, and file connectors all expose effect manifests, mock modes, OAuth/token state, replay fixtures, and approval-gated write operations.
- **End of Phase 42** — Personal Executive Agent backend runs locally as a production-shaped Corvid product: routes, DB, jobs, auth, connectors, approvals, traces, evals, replay, and deployment manifest.
- **End of Phase 43** — v1.0 public release: signed binaries, install scripts, deployment packages, production docs, external beta feedback closed, and launch claims aligned with `corvid claim --explain`.
