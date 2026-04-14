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
- **Memory.** Refcount + cycle collector (Phase 15.5). Predictable release without Java pauses.
- **Deployment.** Single native binary + WASM (Phase 27, candidate for earlier) + C ABI embedding (Phase 15.5).
- **Tooling.** LSP (Phase 29), formatter, package manager (Phase 28), REPL (Phase 15.5). Polished, not novel.
- **Cross-platform.** macOS + Linux + Windows all first-class by v1.0 (Phase 32).

### Deliberately not competing

- Systems-level control — Rust / Zig win. No pointer arithmetic, no manual allocators.
- Raw hot-loop numerics — C++ / Fortran win. FFI for the ~1% of apps that need it.
- Dynamic metaprogramming — Ruby / Lisp win. Opposite trade-off to compile-time checking.
- Ecosystem size at launch — Python / JS have 20-year head starts. Python FFI (Phase 16) closes the gap.

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
- Effect-tagged `import python` → Phase 16
- Proc-macro `#[tool]` + `corvid run` user-tool loading → Phase 14
- Google / Ollama adapters → Phase 18
- Streaming `Stream<T>` → Phase 22
- Async-native concurrent multi-agent execution → revisit when Phase 25 demands it

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

**Pre-phase decisions locked**: 16-byte header (preserves payload alignment + reserves a future-use word), atomic refcount (Phase 25 multi-agent will need it; cheap insurance now), scope-driven release insertion (correct now, liveness-driven optimisation is Phase 22), combined slice (foundation + String) — then split mid-session into 12e (foundation) + 12f (String) once the String integration revealed itself as a slice's worth of work on its own.

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

#### Slice 12j — Make native the default for tool-free programs
(`corvid run` begins AOT-compiling + executing instead of interpreting where possible)

#### Slice 12k — Polish + benchmarks
(stability guarantees, perf measurements vs interpreter and vs hand-written Rust)

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

### Phase 15.5 — General-purpose table stakes (~2–3 months, may split across multiple phases)

Positioning demands that Corvid hold its own as a general-purpose language, not just "a great language for agents." These are the GP features users expect and will reject the language for lacking. Split into explicit slices below; exact phase numbering will firm up at the pre-phase chat.

- **Methods on types.** Dot-syntax `value.method()` where `method` is an associated function on `type T`. No inheritance, no late binding, no `this` — just sugar over free functions with a named receiver. The existing `type` + `agent` model stays; methods are purely ergonomic. Closes the "Corvid doesn't even have methods?" objection without the complexity Java/Python inheritance brings. Pre-phase chat must decide: single dispatch only? trait-like interfaces for abstraction? syntax for the receiver (`self`, `this`, positional)?
- **Cycle collector on top of refcount.** Current model (refcount + deterministic destructors) is correct but leaks on cycles. Python's model — refcount primary, tracing cycle collector as backstop — preserves deterministic release for the common case while handling cycles. The collector runs only when allocation pressure suggests cycles may exist. Keeps the "tools/files/LLM-sessions release promptly" property users will depend on. Out of scope: full generational tracing GC (wrong trade-off for Corvid).
- **C ABI + library mode.** Compile `.cor` code to `.so` / `.dll` / `.dylib` with a stable C-callable surface. Enables embedding Corvid in Rust, Python, Node, Go hosts. Requires: `pub extern "c"` annotation or equivalent, calling-convention choice, header generation, ownership-at-boundary rules (who frees what). The embedding story is what turns Corvid from "a tool you invoke" into "a component you ship."
- **REPL.** Interactive `corvid repl` — evaluate expressions and statements in a persistent session, redefine agents on the fly, inspect values. Sits on top of the interpreter; the AOT tier doesn't need to change. Pairs with the Phase 24 replay work — the REPL becomes the natural surface for `replay` / `step` / `inspect`.
- **Pull WASM target earlier.** Currently Phase 27; a GP language needs it sooner as both a deployment target (browsers, edge) and an embedding format. Re-slot around Phase 18–20 if scheduling allows. Pre-phase chat decides exact ordering.

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

Red lines — features explicitly rejected, not merely deferred:

- **Raw pointer arithmetic + manual allocators.** Pointer aliasing is one of the hardest things for any reasoner (human or LLM) to track, and readability-for-LLM-generated-code is a first-class design goal. Narrow `@unsafe` FFI shim for C interop is allowed; pervasive pointers are a hard no. Rust and Zig own that niche — Corvid doesn't compete there.
- **Classical OOP inheritance.** `type` + methods + interfaces/traits (Phase 15.5) are the model. Subclassing, `this`, virtual dispatch, and deep hierarchies are not. Modern GP consensus (Go, Rust, Swift, Kotlin) agrees composition + methods beat inheritance.
- **Rust/C++-level control for systems work.** Corvid aims for Go / Swift class performance. Fast enough that compute rarely bottlenecks AI-shaped software (where LLM latency dominates by three orders of magnitude), but not competing on hot-loop throughput.

Deferred, not rejected:

- **Every LLM provider at launch.** Anthropic + OpenAI ship first; Google, Ollama, and others follow in Phase 18.
- **Windows + Linux + macOS day-one.** Start on one OS (macOS), add the others in Phase 32.

What is *not* a non-goal, despite earlier framings: **being a general-purpose language.** Corvid must be one. The only question is which GP features ship when — the answer lives in the Phase 15.5 slate and the "table stakes" list in the positioning section above.

---

## Velocity markers

To keep momentum honest, ship these at the phase boundaries:

- End of Phase 11: a `corvid run` that doesn't need Python.
- End of Phase 12: a compiled binary for `hello.cor`.
- End of Phase 16: full interpreter + compiler parity on `refund_bot.cor`.
- End of Phase 20: production-grade error handling.
- End of Phase 25: multi-agent demo.
- End of Phase 32: v1.0 public release.
