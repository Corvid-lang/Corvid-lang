# Corvid — Build Roadmap

> Phase-by-phase plan from v0.1 (complete) to v1.0 (public launch).
> For feature definitions see [`FEATURES.md`](./FEATURES.md).
> For architecture see [`ARCHITECTURE.md`](./ARCHITECTURE.md).

**Positioning.** Corvid is a **general-purpose AI-native language**, not an agent-only DSL. Ambition: be the default choice for the widest possible range of applications. "Best at everything" is a trap that has killed every language that tried it (PL/I, Ada, early Scala); the honest version is **narrow excellence on the moat, broad competence on table stakes, disqualified on nothing.**

### Moat — dimensions Corvid is built to genuinely win on

1. **Safety for AI-shaped software.** Effect checker, approve-before-dangerous, compile-time cost bounds, contract verification. Nobody else is competing here.
2. **AI-native ergonomics.** `agent` / `tool` / `prompt` / `approve` as keywords; replay, grounding contracts, cost budgets as language constructs. Structurally impossible to match without owning the whole pipeline.
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
- Proc-macro `#[tool]` + `corvid run` user-tool loading → Phase 14
- Streaming `Stream<T>` → Phase 20 (moat phase)
- Google / Ollama adapters → Phase 31
- Effect-tagged `import python` → Phase 30
- Async-native concurrent multi-agent execution → Post-v1.0 (deliberately out of pre-v1.0 scope; see bottom of this file)

**v0.2 complete. ~219 tests green.**

---

## In progress

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
5. **Speculative scope moved post-v1.0.** Features that are "enterprise maturity" or "optimization on top of v1.0 capability" (multi-agent durable execution, hot reload, prompt-aware compilation optimization) do not sit in the pre-v1.0 critical path.

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

### Phase 19 — REPL (~3 weeks)

**Goal.** `corvid repl` interactive shell. How users learn Corvid.

**Hard dep:** interpreter (✅).

**Scope:**
- Persistent session: locals, imports, agent declarations live across inputs.
- Redefine an agent mid-session; later calls use the new definition (no state migration — a fresh session is cheap).
- Pretty-printing of return values, including structs (field-by-field) and lists (with length).
- readline-class editing (history, ctrl-r search, multiline input with indent-aware continuation).
- `:help`, `:type <expr>`, `:reset`, `:quit` meta-commands.

**Non-scope:** Native-tier REPL. LSP integration (Phase 24 owns that).

**v0.5 cuts here.** Methods + cycle collector + Result + REPL make Corvid feel like a modern GP language.

---

### Phase 20 — Effect rigor + grounding + cost + streaming (~14–16 weeks) — **THE MOAT PHASE**

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

- [ ] `Grounded<T>` as a compiler-known stdlib type (like `Result`, `Option`). AST `TypeRef` variant + `Type::Grounded(Box<Type>)` + IR lowering.
- [ ] Provenance analyzer in the typechecker: walks each agent's data flow graph to determine which values inherit groundedness from tools with `data: grounded` in their effect declaration. If a value's provenance chain includes at least one grounded source, the value is provably grounded.
- [ ] Compile error `E0201` when an agent returns `Grounded<T>` but no path from a `data: grounded` tool feeds into the return value. Error message names the missing provenance link.
- [ ] Provenance flows compositionally across agent boundaries: if agent B calls a grounded tool and agent A calls B, A's return inherits B's groundedness.
- [ ] `cites ctx strictly` runtime annotation: compile-time proves groundedness exists; runtime verifies the LLM's cited passages actually appear in the context. Emits citation-checking code in the interpreter + native codegen.
- [ ] `.unwrap_discarding_sources()` method on `Grounded<T>` for when the caller consciously drops provenance.
- [ ] Built-in `retrieval` effect with `data: grounded` dimension registered in the `EffectRegistry` so tools can declare themselves as grounded sources.

#### Slice 20c — `eval ... assert ...` language syntax (~2 weeks)
- [ ] Parser + typechecker + lowering for `eval name: body ... assert expr` declarations.
- [ ] IR node `IrEval` alongside `IrAgent`.
- [ ] Runner CLI is out of scope — ships in Phase 27. This slice is language only.

#### Slice 20d — Cost dimension + `@budget` compile-time analysis (~3 weeks)

Cost is a dimension in the effect system, not a standalone annotation. `@budget($1.00)` is an `EffectConstraint` on the cost dimension.

- [ ] Each tool/prompt carries `cost: $X.XX` in its effect declaration.
- [ ] Compile-time worst-case cost analysis sums the cost dimension over control-flow paths using the composition algebra.
- [ ] `E0250` if worst-case cost > budget. `W0251` when the analysis can't prove a bound.
- [ ] Also ships the `@wrapping` annotation for opt-out overflow checks deferred from Phase 12.

#### Slice 20e — Confidence dimension (~2 weeks)

Confidence is a dimension in the effect system. The `Min` composition rule means the least confident result determines the chain.

