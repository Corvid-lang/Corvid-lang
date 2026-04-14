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
- **Memory.** Refcount + cycle collector (Phase 17). Predictable release without Java pauses.
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

### Phase 14 — Native tool dispatch (~4–6 weeks)

**Goal.** User-written tool implementations callable from compiled Corvid code.

**Hard dep:** Phase 13 (async — tools are fundamentally I/O-bound).

**Scope:**
- Proc-macro `#[tool(name = "...")]` in a user Rust crate registers the implementation. **Registry mechanism: `inventory` crate.** Battle-tested (used by `rstest`, `iota`, many Rust frameworks), no linker-script tricks (unlike `linkme`), works uniformly across Linux / macOS / Windows / MSVC / GCC / Clang. Pre-phase chat revisits only if a concrete blocker surfaces.
- `corvid build --target=native` accepts a `--with-tools <crate>` flag; the tools crate is linked into the binary. Generated code emits a dispatch call through the runtime's `ToolRegistry`.
- `approve` tokens flow through at runtime. Runtime verifies the token matches the call site before the tool body runs. Effect checker continues to enforce that `approve` precedes the call statically; Phase 14 ensures the runtime check can't be bypassed.
- Slice 12j's `NotNativeReason::ToolCall` is lifted — tool-using programs start running native.

**Non-scope:** Prompt calls (Phase 15). Python-backed tools (Phase 30 completes that story).

### Phase 15 — Native prompt dispatch (~3 weeks)

**Goal.** `prompt ... -> T:` declarations callable from compiled Corvid code, talking to the LLM adapters live.

**Hard dep:** Phase 13 (async).

**Scope:**
- Codegen lowers `IrCallKind::Prompt` to a runtime call that (a) serialises the prompt template with interpolated args, (b) requests structured output matching the declared return type, (c) deserialises and returns.
- Return-type JSON schema derived automatically from the declared Type. No user-registry needed — prompts live in source.
- Slice 12j's `NotNativeReason::PromptCall` is lifted. Combined with Phase 14, every program from the `examples/` directory runs natively end-to-end.
- Refund-bot demo running with `corvid run examples/refund_bot_demo/src/main.cor --target=native` is the phase-boundary verification.

**Non-scope:** Multi-provider adapters beyond Anthropic + OpenAI (Phase 31).

**v0.4 cuts here.** Native tier is actually useful for real programs.

---

### Phase 16 — Methods on types (~2 weeks)

**Goal.** `value.method(args)` syntax for associated functions on user types. Cheapest, loudest GP-signal feature.

**Hard dep:** frontend (✅), IR (✅). Zero codegen changes required — methods lower to free functions.

**Scope:**
- Parser extension: **`impl T:` block** holding method declarations, separate from the `type T:` declaration. Matches Rust / Swift; keeps `type` declarations compact and data-focused; methods cluster visually; ordering independent (methods can live in a different file, enabling future package-level extension methods). Pre-phase chat revisits only if a concrete blocker surfaces.
- Resolver: dotted-method lookup resolves to the declared method's `DefId`.
- Typechecker: call-site `v.m(args)` typechecks as `m(v, args)` with `v` bound to the first parameter.
- Single dispatch only. No inheritance, no late binding, no virtual tables.
- IR: `value.method(args)` lowers to an ordinary agent/function call — no new IR variants.

**Non-scope:** Interfaces / traits (deferred; revisit at Phase 20 if the moat phase actually needs polymorphism).

### Phase 17 — Cycle collector (~4–6 weeks)

**Goal.** Backstop the refcount runtime against reference cycles. Deterministic destructor release stays the fast path; the cycle collector only catches what refcount misses.

**Hard dep:** Phase 12 (refcount runtime + native codegen).