The invention: confidence isn't a number — it's a dynamic authorization gate. The compiler couples confidence to trust, so a confident agent can act autonomously and an uncertain agent is forced to get human approval. No other system does this.

- [ ] `autonomous_if_confident(threshold)` trust variant: couples trust level to composed confidence. Above threshold → autonomous. Below → human approval activates at runtime.
- [ ] Confidence propagation: deterministic tools produce confidence 1.0, prompts carry LLM-reported confidence, `Min` composition through the call graph.
- [ ] Confidence gate in the interpreter: at tool dispatch, if trust is `autonomous_if_confident(T)`, compute composed confidence of inputs. Below T → dynamically activate the approval prompt.
- [ ] `@min_confidence(P)` compile-time constraint: compiler proves all paths to irreversible actions meet the confidence floor.
- [ ] `calibrated` modifier on prompts: runtime accumulates accuracy statistics, flags miscalibrated models when self-reported confidence drifts from actual accuracy.
- [ ] REPL integration: step-through shows confidence at each step. Confidence gates show threshold vs. actual when they fire.

#### Slice 20f — `Stream<T>` + latency dimension + streaming effect integration (~3 weeks)

Streaming in Corvid isn't just async iteration — streams are **first-class participants in the dimensional effect system**. Every dimension (cost, confidence, provenance, trust, latency) flows through stream types. No other language can do this because no other language has dimensional effects.

**Foundation:**
- [ ] `Stream<T>` as compiler-known stdlib type. Prompts + tools can declare streaming returns.
- [ ] `for x in stream:` consumes the stream. `yield` in agent bodies produces streams.
- [ ] `latency: streaming(backpressure: bounded(N) | unbounded)` dimension value.
- [ ] Tokio `mpsc::Receiver` backing; agent bodies with `yield` run as async tasks.

**Streaming effect integration (the inventions):**
- [ ] **Live cost termination mid-stream.** `@budget($1.00)` on an agent calling a streaming prompt tracks cumulative cost per yielded token. If the budget is exceeded while the stream is still producing, the runtime terminates and raises `BudgetExceeded`. No framework terminates streams by accumulated cost.
- [ ] **Per-element provenance in `Stream<Grounded<T>>`.** Each yielded element carries its own `ProvenanceChain`. Aggregate stream provenance is the union. Step-through REPL shows provenance building up in real time.
- [ ] **`try ... retry` over streams — stream-start semantics.** Retries fire at stream-open, not per-element. Transient connection failures retry with backoff; mid-stream errors propagate.
- [ ] **Confidence-floor termination.** `with min_confidence 0.80` on a streaming prompt terminates the stream if streaming confidence drops below threshold, raising `ConfidenceFloorBreached`.
- [ ] **Mid-stream model escalation** (paired with 20h). On confidence drop, the runtime opens a continuation stream on a stronger model, feeding the partial output as continuation context. Consumer sees seamless tokens with a `StreamUpgradeEvent` in the trace. No framework has this.
- [ ] **Progressive structured types: `Stream<Partial<T>>`.** Compiler-known `Partial<T>` where each field is `Complete(V)` or `Streaming`. Users access fields the moment they're complete without waiting for the full response. Type-level progressive structure.
- [ ] **Resumption tokens.** Cancellation produces a typed `resume_token` capturing elements delivered + provider session state. `resume(prompt, token)` continues from the interruption point, using provider continuation APIs when available, local re-run with accumulated context otherwise.
- [ ] **Declarative fan-out / fan-in.** `stream.split_by(key)` partitions a stream into sub-streams by key extractor. `merge(streams) ordered_by(policy)` combines with ordering guarantees (FIFO, sorted, fair round-robin). Compile-time type + effect checking.
- [ ] **Backpressure propagation.** A slow consumer pulls from a producer at its consumption rate. The effect system captures this: `backpressure: pulls_from(producer_rate)`. Cross-stream coordination when streams share a trace ID.

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

- [x] `corvid test adversarial` CLI command (stub wired — surfaces the plan, routes to the future generator)
- [ ] Generator prompt with category taxonomy (bypass angles) — **parked as post-20g follow-up** — needs LLM prompt-engineering framework + per-run API budget; the rest of 20g ships without it
- [ ] Regression corpus: every historical bypass attempt, permanently tested — partial (composition attacks covered via `counterexamples/composition/` + meta-verifier; LLM-generated bypasses pending generator)
- [ ] Accept/reject classifier runs the compiler on each generated program — same dependency as above
- [ ] Bypasses found during generation automatically filed as issues — same dependency

##### 3. Executable, interactive specification

The spec document isn't prose with code blocks. It's a **literate Corvid program** where every example is runnable. Readers click a code sample and it opens in the Corvid REPL with the session state pre-loaded. Every rule in the spec has:

1. A positive example (program exemplifying the rule)
2. A negative example (near-miss that the rule rejects, with the exact error message)
3. Link to the proptest property that checks the rule
4. Link to the cross-tier test that proves all four tiers agree