**Scope:**
- Stop-the-world mark-and-sweep in the refcount runtime (single-threaded collection; Corvid is single-threaded through v1.0). Triggered by allocation-pressure heuristic: object-count threshold and/or bytes-allocated-since-last-collection, tunable via env var.
- Roots are live locals on the current Tokio task stacks plus runtime-owned caches. Compiler cooperation: codegen emits stack-map metadata per function (Cranelift supports this natively).
- Parity harness gains a cycle fixture: a cyclic data structure that today leaks under `CORVID_DEBUG_ALLOC=1`; after Phase 17, the collector sweeps it and the leak-counter returns to zero.

**Non-scope:** Generational GC. Concurrent collection (mutator-collector concurrency via write barriers — post-v1.0 if multi-threaded Corvid ever becomes a direction).

### Phase 18 — Result + Option + retry policies (~4 weeks)

**Goal.** Language-native error handling. `Result<T, E>`, `Option<T>`, propagation (`?`), retry syntax.

**Hard dep:** typechecker extension for generic types (moderate work — the type system already has `List<T>` machinery, `Result<T, E>` extends the same path).

**Scope:**
- `Result<T, E>` and `Option<T>` as compiler-known stdlib types. Codegen lowers as tagged unions (discriminant + payload).
- `?` operator: `expr?` short-circuits the enclosing function with the error if `Err`, unwraps the value if `Ok`.
- Retry syntax: `try <expr> on error retry N times backoff { linear ms | exponential ms }`. Desugars to a loop over the expression with sleep between attempts.
- Effect integration: tool calls that can fail return `Result` by default; `dangerous` tools return `Result` whose error type includes an `ApprovalDenied` variant.

**Non-scope:** User-defined error enums with payloads beyond simple variants — that's Phase 20's job with effect rows and richer types.

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

#### Slice 20a — Custom effects + effect rows (~3 weeks)
- [ ] Parser: `effect Name` top-level declaration. Effect rows on tool + agent signatures (`fn foo() -> T uses reads_pii, cites`).
- [ ] Resolver + typechecker: effect rows flow through the call graph. Body verified against declared effects (raising an effect not in the row = compile error). Per-effect approval policies declarable in `corvid.toml`.
- [ ] Revisits the Day-4 `Safe | Dangerous` decision — additive, no breaking change to existing code.

#### Slice 20b — Grounding + citation contracts (~2 weeks)
- [ ] `grounds_on ctx` annotation on prompts; template must reference `ctx` or raise `E0201`.
- [ ] `cites ctx` effect; return type must be `Grounded<T>` or `E0202`; template must request citations or `E0203`.
- [ ] `Grounded<T>` compiler-known stdlib type; unwrap via the `.unwrap_discarding_sources()` method (uses Phase 16 methods machinery).
- [ ] `cites ctx strictly` for runtime verification failure (code compiles; runtime checks citations against retrieved context and raises if they don't match).

#### Slice 20c — `eval ... assert ...` language syntax (~2 weeks)
- [ ] Parser + typechecker + lowering for `eval name: body ... assert expr` declarations.
- [ ] IR node `IrEval` alongside `IrAgent`.
- [ ] Runner CLI is out of scope — ships in Phase 27. This slice is language only.

#### Slice 20d — `@budget($)` cost annotations (~3 weeks)
- [ ] `@budget($0.10) agent name():` annotation parsed + typechecked.
- [ ] Compile-time upper-bound analysis over LLM + retrieval calls in the agent body. Each prompt declaration carries an estimated-cost bound; analysis sums over control-flow paths.
- [ ] Refuses to compile (`E0250`) if worst-case cost > budget. Warns (`W0251`) when the analysis can't prove a bound (e.g., unbounded recursion).
- [ ] Also ships the `@wrapping` annotation for opt-out overflow checks deferred from Phase 12.

#### Slice 20e — Uncertainty types `T?confidence` (~2 weeks)
- [ ] Syntax: `T?confidence` — `T` with a `f64` confidence tracked through expressions.
- [ ] Combining rule: confidence of `f(a?c1, b?c2)` = `min(c1, c2)` by default, overridable with `@combine_confidence fn(a, b) -> f64`.
- [ ] Runtime carries the confidence value; prompts returning `T?confidence` parse a model-reported confidence from the LLM response.

#### Slice 20f — `Stream<T>` (~2 weeks)
- [ ] `Stream<T>` as compiler-known stdlib type. Prompts + tools can declare streaming returns.
- [ ] `for x in stream:` consumes the stream. `yield` in agent bodies produces streams.
- [ ] Integrates with Phase 13's native async runtime — streams back-pressure via Tokio channels under the hood.

#### Slice 20g — Bypass tests + effect-system specification (~2 weeks)
- [ ] Property-based bypass tests proving the effect checker cannot be circumvented via FFI, generics, or indirect calls. `proptest`-driven.
- [ ] Written effect-system specification (20–40 pages): syntax, typing rules, worked examples, FFI / async / generics interactions. Related-work section covering Koka, Eff, Frank, Haskell effect libs, Rust `unsafe`, capability systems. Lives in `docs/effects-spec.md`; ships at the phase boundary alongside the code.

**Non-scope:** Runtime eval tooling CLI (Phase 27). RAG runtime infrastructure (Phase 32's `std.rag`). Custom effect annotations on Python FFI imports richer than `effects: <name>` (Phase 30 ships basic; richer stays here).

### Phase 21 — Replay (~4–5 weeks) — **THE FLAGSHIP WOW**

**Goal.** Every run replayable by construction. The feature that ships in the v1.0 demo video.

**Hard dep:** Phases 14–15 (tool + prompt calls must exist to be worth recording). Runtime tracing infrastructure (✅ baseline from Phase 11).
**Soft dep:** Phase 20. Replay doesn't structurally depend on custom effects — it records tool / prompt / approve / seed / time calls regardless of effect category. Paired with Phase 20 at v0.6 for release-narrative reasons: "the moat you can reason about PLUS the moat you can replay" ships as one cut.

**Scope:**
- Runtime records every LLM call, tool call, approve decision, random seed, time-source read into a structured trace.
- `corvid replay <trace-id>` re-executes the program substituting recorded responses for live calls. Deterministic — given the same trace, replay produces byte-identical state transitions.
- Replay works in both interpreter and native tiers (shared recording format). WASM replay lands alongside WASM in Phase 23.
- Cost-zero re-runs: replaying 10,000 times costs $0 in LLM spend.
- Command-line UX ships in this phase: `corvid replay <trace>` deterministic re-run. Scrub-backward / step-forward interactive UX is a followup in the post-v1.0 bucket (uses the same infrastructure).

**Non-scope:** Scrub-backward interactive debugger (post-v1.0). Trace visualization UI (post-v1.0). WASM replay (deferred to Phase 23's WASM scope — same recording format, needs host-function bindings).

**v0.6 cuts here.** Moat phase + flagship wow feature land together. Corvid becomes unignorably different.

---

### Phase 22 — C ABI + library mode (~6–8 weeks)

**Goal.** Embed Corvid in Rust, Python, Node, Go hosts. Corvid becomes a component, not only a tool.

**Hard dep:** Phase 12 (native codegen).
**Soft dep:** Phase 17 (cycle collector). C ABI without the cycle collector means embedders who build cyclic data across the boundary leak — exactly the same behaviour every pre-Phase-17 Corvid program has. Not a compilation blocker, but pairing with Phase 17 at the same release is the honest story: the v0.7 pitch is "Corvid ships as a library" and shipping a leaking library would undercut that.

**Scope:**
- `pub extern "c"` annotation on agent declarations. Codegen emits C-callable wrappers with stable calling conventions.
- `corvid build --target=cdylib` produces `.so` / `.dll` / `.dylib`. `--target=staticlib` produces `.a` / `.lib`.
- `corvid embed --header` generates a C header describing the exported surface.
- Ownership-at-boundary rules documented: who frees what, who retains what. Enforced at compile time (effect-checker-adjacent analysis on extern signatures).
- Reference host bindings land in-tree for Rust + Python (Python via CPython's C API, not PyO3 — PyO3 integration is Phase 30's separate problem).

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

**v1.0 cuts here. Launch day.**

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