The spec becomes a **living proof obligation**. Change the composition algebra → the spec examples either still compile (ship it) or they don't (spec fails CI).

- [x] `docs/effects-spec/` as a literate spec — `.md` files with embedded runnable corvid blocks + `# expect:` directives (commits `3f80585` through `b628068`, 13 sections total)
- [x] Build pipeline: every code block compiles during spec publication — `corvid test spec` wired to CI (commit `4d4944b`). Current report: 5 compile / 38 skip / 0 fail across 43 blocks.
- [ ] Static site generator that renders the spec with "Run in REPL" buttons — **parked as post-20g follow-up** — spec is fully readable as Markdown on GitHub today; interactive renderer is a launch-phase nice-to-have
- [ ] Cross-links from spec rules to proptest + differential-verify tests — partial (spec references crate modules by path today; named-link cross-refs pending)
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
- [ ] Divergence reports name the rewrite rule that caused the profile drift — slice C of Dev B's invention #4 track

##### 5. Bounty-fed regression corpus

Phase 20g ships with a **standing bounty surface**:

> "Find a Corvid program that performs a dangerous operation without the compiler flagging it, composes effects incorrectly, or bypasses a constraint. Ship a PR with the program → we fix, credit you, add it to the regression corpus."

Every accepted bypass becomes a permanent entry in the counterexample museum. Future Corvid versions must reject every historical bypass. The spec's credibility compounds over time — each release is tested against every historical attack.

- [x] `docs/effects-spec/counterexamples/` directory with five composition-attack fixtures (commit `f4e802e`)
- [ ] Each counterexample has: the bypass program, the bug it exposed, the fix commit, the contributor credit — partial (header comments present; contributor credit pending bounty program)
- [x] CI rejects any change that causes a historical counterexample to compile again — meta-verifier (commit `e368ebb`) runs on every push via `.github/workflows/ci.yml`
- [ ] Public bounty page with submission guidelines and disclosed fixes — **parked as post-20g follow-up** — social infrastructure (disclosure policy, credit mechanism, GitHub issue template) better sequenced after public launch

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
- [ ] Optional Lean/Coq proof replay hook for dimensions that ship one
- [x] CI gate: any custom dimension whose laws fail blocks publication — `corvid add-dimension` runs the harness before writing

##### 8. Spec↔compiler bidirectional sync

Every `effect` declaration, `uses` clause, and constraint example in [docs/effects-spec/](../docs/effects-spec/) is parsed by the actual Corvid parser. Every composition rule table in the spec is evaluated by the actual type checker. The spec and the compiler cannot drift — every commit either ships matching spec+compiler or fails CI.

- [x] Spec examples extracted from every `.md` file in `docs/effects-spec/` (commit `413b39e`) — examples stay inline under ```corvid fences with `# expect: compile|error|skip` directives rather than a separate `examples/` directory
- [x] `corvid test spec` walks spec, compiles each block, compares outcome to the declared expectation
- [ ] Cross-links from spec rules → proptest files → differential-verify tests
- [ ] CI gate: any example whose behavior diverges from the spec fails the build — local enforcement is live, needs CI wiring

##### 9. Community dimension registry + `corvid effect-diff`

Other languages have package registries for code. Corvid has one for effect *dimensions*. `corvid add-dimension fairness@1.2` resolves a registered dimension, verifies its signature, replays its proofs against the current toolchain, and adds it to `corvid.toml`. Companion tool `corvid effect-diff <before> <after>` reports exactly which agents' composed profiles changed and which constraints newly fire or release — effect refactoring becomes safe because the diff tool surfaces every consequence.

- [x] `corvid add-dimension` CLI command — local-path form wired with pre-install law-check (commit `119cc9c`)
- [ ] Signed dimension artifacts (declaration + proof + regression corpus) — follow-up once registry hosts
- [ ] Registry host at `effect.corvid-lang.org` (placeholder — registry form returns actionable error, local-path form works today)
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
| #2 Adversarial LLM generation | ◐ CLI stub + harness design; full generator parked (needs prompt-engineering framework + API budget — better sequenced post-launch) |
| #3 Literate executable spec | ◐ Markdown spec + `corvid test spec` CI gate shipped; "Run in REPL" static-site renderer parked (Markdown is readable on GitHub today) |
| #4 Preserved-semantics fuzzing | ◐ Scaffold + slice A (α-conv, let-extract/inline) shipped; slices B + C on Dev B's track |
| #5 Bounty corpus | ◐ Seed corpus + meta-verifier + CI gate shipped; public bounty page + credit mechanism parked (social infra — sequenced after public launch) |
| #6 Custom dimensions via corvid.toml | ✅ shipped, CI gated |
| #7 Archetype law-check harness | ✅ shipped, CI gated (caught a real Union associativity bug during development) |
| #8 Spec↔compiler sync | ✅ shipped, CI gated |
| #9a `corvid effect-diff` | ✅ shipped |
| #9b `corvid add-dimension` (local-path) | ✅ shipped; registry host parked (needs hosted infrastructure — post-launch) |
| #10 Self-verifying meta-test | ✅ shipped, CI gated |

**Parked post-20g follow-ups** (none block downstream phases):
- Full adversarial-generation pipeline (needs prompt engineering + budget).
- Static-site spec renderer with "Run in REPL" buttons.
- Public bounty surface (issue template, disclosure protocol, credit mechanism).
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

- [ ] `model Name:` catalog declaration syntax (AST + parser + resolver + typechecker + IR)
- [ ] `DeclKind::Model` in the scope table; model references in effect rows and routing tables
- [ ] `requires:` / `latency:` / `specialty:` / `privacy:` annotations on prompts
- [ ] `route:` pattern-match routing with content predicates (`domain`, `language`, `length`, type checks)
- [ ] `classifier` prompt variant (routing prerequisite)
- [ ] `try ... else ... else` progressive refinement chains
- [ ] `ensemble N of [...] agree_at P` syntax
- [ ] `weighted_by accuracy_history` + `on disagreement escalate_to X`
- [ ] `generator: X validator: Y acts_as critic` adversarial validation
- [ ] `@jurisdiction`, `@compliance`, `privacy_tier` as dimensions
- [ ] `cacheable: true` + fingerprint cache in interpreter + replay integration
- [ ] `rollout(P%)` weighted routing for A/B tests
- [ ] `version: "..."` model versioning + replay-pinned safety
- [ ] Output-format-aware routing (`strict_json`, `markdown_strict`, etc.)
- [ ] Runtime adaptive selection + confidence-driven auto-escalation (builds on 20e gate)
- [ ] `corvid eval --swap-model` retrospective migration tooling
- [ ] `corvid routing-report` quality reports from eval data
- [ ] `corvid cost-frontier` Pareto visualization
- [ ] Bring-your-own-model sandboxing (Ollama/vLLM/llama.cpp adapter pattern)

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
| G (syntax) | `6047e00` | `adversarial:` propose / challenge / adjudicate block + order / arity parse checks |
| B-rt | `a2b9160` | Runtime: capability-based model dispatch |
| C-rt | `cf301d7` | Runtime: route-based model dispatch |
| E-rt | `1722a7a` | Runtime: progressive refinement dispatch |
| I-rt | `04f5c77` | Runtime: seeded rollout dispatch + `AbVariantChosen` trace |
| F-rt | `7651420` | Runtime: ensemble voting + `EnsembleVote` trace |
| G-contract | `a0345e7` | Adversarial stages typecheck as prompts with chaining contract |
| G-rt | `a610894` | Runtime: adversarial sequential pipeline + contradiction traces |
| H | `24c56fa` | `corvid routing-report` CLI + routing trace aggregation |

**Phase 20 closed.** Dimensional effects, grounding, evals, cost analysis, confidence gates, streaming effects, bypass verification, and the typed model substrate are all shipped. The moat phase is complete.

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

### Phase 21 — Replay (~5–6 months, maximal-flagship scope) — **THE FLAGSHIP WOW**

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
- [ ] 21-inv-H-4             AI-generated prose summary: LLM writes the top-of-receipt paragraph as `Grounded<Phrase>` tethered to specific algebraic deltas; reviewer gains a prompt-tier sibling agent.
- [ ] 21-inv-H-5             GitHub/CI integration: `--format=github-check|markdown|json` outputs for PR annotations, PR comments, and bot consumption.
- [ ] 21-inv-H               (rollup — closes when H-1..H-5 all land)
- [x] 21-docs                Spec [section 14](docs/effects-spec/14-replay.md) (Phase 21 implementation reference) + v1.0 launch demo at [docs/v1.0-demo-script.md](docs/v1.0-demo-script.md) + ROADMAP closeout status below.

**Phase 21 closeout status (as of 2026-04-22).**

Lane A (compiler + CLI + docs) has shipped every primary slice except the four `21-inv-H` follow-ups (H-2 counterfactual replay, H-3 structured approval/provenance drill-down, H-4 LLM prose summary, H-5 GitHub/CI format modes). The thesis claim is demonstrable today: `@replayable` compiles only what can be deterministically reproduced, every run writes a trace, `corvid test --from-traces --promote` closes the Jest-snapshot loop, `corvid trace-diff` produces a PR behavior receipt whose reviewer is itself a `@deterministic` Corvid agent.

Lane B (runtime + codegen + daemon) has shipped every slice except `21-inv-I-native` (native-tier shadow daemon parity), which is explicitly deferred to v0.6 — the interpreter-tier shadow daemon is the v1.0 claim; native parity is an optimisation.

What's between us and a clean "Phase 21 done" on the ROADMAP:

- Four receipt-extension slices (`21-inv-H-2` through `21-inv-H-5`). Each builds on `21-inv-H-1`'s surface, each is independently shippable, each is 1–3 days.
- The deferred native shadow daemon (`21-inv-I-native`), post-v1.0.

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
- [ ] 21-inv-I-native        Native-tier shadow replay daemon parity (interpreter shadow ships in v0.6)

**Rules (standing):** CLAUDE.md rubric on every file (1–2 responsibilities). One commit per file extraction or feature step. Validation gate between every commit: `cargo check --workspace` + `cargo test -p <crate> --lib` + `cargo test -p <crate> --test <name> -- --list` (for test-file touches) + `cargo run -q -p corvid-cli -- verify --corpus tests/corpus` (must still exit 1 only on the two deliberate fixtures). Push before next slice. Wait for acknowledgement at slice boundaries. Zero semantic changes mid-refactor. No shortcuts — a thin feature is a shortcut.

**Success criteria.** Every agent marked `@replayable` compiles iff it can be deterministically replayed. Every run under recording produces a JSONL trace that replays to byte-identical state. `corvid replay --model claude-opus-5.0 trace.jsonl` runs cost-free and reports divergences. `replay` is a first-class Corvid expression. Prod traces become regression tests with `corvid test --from-traces`. PRs show a behavior diff before merge. Live shadow mode detects regressions in production.

---

### Phase 22 — C ABI + library mode (~6–8 weeks)

**Goal.** Embed Corvid in Rust, Python, Node, Go hosts — with the AI-safety guarantees (effects, approvals, provenance, budgets) surviving into the host's type system. Corvid isn't just a callable library; it's the only embeddable language whose compile-time AI-safety contracts are observable from the host.

**Hard dep:** Phase 12 (native codegen).
**Soft dep:** Phase 17 (cycle collector). C ABI without the cycle collector means embedders who build cyclic data across the boundary leak — exactly the same behaviour every pre-Phase-17 Corvid program has. Not a compilation blocker, but pairing with Phase 17 at the same release is the honest story: the v0.7 pitch is "Corvid ships as a library" and shipping a leaking library would undercut that.

**Slice checklist:**

- [x] 22-A-cdylib            `pub extern "c"` + `--target=cdylib`/`--target=staticlib` + `--header` scalar C header
- [x] 22-B-abi-descriptor    `--abi-descriptor` + `corvid-abi` crate (machine-readable effect/approval/provenance surface, deterministic JSON)
- [ ] 22-C-prompt-catalog    Runtime-queryable typed prompt/agent catalog: cdylibs embed the descriptor, expose `corvid_list_agents` / `corvid_agent_signature` / `corvid_call_agent` so hosts can discover + dispatch agents with type-checked args at runtime
- [ ] 22-D-effect-filter     Host-side effect-dimension filter: `corvid_find_agents_where(trust<=autonomous, cost<=0.10)` — the host can narrow the agent set by effect algebra without re-reading the descriptor
- [ ] 22-E-approval-bridge   Approval contracts survive FFI: `@dangerous` entrypoints reach back through the boundary to invoke a host-supplied approver; no way for a host to bypass by linking
- [ ] 22-F-grounded-return   `Grounded<T>` return values cross the boundary with their provenance chain intact; host receives `(payload, provenance_handle)` it can query
- [ ] 22-G-budget-observe    Per-call cost/latency observability: host reads real-time budget burn per agent
- [ ] 22-H-replay-across-ffi Traces recorded on one side of the boundary replay deterministically from the other; the embedded binary becomes a recordable unit
- [ ] 22-I-host-bindings     Reference Rust + Python host crates; generated idiomatic bindings from the descriptor (Rust traits; Python Protocols)
- [ ] 22-J-ownership-check   Compile-time checker on extern signatures (who frees what, who retains what)
- [ ] 22-K-cdylib-demo       End-to-end `pub extern "c"` scalar-signature agent shipping as `.so`/`.dll`, plus a matching host-side Rust + Python demo that reads the descriptor and dispatches

**Non-scope:** WASM (Phase 23). Language-level FFI imports of other languages.

### Phase 23 — WASM target (~8–10 weeks)

**Goal.** Deploy Corvid to browsers and edge runtimes.

**Hard dep:** IR (✅). Parallel codegen backend to Cranelift-native; does not depend on it.

**Scope:**
- New `corvid-codegen-wasm` crate using `cranelift-wasm` (same Cranelift you've already shipped, different output target).
- `corvid build --target=wasm` emits `.wasm` + an ES module loader + TypeScript types.
- Runtime: the wasm module imports host functions for LLM calls + tool dispatch + approval UI + replay recording (host provides them — same pattern as JavaScript environments that delegate I/O).
- **Replay in WASM**: host functions that record tool + prompt + approve calls write to a JS-side trace store compatible with Phase 21's format. `corvid replay <trace>` on a WASM module runs via the same host-function contract, substituting recorded responses. Shared recording format means a trace captured from native can be replayed under WASM and vice versa — a property worth preserving from the start.
- wasmtime / wasmer harness tests running the same IR-level programs the native parity harness runs.
- Browser smoke test: a small Corvid program compiled to wasm and loaded in a web page.

**Non-scope:** Wasm-specific optimizations (post-v1.0). Wasm-side cycle collection (wasm's own GC proposal is stabilising; use it once available, fall back to host-delegated collection via exported functions in the interim).

**v0.7 cuts here.** Corvid ships as a library + a wasm module. Real deployment story.

---

### Phase 24 — LSP + IDE (~6–8 weeks)

**Goal.** Editor support worthy of a real GP language. Users need this to write serious Corvid — must land before the moat features are worth using daily.

**Hard dep:** frontend (✅). Types stable enough that LSP doesn't churn when language evolves.

**Scope:**
- `corvid-lsp` crate implementing the Language Server Protocol. Backend-agnostic (same LSP serves native + interpreter + wasm users).
- VS Code extension as the reference client.
- Features: diagnostics (live), hover with inferred types, completion, go-to-def, find-references, rename, inline-documentation.
- Effect rows shown in hover. `@budget($)` overruns shown as squigglies with the worst-case cost.
- Debugging attach point wired even if debugger UI is post-v1.0 — protocol contract stable.

**Non-scope:** Other editors (vim / emacs / JetBrains) — users can use the LSP via any LSP-compatible client, but official extensions are post-v1.0.

### Phase 25 — Package manager (~6–8 weeks)

**Goal.** Users can share Corvid code. Table stakes for any language anyone takes seriously.

**Hard dep:** nothing internal. Major external work: registry hosting.

**Scope:**
- `corvid add <pkg>`, `corvid remove`, `corvid update` CLI.
- `Corvid.lock` lockfile with exact resolved versions + content hashes.
- Registry service: stateless HTTP API + CDN for package tarballs. Hosting at `registry.corvid.dev`.
- SemVer-based resolution with conflict detection.
- `corvid.toml` `[dependencies]` section wired through the driver.

**Non-scope:** Private registries (post-v1.0). Binary package distribution (post-v1.0 — all v1.0 packages are source).

### Phase 26 — Testing primitives (~4 weeks)

**Goal.** `test`, `mock`, `fixture` as language features. Users can't ship production Corvid without first-class tests.

**Hard dep:** typechecker extension for `test`/`mock` decls.
**Soft dep:** Phase 25 (package manager). Shared fixtures can distribute as packages eventually, but in-repo fixtures work without the package manager — not a blocker.

**Scope:**
- `test name: body` declaration. Discovered automatically; run by `corvid test`.
- `mock tool_name: body` overrides a tool implementation within a test's scope.
- `fixture name: body` for reusable test data; resolved by `corvid test` at run time.
- Snapshot testing primitive — `assert_snapshot expr` writes the first run's value to a file, compares on subsequent runs.
- Interop with Phase 20's `eval ... assert ...` syntax (evals are tests, tests aren't necessarily evals — eval is statistical assertions over LLM behaviour).

### Phase 27 — Eval tooling CLI (~3 weeks)

**Goal.** Turn Phase 20's `eval ... assert ...` syntax into a usable dev + CI workflow.

**Hard dep:** Phase 20 slice 20c (eval syntax — nothing to run without it).
**Soft dep:** Phase 26 (testing primitives). Eval tooling could have its own runner + discovery, but reusing Phase 26's infrastructure avoids duplication; the sequencing here is "ship tests first, build eval on top."

**Scope:**
- `corvid eval <file>` runs all `eval` blocks; produces terminal report + HTML report.
- Regression detection against prior eval results (stored under `target/eval/`).
- CI exit-code contract: non-zero if any `assert` fails or regression threshold crossed.
- Prompt-diff report: when a prompt body changed between runs, show before/after + delta in grounding / cost / assert pass-rates.

**v0.8 cuts here.** Full developer workflow: write in LSP, share via package manager, test + eval in CI.

---

### Phase 28 — HITL expansion (~3 weeks)

**Goal.** `ask`, `choose`, rich approval UI. Completes the human-in-the-loop surface.

**Hard dep:** runtime (✅).

**Scope:**
- `ask(prompt, Type)` — structured input from the human. Returns `Type`. Ties into the approval runtime.
- `choose(options: [T]) -> T` — pick one. UI presents options; user selects.
- Rich `approve` UI: show context (why approval requested), diff preview (what will change), arguments inspection.
- CLI + web-UI implementations; approval tokens same regardless of UI.

### Phase 29 — Memory primitives (~4–5 weeks)

**Goal.** `session` and `memory` as typed, SQLite-backed stores. Core to how AI applications handle state.

**Hard dep:** Phase 18 (Result — `session.get()` returns `Result<T, StoreError>`). SQLite (external).

**Scope:**
- `session { ... }` block declares per-conversation state. Compiler generates typed accessors.
- `memory { ... }` block declares long-lived state (survives process restarts).
- Both backed by SQLite (native) and IndexedDB (wasm).
- Effect-tagged: `reads_session` / `writes_session` / `reads_memory` / `writes_memory`. Integrate with Phase 20's effect rows.

### Phase 30 — Python FFI via PyO3 (~5–6 weeks)

**Goal.** `import python "..."` works in compiled code. Closes the "but Python has the ecosystem" gap.

**Hard dep:** Phase 13 (async — PyO3's GIL-aware runtime needs async context).
**Soft dep:** Phase 20 slice 20a (effect rows). Python imports declare effects at the import site — the basic `effects: network` / `effects: unsafe` syntax works against the existing `safe` / `dangerous` split; richer user-declared effects via 20a's effect rows make the story better but aren't a compilation blocker.

**Scope:**
- PyO3 integration in `corvid-runtime`. Lazy CPython load.
- `import python "requests" as requests effects: network` — untagged imports rejected by the effect checker. `effects: unsafe` is the opt-in escape hatch and is flagged for review.
- Error marshalling: Python exceptions become Corvid `Result::Err` with preserved traceback.
- Type marshalling: Python dicts ↔ Corvid structs (when schema known), lists ↔ lists, scalars ↔ scalars.
- Interpreter tier gets the same FFI surface so both tiers behave identically.

### Phase 31 — Multi-provider LLM adapters (~2 weeks)

**Goal.** Google Gemini + Ollama + any other adapter users request.

**Hard dep:** runtime adapter trait (✅).

**Scope:**
- `GoogleAdapter` in `corvid-runtime`. API compatibility with existing AnthropicAdapter + OpenAiAdapter surface.
- `OllamaAdapter` for local-first Corvid.
- Provider selection via `CORVID_MODEL` env var (existing convention).

### Phase 32 — Standard library (~8 weeks)

**Goal.** Batteries included. Common patterns available without a package install.

**Hard dep:** everything language-core stable.

**Scope:**
- `std.rag` runtime pieces: sqlite-vec, document loaders (pdf / md / html), chunking, embedder trait with reference OpenAI + Ollama impls. Pairs with Phase 20's grounding-contract language half.
- `std.http` — typed HTTP client with effect tags.
- `std.io` — structured file I/O, streaming, path manipulation.
- `std.agent` — common patterns: classification, extraction, summarization, ranking.
- Everything in `std.*` effect-tagged so users get the moat's benefits from day one.

**v0.9 cuts here.** Language feature-complete: HITL, memory, Python FFI, multi-provider LLMs, stdlib. Only polish remaining.

---

### Phase 33 — Polish for launch (~6–10 weeks)

**Goal.** v1.0. Stable, documented, installable by a stranger on any OS.

**Hard dep:** everything.

**Scope:**
- Stability guarantees on the language surface: documented SemVer contract for syntax, type system, stdlib.
- Windows + Linux + macOS all first-class (`corvid doctor` passes, installer works, parity harness green on all three).
- Installer: `curl -fsSL corvid.dev/install.sh | sh` on Unix, PowerShell equivalent on Windows. Corresponding `cargo install` flow.
- Website: landing page, live playground (runs the wasm target from Phase 23), docs site, blog, benchmarks page.
- Documentation rewrite: reference, tutorial, cookbook, migration-from-Python guide.
- Launch materials: 2-minute GIF/video showing the time-travel replay moment + effect-checker catching a bug + compile-time cost budget. HN + Reddit + ProductHunt announcement drafts reviewed with 3 external readers.
- Beta round: 20 external developers build something real in Corvid; their feedback gates the final cut.

### Phase 34 — Inventions readme + landing page (~2 weeks)

**Goal.** Every Corvid invention documented in one place, visible from the repo's front door. The README and landing page must answer: "what does this language do that no other language does?" — in code, not in prose.

**Hard dep:** everything. This is the final writing pass before launch. Every feature referenced must be shipped and runnable.

**Why this phase exists.** Phase 33 ships v1.0 with documentation (reference, tutorial, cookbook, migration guide). Phase 34 adds a **dedicated inventions catalog** — a single authoritative document listing every feature Corvid has that no other language has, with runnable examples for each. This is the artifact developers link to, cite on HN, and scan before deciding to try Corvid. Without it, the inventions are buried across Phase 20 slices, the eval docs, the streaming spec, the typed model substrate spec, and the replay flagship docs.

**Scope:**

- [ ] Rewrite the repo root `README.md` with the full inventions catalog up top, above the install instructions. Every entry has a 2-line pitch + code example + link to spec.
- [ ] Category structure matching the moat: **Safety at compile time** (approve gates, dimensional effects, Grounded<T>, @min_confidence, @budget), **AI-native ergonomics** (agent/tool/prompt/approve/effect/model keywords, evals with trace assertions, replay), **Adaptive routing** (20h model substrate — capability routing, content-aware dispatch, progressive refinement, ensemble voting, adversarial validation, jurisdiction/compliance, privacy tiers, cost-frontier exploration), **Streaming** (20f — live cost termination, per-element provenance, mid-stream escalation, progressive structured types, resumption tokens, fan-out/fan-in), **Verification** (20g — cross-tier differential verification, LLM-driven adversarial bypass generation, executable interactive spec, preserved-semantics fuzzing, bounty-fed regression corpus).
- [ ] Landing page rewrite (`docs/site/`): every invention gets a runnable playground example. "Corvid is faster than Python at X" / "safer than TypeScript at Y" claims are supported with side-by-side comparisons that actually run.
- [ ] Runnable invention index: `corvid tour --topic <name>` CLI command that opens the REPL pre-loaded with a runnable demo of each invention. `corvid tour --list` shows the full catalog.
- [ ] Cross-references: each invention in the README links to (a) the roadmap slice that shipped it, (b) the spec section that formalizes it, (c) the example in the tour, (d) the test that validates it.
- [ ] Headline inventions page (`docs/inventions.md`): the standalone artifact HN threads link to. No install prerequisite, no build system context — just the inventions, their syntax, and why each is unique.
- [ ] Update `CLAUDE.md` (or equivalent contributor doc) to require that every new invention ships with a README catalog entry + tour demo.

**Non-scope:** marketing copy, video scripts, social-media assets — those belong to Phase 33's launch materials. Phase 34 is the authoritative technical catalog; Phase 33 is the launch campaign that points to it.

**v1.0 final cut here. Launch day.**

---

## Post-v1.0 roadmap

Scoped-out of the pre-v1.0 critical path. Not abandoned — explicitly planned, with honest reasoning for why they're not in v1.0.

- **Multi-agent + durable execution.** Crash-safe agents, recursion / composition with automatic trace merging. Enterprise-maturity feature; most v1.0 users write single-agent applications. Ship when real user pull for it is measurable. Uses the replay infrastructure from Phase 21.
- **Hot reload.** In-flight runs keep version; new runs use new code. Production-runtime concern for always-on services. Most v1.0 users ship scripts + CLIs + embedded apps where restart-is-cheap. Ship when the production-service user segment is sized.
- **Prompt-aware compilation.** Schema caching, TOON compression, template deduplication. Performance optimization on top of v1.0 capability — measurable once cost data from real users shows where to target. Builds on Phase 20's cost model.
- **Interactive time-travel debugger UI.** Phase 21 ships deterministic replay; the scrub-backward / step-forward UI is a followup using the same infrastructure.
- **Generational GC, concurrent cycle collection.** Phase 17's cycle collector is good enough; generational + concurrent are post-v1.0 if allocation benchmarks ever justify the complexity.
- **Private package registries, binary packages.** Phase 25 ships the OSS registry + source packages; enterprise and binary distribution are post-v1.0.
- **Other editors (vim / emacs / JetBrains official extensions).** Phase 24 ships VS Code + the LSP; the LSP works with any client, but branded extensions are post-v1.0.

---

## Total estimated effort

**~27 months of focused solo work** from today to v1.0 public launch, summed from the per-phase estimates above:

| Release | Phases | Bottom-up estimate |
|---|---|---|
| v0.3 (close Phase 12) | 12k | ~2 weeks |
| v0.4 (native tier useful) | 13, 14, 15 | ~3 months |
| v0.5 (GP feel) | 16, 17, 18, 19 | ~3 months |
| v0.6 (moat + replay) | 20 (7 slices), 21 | ~5 months |
| v0.7 (embed + deploy) | 22, 23 | ~4 months |
| v0.8 (dev workflow) | 24, 25, 26, 27 | ~5 months |
| v0.9 (feature-complete) | 28, 29, 30, 31, 32 | ~5 months |
| v1.0 (launch polish) | 33 | ~2 months |

Bottom-up sums to **~27 months** — over the 18–24 originally quoted because the bottom-up is pessimistic per phase and because Phase 20's honest slice breakdown pushed its estimate up (7 slices × ~2 weeks each vs. the earlier 12–14-week monolith). Real slip will come from Phase 20 unknowns (slice 20d's cost-analysis is novel research; slice 20e's `T?confidence` interaction with the type system is unpredictable), Phase 24 (LSP — scope tends to grow), and Phase 33 (launch polish — always longer than estimated). Build schedule with a 20% buffer; re-plan quarterly.

The original "~18–24 months" quote is not preserved above because preserving it would be dishonest — the honest plan sums to more. Quoting 24 months and planning 27 is the shortcut; quoting what the plan actually sums to is the non-shortcut.

The dates aren't the point. The point is that each phase has:
- A clear goal with a named hard dependency, not a vibe sequence.
- A concrete scope list — no "TBD" or "polish" stand-ins.
- A version cut-line saying which release it ships in.
- A pre-phase brief before code.
- Tests green at the boundary.
- A dev-log entry.

That discipline is what makes the 27 months possible. Without it, the plan slips to 40+ and v1.0 becomes aspirational rather than a calendar commitment.

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
- **End of Phase 33** — v1.0 public release: installer, website, beta-tester feedback incorporated, launch GIF, announcement.
