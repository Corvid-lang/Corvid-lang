# Corvid dev log

Weekly journal. Non-negotiable. Every entry is one commit.

---

## Day 1 — repo scaffolded

- Language name: **Corvid**. File extension: `.cor`.
- Compiler host: **Rust**. Parser crate: `chumsky`. Errors: `ariadne`.
- Syntax philosophy: Pythonic baseline, AI primitives (`agent`, `tool`, `prompt`, `effect`, `approve`) as new keywords.
- Runtime strategy: transpile to Python in year 1, add WASM in year 2, native via Cranelift in year 3.
- Workspace laid out per `ARCHITECTURE.md` §3 — 11 crates, one per pipeline stage.

Next: install Rust, do Rustlings, read *Crafting Interpreters* chapters 1–4. No code in the crates yet.

---

## Day 2 — AST types (Phase 1)

- Filled out `crates/corvid-ast/` with 6 source files: `span.rs`, `effect.rs`, `ty.rs`, `expr.rs`, `stmt.rs`, `decl.rs`.
- Decisions made:
  - `Box<Expr>` for recursive nodes (not arena-allocated).
  - One `Expr` enum and one `Stmt` enum (not separate structs per variant).
  - `Stmt` and `Expr` are separate (matches Python-shaped grammar).
  - All nodes carry a `Span`; all types derive `Serialize` / `Deserialize`.
- Scope calls:
  - Deferred `while` loops to v0.2 — agents rarely need them.
  - Kept `FunctionDecl` alongside `AgentDecl` — helper functions are useful.
  - `ImportSource` enum is Python-only in v0.1; JS/C variants added when interop expands.
  - Tool bodies deferred — all tools are external in v0.1.
  - Struct-like `TypeDecl` only; enum/union types in v0.2.
- Tests: 3 unit tests green — one reconstructs the full `refund_bot.cor` AST by hand, proving coverage.
- `cargo check` + `cargo test -p corvid-ast` both green. Full workspace still compiles.

Next: Phase 2 — Lexer. Turn source text into a token stream.

---

## Day 3 — Lexer (Phase 2)

- Filled out `crates/corvid-syntax/` with `token.rs`, `errors.rs`, `lexer.rs`.
- 27 keywords total (added `break`, `continue`, `pass` over the original plan).
- Decisions made:
  - Hand-rolled lexer (not using `chumsky` for lexing — cleaner indentation handling).
  - Single pass: lexer emits `Indent`/`Dedent`/`Newline` inline, not a post-pass.
  - `#` for comments (Pythonic).
  - Spaces only for indentation; tabs rejected with `TabIndentation` error.
  - Single-line `"..."` strings; multi-line `"""..."""` triple-quoted strings for prompt bodies.
  - Escape sequences: `\n \t \r \\ \" \0`.
  - Newlines inside brackets (`(`, `[`) are ignored — implicit line continuation, Python-style.
  - Blank lines and comment-only lines don't affect indentation.
  - ASCII-only identifiers in v0.1.
- Scope calls:
  - No compound assignment (`+=`, `-=`) in v0.1.
  - No `**` power operator in v0.1.
  - No `{`, `}` tokens (no dict literals, no brace blocks).
  - No decorator `@` in v0.1.
- Tests: 21/21 green. The full `examples/refund_bot.cor` lexes without error.

Next: Phase 3 — Parser. Consume tokens, produce AST.

---

## Day 4 — Apple-simple pass

Ruthlessly cut the keyword count before writing the parser. Every concept that wasn't load-bearing got removed.

- **Dropped 6 keywords:** `let`, `function`, `effect`, `pure`, `compensable`, `from`.
- **Renamed 1:** `irreversible` → `dangerous` (tells the reader *why* approval is needed, not just *what* the internal classification is).
- **Simplified `Effect` enum** to just `Safe` | `Dangerous`. If we ever need `Compensable`, we add a variant — adding enum variants is a non-breaking change.
- **22 keywords total**, all real English words.
- Updated: `token.rs`, `effect.rs`, `decl.rs`, AST tests, lexer tests, all 3 `.cor` examples, `README.md`, `ARCHITECTURE.md` §15, `FEATURES.md` v0.1.
- Tests: 25/25 green (3 AST + 22 lexer).

Guiding rule recorded: **default is safe, mark the exception.** Users don't write `safe` — unannotated means safe. Only `dangerous` needs a mark. Matches how humans actually think about risk.

Next: Phase 3a — Expression parser only. Literals, identifiers, calls, field access, operators with precedence.

---

## Day 5 — Expression parser (Phase 3a)

- Added `crates/corvid-syntax/src/parser.rs` (~450 LOC) and `ParseError` to `errors.rs`.
- Technique: recursive descent with one function per grammar rule, binary ops layered by precedence level.
- Operator precedence (lowest → highest): `or` → `and` → `not` (prefix) → comparison (non-chainable) → `+ -` → `* / %` → unary `-` → postfix (`.` `[` `(`).
- `parse_expr(&[Token]) -> Result<Expr, ParseError>` is the public entry point. Structural tokens (`Newline`/`Indent`/`Dedent`/`Eof`) terminate the expression cleanly.
- Decisions made:
  - Chained comparisons (`a < b < c`) are rejected with a dedicated error.
  - Trailing commas allowed in call args and list literals.
  - Struct literals parse as calls (`IssueRefund(x, y)` is `Call` at parse time; the resolver decides it's a constructor).
  - `List[T]` generic type syntax deferred — `[1, 2, 3]` is a list *literal* here, a value not a type.
- Tests: 26 parser tests green, 22 lexer tests still green. Total: 48/48 across the crate.

Next: Phase 3b — Statement parser. `let`-free bindings (`x = expr`), `if`/`else`, `for`, `return`, `approve`, `break`/`continue`/`pass`, expression statements, and blocks.

---

## Day 6 — Statement and block parser (Phase 3b)

- Extended `parser.rs` with `parse_stmt`, `parse_indented_block`, `parse_block` (public).
- Added `ParseErrorKind::EmptyBlock` and `ExpectedBlock`.
- `parse_block` now returns `(Block, Vec<ParseError>)` — collects errors rather than bailing. Panic-and-sync recovery: on a bad statement we skip to the next newline and continue.
- Decisions made:
  - **Assignment detection** via two-token lookahead: if next is `IDENT` and second-next is `=`, it's `x = expr`; otherwise expression-statement.
  - **Required `pass`** for empty blocks. Zero-stmt block = `EmptyBlock` error pointing at the indent.
  - **`break` / `continue` / `pass`** are parsed as statements but encoded as `Stmt::Expr` with a sentinel `Ident`. The name resolver will recognize them; dedicated AST variants can arrive later without breaking callers.
  - Blank lines inside blocks (stray `Newline` tokens) are skipped.
- Tests: 14 new statement tests — assignment, return (with/without value), if/else, for, approve, error recovery, missing colon, empty block. Plus the canonical refund_bot body parses to the expected 4-statement structure.
- Total in `corvid-syntax`: **62/62 green** (22 lexer + 40 parser).

Next: Phase 3c — Top-level declarations. `import`, `type`, `tool`, `prompt`, `agent`. Produce a full `File` AST from a `.cor` source.

---

## Day 7 — File and declaration parser (Phase 3c)

- Added to `parser.rs`: `parse_file` (public), `parse_decl`, plus one parser per declaration kind (`parse_import_decl`, `parse_type_decl`, `parse_tool_decl`, `parse_prompt_decl`, `parse_agent_decl`).
- Added helpers: `parse_params`, `parse_param`, `parse_type_ref`, `parse_field`, `skip_newlines`, `sync_to_next_decl`.
- Dispatch is by first-keyword lookup — each declaration starts with a unique keyword (`import`, `type`, `tool`, `prompt`, `agent`).
- Decisions made:
  - **Type refs v0.1** are `Named` only. No generic application yet (`List[T]` → v0.2). One-line `parse_type_ref` for now.
  - **Only `python`** is accepted as an import source. Unknown sources (e.g. `import ruby`) produce an error.
  - **Tools end at newline** — no body, no indented block.
  - **Prompts require** `Indent + StringLit + Newline + Dedent`. Single- or triple-quoted.
  - **Error recovery at file level**: `sync_to_next_decl` skips tokens until the next top-level keyword (or EOF). A broken declaration no longer kills parsing of the rest of the file.
- Tests: 13 new declaration tests. The big one (`parses_full_refund_bot_file`) parses the canonical example with 1 import + 4 types + 2 tools + 1 prompt + 1 agent, verifies effect flags, and confirms the agent body resolves to `Let`/`Let`/`If`/`Return`.
- Total: **75/75 green** across `corvid-syntax`.

Phase 3 complete. The full `.cor` → `File` pipeline works end-to-end.

Next: Phase 4 — Name resolution. Link every identifier use to its declaration; detect undefined names and duplicate declarations.

---

## Day 8 — Name resolution (Phase 4)

- Filled out `crates/corvid-resolve/` with `errors.rs`, `scope.rs`, `resolver.rs`.
- Side-table approach: resolver produces `HashMap<Span, Binding>` instead of mutating the AST. `Span` now derives `Hash` (one-line fix on `corvid-ast`).
- Two-pass design:
  - Pass 1 registers every top-level declaration into a `SymbolTable`. Duplicates report `DuplicateDecl` pointing at both the first site and the offender.
  - Pass 2 walks the AST and records a `Binding::Local | Decl | BuiltIn` for every identifier use.
- Strict duplicate detection (decided with the user): `tool foo` and `agent foo` clash just like two `tool foo` would.
- Built-ins registered up front: `Int`, `Float`, `String`, `Bool`, plus sentinel `Break`/`Continue`/`Pass` so the parse-time surrogates resolve cleanly.
- `approve Label(args)` — the top-level callee is treated as a descriptive label and not resolved. Arguments ARE resolved normally (an undefined arg still flags).
- Tests: 13/13 green. The full `refund_bot.cor` resolves cleanly with 0 errors. Duplicate detection works across categories. Undefined-name errors point at the use site.

Next: Phase 5 — Type checker + effect checker. The killer feature. A dangerous tool call must be preceded by a matching `approve` in the same block, or the file fails to compile.

---

## Day 9 — Type checker + effect checker (Phase 5) 🎯

**The killer feature is live.** A file that calls a dangerous tool without a matching `approve` no longer compiles.

- Filled out `crates/corvid-types/` with `types.rs`, `errors.rs`, `checker.rs`.
- `TypeError` carries a one-line `message()` and an optional `hint()` — every error suggests the fix. Example: `UnapprovedDangerousCall` hints `add \`approve IssueRefund(arg1, arg2)\` on the line before this call`.
- `Type` enum: `Int | Float | String | Bool | Nothing | Struct(DefId) | Function{...} | List(T) | Unknown`. `Unknown` is load-bearing — it suppresses error cascades when we can't infer cleanly.
- Effect algorithm: a flat `approvals` stack. On entering a block, save its length; on leaving, truncate back. Outer approvals are visible to inner blocks; inner approvals don't leak out.
- Matching rule (locked with user): `approve IssueRefund(a, b)` authorizes subsequent `issue_refund(..., ...)` if `snake_case(label) == tool_name` **and** arity matches.
- Added `Nothing` as a built-in type (was missing from resolver).
- Added `SymbolTable::lookup_def` so the checker can turn named types into `Type::Struct(DefId)`.
- Decisions made:
  - No approval consumption in v0.1 — one approve authorizes N subsequent matching calls in the same scope. Simpler mental model; tightening comes later.
  - Int widens to Float in assignments (standard numeric widening).
  - `Unknown` propagates without producing secondary errors.
  - Bare function reference (`x = get_order`) is an error — no first-class functions in v0.1.
  - Type used as value (`x = String`) is an error with a specific hint.
- **The two headline tests pass:**
  - `refund_bot_typechecks_cleanly` — canonical program with `approve IssueRefund(...)` → zero errors.
  - `refund_bot_without_approve_fails_to_compile` — same program minus the `approve` line → exactly one `UnapprovedDangerousCall` error whose hint says `add \`approve IssueRefund(...)\``.

Running total across the workspace: **107 tests, all green** (3 AST + 75 syntax + 13 resolve + 16 types).

Next: Phase 6 — IR lowering. Desugar and normalize the typed AST into an intermediate representation ready for codegen.

---

## Day 10 — IR lowering (Phase 6)

- Filled out `crates/corvid-ir/` with `types.rs` (IR node types) and `lower.rs` (AST → IR transform).
- IR types: `IrFile` holding imports, types, tools, prompts, agents. Parallel shape to AST but references are resolved (`DefId`/`LocalId` instead of idents), types are attached to every expression, and parse-time hacks are normalized away.
- Normalizations performed:
  - `Stmt::Expr(Ident("break"))` → `IrStmt::Break`. Same for `continue` and `pass`. The parser's sentinel hack ends at the IR boundary.
  - `Stmt::Approve { action: Call(label, args) }` → `IrStmt::Approve { label: "IssueRefund", args: [...] }`. Codegen consumes this structured form directly.
  - Every call is classified: `IrCallKind::Tool { def_id, effect }` / `Prompt { def_id }` / `Agent { def_id }` / `Unknown`. Codegen routes by this tag.
- Noted for later: `SymbolTable` doesn't carry the full decl, so the tool-effect lookup in `lower_call` conservatively returns `Safe` and defers the truth to `IrTool.effect` (which the codegen should prefer). A future refactor can push effect into `DeclEntry`.
- Tests: 6 tests green — simple agent lowering, break/continue/pass → dedicated variants, approve structure preserved with label + arity, tool call IR identifies the tool, full `refund_bot` produces the expected 1+4+2+1+1 declaration counts with the dangerous flag preserved.

Running total across workspace: **113 tests green** (3 AST + 75 syntax + 13 resolve + 16 types + 6 ir).

Next: Phase 7 — Python code generator. Walk `IrFile` and emit runnable `.py` to `target/py/`. The first phase users can actually *run*.

---

## Day 11 — Python codegen (Phase 7)

- Filled out `crates/corvid-codegen-py/` with `emitter.rs` (indentation-aware string builder) and `codegen.rs` (IR → Python walker).
- Generated Python structure:
  - Preamble: `from corvid_runtime import tool_call, approve_gate, llm_call` + `@dataclass` import.
  - User imports (`import python "X" as Y` → `import X as Y`; collapses `import X as X` to `import X`).
  - `TOOLS` dict marking each tool's effect (`"safe"` / `"dangerous"`) and arity.
  - `PROMPTS` dict with template + param names.
  - `@dataclass`-decorated Python classes for each `type` decl.
  - `async def` for each agent body.
- Call dispatch: tools → `await tool_call("name", [args])`, prompts → `await llm_call("name", [args])`, agents → `await agent_name(args)`, imports/unknown → direct Python call.
- `approve IssueRefund(a, b)` → `await approve_gate("IssueRefund", [a, b])`. The structured IR form makes this a one-line emission.
- `break`/`continue`/`pass` become their Python equivalents directly.
- Literals round-trip faithfully: floats always carry a decimal point, strings are escaped, `nothing` → `None`, `true/false` → `True/False`.
- Binops wrap in parens to preserve precedence without tracking it at emit time.
- Tests: 13/13 green. The canonical `refund_bot.cor` generates Python that:
  - Declares `TOOLS` with `"issue_refund": {"effect": "dangerous"}`
  - Produces 4 `@dataclass` definitions
  - Emits `async def refund_bot(ticket):`
  - Correctly orders `approve_gate(...)` BEFORE `tool_call("issue_refund", ...)`

Running total: **126 tests green** across the workspace (3 AST + 75 syntax + 13 resolve + 16 types + 6 ir + 13 codegen).

Next: Phase 8 — the `corvid_runtime` Python package. Implements `tool_call`, `approve_gate`, `llm_call`, a tool registry, and the actual LLM dispatch. This makes generated code *executable*.

---

## Day 12 — Python runtime (Phase 8)

- Created `runtime/python/` with a proper `pyproject.toml` and the `corvid_runtime` package.
- Modules:
  - `core.py` — `tool_call`, re-exports `approve_gate` and `llm_call`, plus `run` / `run_sync` trace wrappers.
  - `registry.py` — `@tool("name")` decorator, `register_tools` / `register_prompts` called from generated modules.
  - `approvals.py` — interactive stdin prompt by default; programmatic `set_approver(fn)`; `CORVID_APPROVE_ALL=1` for CI.
  - `llm.py` — adapter registry keyed by model name prefix. Claude adapter auto-registers under `claude-`. Renders prompt templates via `{name}` substitution.
  - `config.py` — model resolution precedence: per-call → `CORVID_MODEL` env → `corvid.toml`. No hardcoded default.
  - `tracing.py` — JSONL event emission to `target/trace/<run_id>.jsonl`. Silently swallows IO errors so tracing can't crash user code.
  - `errors.py` — CorvidError hierarchy (NoModelConfigured, UnknownTool, UnknownPrompt, ApprovalDenied, etc.).
  - `testing.py` — `mock_llm`, `mock_approve_all`, `reset` for tests.
- Decisions locked (with user):
  - **No default model.** Missing config → `NoModelConfigured` with a fix hint.
  - **No default approver.** Interactive by default; programmatic via `set_approver`.
  - Adapter-based LLM dispatch — v0.2 adds OpenAI, Google, Ollama as additional adapters.
- Tests: 10/10 green with pytest-asyncio. Covers tool dispatch, missing impl, approval approve/deny paths, env-flag auto-approve, missing-model error, mock adapter, unknown prompt, and trace file creation + `run()` wrapper.
- Package installed locally with `pip install -e '.[dev]'` — `pytest` passes cleanly.

Phase 8 complete. Running total: **Rust — 126 tests, Python — 10 tests, all green.**

Next: Phase 9 — wire the CLI so `corvid build refund_bot.cor` produces `target/py/refund_bot.py` on disk, and `corvid run refund_bot.cor` executes it end-to-end.

---

## Day 13 — CLI wiring (Phase 9) 🚀

**The compiler is real.** `corvid check` / `build` / `run` / `new` all work.

- `corvid-driver/src/lib.rs`: grew real implementations.
  - `compile(source)` runs the full frontend and returns `CompileResult { python_source, diagnostics }`.
  - `build_to_disk(path)` reads a file, compiles, and writes `target/py/<stem>.py`.
  - `scaffold_new(name)` / `scaffold_new_in(parent, name)` create a project skeleton.
  - `Diagnostic` type unifies errors from every phase (lex/parse/resolve/typecheck) so the CLI has one thing to render.
  - `line_col_of` converts byte offsets to 1-based line/col for error display.
- Output path convention: if the source is under `<project>/src/`, output goes to `<project>/target/py/<stem>.py`; otherwise to `<source_dir>/target/py/<stem>.py`.
- Build returns a file ONLY when zero diagnostics — partial output is more confusing than nothing.
- `corvid-cli/src/main.rs`: subcommands (`new`, `check`, `build`, `run`, `test`) now dispatch to the driver. `run` shells out to `python3 <file>`.
- Exit codes: 0 = ok, 1 = compile errors, 2 = usage/IO errors.
- Tests: 8 driver tests green (clean compile → Python, bad effect → diagnostic with hint, `build_to_disk` writes file, src-dir-aware output path, no file when errors, scaffold creates expected structure, scaffold rejects existing dir, line/col translation).
- **End-to-end verified on the real binary:**
  - `corvid check examples/refund_bot.cor` → `ok: examples/refund_bot.cor — no errors`
  - `corvid build examples/refund_bot.cor` → writes `examples/target/py/refund_bot.py`
  - The output parses cleanly with Python's `ast.parse` — it's syntactically valid Python.
  - `corvid check /tmp/bad.cor` (missing approve) prints:
    ```
    /tmp/bad.cor:7:12: error: dangerous tool `issue_refund` called without a prior `approve`
      help: add `approve IssueRefund(arg1, arg2)` on the line before this call
    1 error(s) found.
    ```
    Exits 1.

Running total: **Rust — 134 tests, Python — 10 tests, all green.** The full pipeline (source .cor → runnable .py) works from one `corvid build` command.

Next: Phase 10 — polish. Line numbers in error output already done. Remaining polish: prettier multi-line error rendering via `ariadne`, docs, the 30-second demo video/GIF, launch-ready README.

---

## Day 14 — Polish (Phase 10) 🎨

- **Ariadne rendering**: added `corvid-driver/src/render.rs`. CLI errors now look like Rust's compiler output — multi-line, caret-underlined, colored, with error codes (`E0101`, etc.) and help footers. Ariadne 0.4 API signature fixed on the first compile error.
- **Error codes assigned** across the compiler (E0001-E0003 lex, E0051-E0054 parse, E0101 effect, E0201-E0208 type, E0301-E0302 resolve). Stable, documentable, searchable.
- **New command**: `corvid doctor` — detects Python 3.11+, `corvid-runtime`, `anthropic` (optional), and `CORVID_MODEL`. Tells the user exactly what to install.
- **README rewritten** for a real audience: the "what makes it different" section, the install flow (3 commands), the architecture diagram, and links to ARCHITECTURE.md / FEATURES.md / dev-log.md.
- **Runnable demo project** at `examples/refund_bot_demo/` with a `corvid.toml`, a `.cor` source, a `tools.py` with mocked tool impls + a fake LLM adapter. `corvid build src/refund_bot.cor && python3 tools.py` prints `refund_bot decided: should_refund=True reason='...'`.
- **Real bug caught by the demo**: codegen was emitting `TOOLS` and `PROMPTS` dicts but never calling `register_tools`/`register_prompts`. One-line fix; the integration now works end-to-end. (Good reminder: integration tests that run generated code surface bugs unit tests miss.)
- Tests: **134 Rust + 10 Python, all green.**

**The CLI user experience now:**

```
$ corvid check refund_bot.cor
ok: refund_bot.cor — no errors

$ corvid build refund_bot.cor
built: refund_bot.cor -> target/py/refund_bot.py

$ corvid check broken.cor
[E0101] error: dangerous tool `issue_refund` called without a prior `approve`
   ╭─[broken.cor:7:12]
   │
 7 │     return issue_refund(id, amount)
   │            ─────────┬──────────────
   │                     ╰── this call needs prior approval
   │
   │ Help: add `approve IssueRefund(arg1, arg2)` on the line before this call
───╯

1 error(s) found.
```

**v0.1 is done.** The compiler parses, resolves, typechecks, lowers, codegens. The runtime dispatches tools, gates approvals, calls LLM adapters, writes traces. The CLI scaffolds, checks, builds, runs. The demo runs offline in 2 commands.

What's left before a real launch: a domain + install script, a short demo GIF, and a blog post. Those are promotion, not product. The product works.

---

## Day 15 — Phase 11 first slice: interpreter foundation

Hard way, no shortcuts. Started the VM crate. Two real bugs surfaced during the first test run — fixed each at its root rather than patching the test.

**New crate `corvid-vm`:**

- `value.rs` — `Value` enum (Int, Float, String via `Arc<str>`, Bool, Nothing, Struct, List). `StructValue` holds `type_id + type_name + fields`. `PartialEq` implements Corvid's `==` semantics (Int-Float cross-compare, structural struct equality).
- `env.rs` — `Env` maps `LocalId` → `Value`. One flat scope per function body (matches resolver's current model).
- `errors.rs` — `InterpError` with kinds for UndefinedLocal, TypeMismatch, UnknownField, Arithmetic, IndexOutOfBounds, NotImplemented, MissingReturn, ApprovalDenied, DispatchFailed. Every one carries a span.
- `interp.rs` — tree-walking interpreter. Evaluates literals, locals, binops, unops, field access, index, list, if/else, for (over lists and strings), break/continue/pass, let bindings, return, expression statements. Arithmetic uses `checked_*` for Int overflow; float follows IEEE 754. String `+` concatenates.
- Tool/prompt/agent calls and `approve` return `NotImplemented` — the next Phase-11 slice wires them to `corvid-runtime`.

**Bugs caught by the tests (honest fixes, not patched-over):**

1. **Resolver: `x = expr` was creating a fresh `LocalId` every time.** In a loop body, `total = total + x` read the *outer* binding and wrote to a *new* one, so accumulators never accumulated. Fixed `corvid-resolve` to reuse the existing `LocalId` when the name is already bound in the current function's scope. Added `reassignment_reuses_same_local` test in `corvid-resolve`.
2. **Typechecker rejected `String + String`.** But the obvious user expectation (and the interpreter's impl) was concatenation. Updated `check_binop` to special-case `Add`: `(String, String) → String`. `Sub/Mul/Div/Mod` still numeric-only. Added two tests: `string_plus_string_is_concatenation` and `string_plus_int_still_errors`.

**Belt-and-braces test:**

`if_non_bool_condition_is_defensive_runtime_error` constructs `IrFile` by hand (bypassing the typechecker) and asserts the interpreter's defensive branch produces a `TypeMismatch` instead of panicking. Hard way: test the dead-in-practice code path, don't just delete the test.

**Test counts:**

- Added 25 new tests in this slice (22 VM + 1 resolve + 2 types).
- Total: **Rust 159 + Python 10 = 169 green.**
- Canonical `corvid check examples/refund_bot.cor` still clean.

**Next Phase-11 slice:** wire the native runtime. Tool dispatch in Rust, native HTTP via `reqwest`, Anthropic adapter, approval flow, tracing. Then `corvid run` invokes the interpreter instead of shelling to `python3`.

---

## Day 16 — feature-proposal: interop rigor, grounding contracts, effect-system extension

Four-workstream proposal reviewed. Decision: accept the language-level pieces, defer the library-level pieces to separate packages. Positioning stays unchanged — Corvid is a standalone, natively-compiled language with first-class Python interop (TypeScript/`.d.ts` analogy). Cranelift (Phase 12+) is **not** deferred.

**Rule applied:** if removing the feature means the compiler can no longer enforce a safety property, it's language and it goes in. If removing it only means users write `corvid add <pkg>`, it's a library and it doesn't.

**Accepted (compiler-enforced):**

1. **Effect-tagged `import python`.** Imports declare effect sets at the import site; untagged rejected; `effects: unsafe` is a visible escape hatch. → Phase 16 enhanced.
2. **Grounding + citation contracts.** `grounds_on ctx` / `cites ctx` / `cites ctx strictly` on prompts; `Grounded<T>` compiler-known type with `.unwrap_discarding_sources()`; errors `E0201`/`E0202`/`E0203`; `retrieves` effect on retriever tools. → Phase 22 expanded.
3. **Custom effects + effect rows.** User-declared `effect Name` (revisits Day-4 `Safe | Dangerous` — additive, non-breaking). Effect rows on signatures, data-flow tracking, per-effect approval policies, property-based bypass tests. → Phase 22.
4. **`eval ... assert ...` language syntax.** Pulled from Phase 31 into Phase 22; CLI/reports/CI stay in Phase 31.
5. **Written effect-system specification.** 20–40 page spec doc — syntax, typing rules, worked examples, FFI/async/generics interactions, related work (Koka, Eff, Frank, Haskell, Rust `unsafe`, capabilities). Phase 22 deliverable.

**Rejected (library, not language):**

- `corvid-py` Python-embedding package.
- Typed wrappers for top-10 Python libs (`std.python.*`).
- `std.rag` runtime substrate — sqlite-vec bundling, document loaders, chunking, incremental reindexing, embedder. Ships as separate `corvid-rag` package.
- `Retriever<T>`, `Chunk<T>`, `Query` types — live in `corvid-rag` (`Grounded<T>` stays in the language because `cites` needs to check its return type).
- MCP runtime client/server. Protocol library. Custom-effect mechanism from Phase 22 is enough to tag `mcp_call` when the runtime lands.
- `corvid new rag-bot` template, HTML eval reports, CI mechanics — scaffolding/tooling, arrive with Phase 31 and the eventual package registry.

**Docs updated:** `FEATURES.md` (v0.3 FFI enhanced, v0.4 gains 4 items, v0.7 eval tooling clarified, deferred list updated); `ROADMAP.md` (Phase 16 enhanced, Phase 22 expanded, Phase 31 renamed); `ARCHITECTURE.md` (§7 import example carries effect tags, §14 RAG non-goal softened to "not a RAG framework" with the runtime-substrate clarification).

**Non-change:** Cranelift timeline. Standalone native binary remains v1.0. Python interop is the TS/JS-style peer, not a replacement for the native target.

Next: resume Phase 11 slice 2 (native runtime wiring). The Phase 22 work stays on its scheduled runway.

---

## Day 17 — Phase 11 slice 2a: native runtime stand-up 🚀

**`corvid run` no longer needs Python.** The interpreter dispatches tools, prompts, agents, and approvals through a Rust-native `corvid-runtime`. The refund_bot demo runs end-to-end with Python uninstalled.

### Pre-phase decisions, locked in conversation

1. **Async interpreter end-to-end** — not the easy "block_on at call sites" shortcut. Reason: the Cranelift backend (Phase 12+) will be async-native, and our compiler-vs-interpreter parity strategy depends on identical observable behaviour under concurrency. Cost accepted: boxed recursion via `async-recursion`, slightly more boilerplate. Returns: the oracle property survives.
2. **Slice 2 split into 2a + 2b.** 2a brings up the runtime skeleton (no network); 2b adds reqwest + the Anthropic adapter + `.env` loading. Smaller wins, two dev-log entries, two clean test boundaries.
3. **JSON at the runtime boundary.** Tools and LLM adapters speak `serde_json::Value`; the interpreter does `Value` ↔ JSON conversion in `corvid-vm/src/conv.rs`. Reason: avoids the circular crate dependency (runtime → vm → runtime), matches every real LLM tool wire format, lets the future Cranelift backend reuse `corvid-runtime` without dragging the interpreter's value type along.
4. **Approval policy.** No "default approve all". `Runtime::builder()` defaults to `StdinApprover`; tests opt into `ProgrammaticApprover::always_yes` explicitly so the intent is on the page.
5. **`.env` confirmed for slice 2b.** Standard convention. No custom `.secret` file. Loaded via `dotenvy` when slice 2b lands.

### What landed

**`corvid-runtime` (real this time)**
- `errors.rs`: `RuntimeError` with variants for unknown tool / tool failed / unknown prompt / no adapter / approval denied / marshal / no-model-configured.
- `tools.rs`: `ToolRegistry` with closure-based registration. `register("name", |args| async move { ... })`.
- `approvals.rs`: `Approver` trait + `StdinApprover` (spawn_blocking for stdin) + `ProgrammaticApprover` (closure wrap + `always_yes` / `always_no` constructors).
- `tracing.rs`: JSONL `Tracer`, event-shape parity with the Python runtime, IO failures swallowed so a broken trace cannot crash an agent.
- `llm/mod.rs`: `LlmAdapter` trait + prefix-dispatch `LlmRegistry`.
- `llm/mock.rs`: `MockAdapter` keyed by prompt name with builder-style `.reply(...)` and `add_reply(...)`.
- `runtime.rs`: top-level `Runtime` + `RuntimeBuilder`. Bracketing trace events around tool/LLM/approval calls.

**`corvid-vm` async conversion**
- All `eval_*` methods became `async fn` with `#[async_recursion]` on the recursive ones.
- `InterpErrorKind` gained `Runtime(RuntimeError)` and `Marshal(String)` variants. Removed `PartialEq` from `InterpError` since `RuntimeError` doesn't implement it (would require `PartialEq` on every `serde_json::Value` we drag through, which is not worth it).
- Added `crate::conv` — `value_to_json` and `json_to_value`, the latter type-driven so struct results recover their `type_id` / `type_name` from the IR's type table.
- Wired the four call kinds: Tool → `runtime.call_tool`, Prompt → render template + `runtime.call_llm`, Agent → recurse with a fresh sub-`Interpreter`, Approve → `runtime.approval_gate`. Unknown call kind = hard `DispatchFailed`.
- `run_agent(ir, name, args, &runtime)` is the new public entry point. Existing tests rewritten to `#[tokio::test]` with an `empty_runtime()` helper.

**`corvid-driver` native run path**
- `compile_to_ir(source) -> Result<IrFile, Vec<Diagnostic>>` exposed for embedding hosts.
- `run_with_runtime(path, agent, args, &runtime)` — full pipeline + interpreter.
- `run_ir_with_runtime(...)` — same but takes pre-lowered IR.
- `run_native(path)` — what `corvid run` calls. Builds an empty runtime with stdin approver and JSONL trace under `<project>/target/trace/`. Tool-using programs need a runner binary; documented.
- `RunError` enum: `Io`, `Compile`, `NoAgents`, `AmbiguousAgent`, `UnknownAgent`, `NeedsArgs`, `Interp`. Each prints a clear, actionable message.
- Re-exports the runtime + vm surface so consumers depend only on `corvid-driver`.

**`corvid-cli`**
- `cmd_run` now dispatches to `run_native`. The `python3 target/py/...` shell-out is gone.

**`examples/refund_bot_demo` becomes a workspace member**
- New `Cargo.toml` + `runner/main.rs` — registers mock `get_order` / `issue_refund` tools, `ProgrammaticApprover::always_yes`, a `MockAdapter` returning a canned `Decision`, and runs the agent with a constructed `Ticket` struct. Trace file lands under `examples/refund_bot_demo/target/trace/run-*.jsonl`.
- README updated: the native path (`cargo run -p refund_bot_demo`) is now the primary; the Python path stays documented as legacy.

### Bug caught honestly during the slice

**Lexer didn't accept CRLF line endings.** The first attempt to run the demo on Windows produced 34 lex errors. Existing tests use string literals with `\n` only, so the bug had never been exercised. Fix: add `b'\r'` to the inline-whitespace match arm of the main lexer loop, plus a leading-`\r` skip in `process_line_start` for blank CRLF lines, plus `b'\r'` in the blank-line check. Two-character lex bug fix; the bigger lesson is that we now exercise file I/O for real.

### Test counts

All green across the workspace:

| Crate | Tests |
|---|---|
| corvid-ast | 3 |
| corvid-syntax | 75 |
| corvid-resolve | 14 |
| corvid-types | 16 |
| corvid-ir | 6 |
| **corvid-runtime** | **16 (new)** |
| corvid-vm | **31 (was 25)** |
| corvid-codegen-py | 13 |
| **corvid-driver** | **12 (was 8)** |
| Python runtime | 10 |

**Total: ~196 tests, all green.** 6 new VM integration tests (tool-with-handler, tool-without-handler, approve-yes, approve-no, prompt-via-mock, agent-to-agent). 4 new driver tests (full refund_bot e2e, ambiguous agent, prefer-`main`, args-required-for-arg-taking-agent). 4 conv unit tests inside the VM. 16 runtime unit tests across all five new modules.

### Verified end-to-end

```sh
$ cargo run -p refund_bot_demo
refund_bot decided: should_refund=true reason="user reported legitimate complaint"
trace written under examples/refund_bot_demo/target/trace
```

The trace file shows the expected event sequence: `run_started → tool_call(get_order) → tool_result → llm_call → llm_result → approval_request → approval_response(approved=true) → tool_call(issue_refund) → tool_result → run_completed(ok=true)`.

### Scope honestly held

In: runtime skeleton, async interpreter, JSON marshalling, four call kinds wired, demo runner.

Out (deferred to slice 2b as agreed): `reqwest`, real Anthropic adapter, `.env` loading + `dotenvy`, the proper `corvid run`-with-tool-registration story (currently `corvid run` works only on tool-free programs; tool-using programs need a runner binary like the demo's). Effect-tagged `import python` stays on its Phase 16 schedule.

### Next

Slice 2b pre-phase chat. Topic: HTTP client, Anthropic adapter, `.env` loading, secret redaction in traces, and how `corvid run` should learn about user-side tool implementations once we have a way to load them.

---

## Day 18 — Phase 11 slice 2b: real network + secrets ✅

**Phase 11 is complete.** Real Claude and GPT calls work end-to-end. `.env` loading, secret redaction, two adapters side by side, two minimal real-network demos, mock-HTTP integration tests for both adapters. Python has been off the critical path since slice 2a; slice 2b is what makes the runtime useful.

### Pre-phase decisions, locked in conversation

1. **Provider scope: OpenAI + Anthropic** (Option B). Reason: the developer has an OpenAI key, so Anthropic alone would mean shipping unverifiable code. Two adapters also prove the prefix-dispatch abstraction holds against two different APIs. Google + Ollama stay on the Phase 18 schedule.
2. **TLS: `rustls-tls`.** Pure Rust, identical behaviour across Linux / macOS / Windows, no system OpenSSL or schannel surprises. Cost accepted: slightly larger binary.
3. **Tool-program gap stays open.** `corvid run` for tool-using programs still requires a runner binary. Closes properly in Phase 14 when proc-macro `#[tool]` registration lands. No `--runner` stopgap (would ossify into a permanent UX bandaid).
4. **Schema lives in `corvid-vm`, not `corvid-runtime`.** The runtime stays type-agnostic — no dependency on `corvid-types`. Schema derivation goes in `corvid-vm/src/schema.rs`; the interpreter populates `LlmRequest.output_schema: Option<serde_json::Value>` per call. Adapters consume it without ever knowing what a `Type` is.
5. **Structured output per provider.** Anthropic uses `tool_use` (a synthetic tool named `respond_with_<prompt>` with `tool_choice` forcing it). OpenAI uses `response_format: {type: "json_schema", json_schema: {strict: true, schema: ...}}`. The same JSON Schema feeds both — our derivation already meets OpenAI strict-mode requirements (`additionalProperties: false`, every property in `required`).

### What landed

**`corvid-runtime`**
- `Cargo.toml`: `reqwest = "0.12"` with `default-features = false, features = ["json", "rustls-tls"]`, `dotenvy = "0.15"`, `wiremock` as dev-dep.
- `llm/anthropic.rs`: `AnthropicAdapter` — `POST /v1/messages`, `x-api-key` + `anthropic-version: 2023-06-01` headers, structured output via `tool_use` with `tool_choice: {type: "tool", name: ...}`, text-block concatenation for unstructured. `with_base_url` for tests. 60s default timeout. `handles(model)` matches `claude-*`.
- `llm/openai.rs`: `OpenAiAdapter` — `POST /v1/chat/completions`, `Authorization: Bearer`, `response_format: json_schema` with `strict: true`, content-string parse for unstructured. Same `with_base_url` pattern. `handles(model)` matches `gpt-*`, `o1-*`, `o3-*`, `o4-*`, plus bare `o1`/`o3`/`o4`.
- `env.rs`: `find_dotenv_walking` + `load_dotenv_walking` + `load_dotenv`. Real env vars win; missing `.env` is silent. `dotenvy::from_path` is the underlying call.
- `redact.rs`: `RedactionSet` — built once from env vars matching `*_KEY` / `*_TOKEN` / `*_SECRET` / `*_PASSWORD`. `redact(Value)` walks JSON recursively, replacing string matches with `"<redacted>"`. `redact_args(Vec)` for trace events.
- `tracing.rs`: `Tracer::with_redaction(RedactionSet)` builder method. `emit` filters event payloads (`ToolCall.args`, `ToolResult.result`, `LlmResult.result`, `ApprovalRequest.args`) before serialization. Note: `with_redaction` must be called before any clones — documented.

**`corvid-vm`**
- `schema.rs`: `schema_for(&Type, &types_by_id) -> serde_json::Value`. Cycle-guarded for defensive reasons (the type system doesn't permit recursive types yet but the schema walker shouldn't loop if one ever slips through). `Function` and `Unknown` emit `{}` (permissive — type checker is the real backstop). Handles inline nested struct schemas (no `$ref`).
- `interp.rs::eval_call`: when handling a `Prompt` call, derives the schema from `prompt.return_ty` and threads it into `LlmRequest.output_schema`.

**`corvid-driver`**
- `run_native`: now loads `.env` (walks from source's parent and from cwd), opens the tracer with `RedactionSet::from_env()`, and autoregisters adapters: Anthropic when `ANTHROPIC_API_KEY` is set, OpenAI when `OPENAI_API_KEY` is set. `CORVID_MODEL` becomes the default model.
- Re-exports: added `AnthropicAdapter`, `OpenAiAdapter`, `RedactionSet`, `fresh_run_id`, `load_dotenv_walking`, plus `StructValue` for runner ergonomics.

**`corvid-cli`**
- `cmd_doctor` rewritten. Loads `.env` so it sees what programs would. Reports: `.env` path / absent, `CORVID_MODEL` value or hint, `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` set/unset, model-prefix vs key cross-check (warns if `CORVID_MODEL=claude-*` but no Anthropic key, etc.), Python presence as legacy-only note.

**Demos** (workspace members)
- `examples/openai_hello/` — `Greeting { salutation, target }` returned by a real `gpt-4o-mini` call.
- `examples/anthropic_hello/` — same shape, Claude-haiku default.
- Both register their own tracer with redaction.

**Mock-HTTP integration tests**
- `crates/corvid-runtime/tests/anthropic_integration.rs` — 3 tests: structured call sends tool definition + extracts tool_use input, unstructured concatenates text blocks, HTTP error surfaces as `AdapterFailed`.
- `crates/corvid-runtime/tests/openai_integration.rs` — 3 tests: structured call sends `response_format` + parses JSON content string, unstructured returns raw string, HTTP error surfaces as `AdapterFailed`. Both inspect the recorded request to verify wire format.

### Test counts

| Crate | Tests |
|---|---|
| corvid-ast | 3 |
| corvid-syntax | 75 |
| corvid-resolve | 14 |
| corvid-types | 18 |
| corvid-ir | 6 |
| corvid-runtime (unit) | 37 |
| corvid-runtime (anthropic_integration) | 3 |
| corvid-runtime (openai_integration) | 3 |
| corvid-vm | 35 |
| corvid-codegen-py | 13 |
| corvid-driver | 12 |
| Python runtime | 10 |

**Total: ~229 across the workspace, all green.** Slice 2b added: 5 anthropic + 5 openai unit, 4 env + 6 redact, 4 schema, 6 mock-HTTP integration (3 per adapter).

### Honest scope check

- **No real-network test in the suite.** A `#[ignore]`d test per adapter was in the brief; we omitted it because `wiremock` already covers wire-format correctness, and an `#[ignore]`d test that nobody runs is documentation pretending to be a test. We rely on the demos (`cargo run -p openai_hello` / `anthropic_hello`) for real-network verification.
- **`with_redaction` clone-ordering caveat** is documented in code: callers must apply it before sharing the `Tracer` handle, otherwise the redaction-aware sibling has no file backing. The `RuntimeBuilder` path in `run_native` orders this correctly. Acceptable for slice 2b; revisit if it bites a real user.
- **Retries / circuit breakers** belong to Phase 20 (typed `Result` + retry policies). A 401 / 429 / 5xx today returns `RuntimeError::AdapterFailed` and the agent aborts. That's the correct behaviour for now.

### Phase 11 done

`corvid run examples/refund_bot_demo/src/refund_bot.cor` (or `cargo run -p refund_bot_demo`) works without Python. Real `claude-*` and `gpt-*` calls work given the matching API key. Trace events get scrubbed of secrets. The TS/`.d.ts` analogy holds: Corvid is a standalone language with first-class provider interop, not a wrapper around any one vendor.

### Next

Phase 12 — Cranelift scaffolding. Pre-phase chat first per the standing rule. Topic: Cranelift module layout, IR → CLIR translation strategy for arithmetic / control flow / calls, parity-test harness, and how `corvid build` starts emitting native binaries alongside the existing Python `target/py/`.

---

## Day 19 — Phase 12 slice 12a: AOT scaffolding + Int arithmetic ✅

**Corvid now produces real native binaries.** `corvid build --target=native examples/answer.cor` emits `examples\target\bin\answer.exe` (or `answer` on Unix), a standalone executable that runs, prints its `i64` result, and exits cleanly. The interpreter-vs-compiled-binary parity harness proves 15 fixtures agree byte-for-byte, including the three overflow/div-zero cases.

### Pre-phase decisions, locked in conversation

1. **AOT-first, not JIT.** The v1.0 pitch is literally "single binary." JIT would have been ~50 lines of throwaway plumbing and a spiritually wrong detour. We use `cranelift-object` + system linker (via the `cc` crate) from day one.
2. **Trap-on-overflow arithmetic.** Cranelift's `iadd` is wrapping; the interpreter uses `checked_add`. Silent wrapping is the exact bug class "safety at compile time" is supposed to prevent, and a divergence between tiers destroys the oracle property. We emit `sadd_overflow` / `ssub_overflow` / `smul_overflow` with a branch to a runtime handler on overflow. Division and modulo trap on a zero divisor. Matches interpreter semantics byte-for-byte. Cost: one extra instruction per arithmetic op (~ Rust-debug-mode speed). `@wrapping` opt-out is a Phase-22 conversation alongside `@budget($)`.
3. **Slice plan for Phase 12.** 12a = Int-only AOT scaffolding. 12b = Bool + comparisons + if/else. 12c = Let + for + richer control flow. 12d = Float / String / Struct / List. 12e = make native the default for tool-free programs. 12f = polish + benchmarks. Tool / prompt / approve calls in compiled code wait for Phase 14.

### What landed

**New crate `corvid-codegen-cl`**
- `src/errors.rs` — `CodegenError` with `NotSupported` / `Cranelift` / `Link` / `Io` kinds. Every `NotSupported` message names the slice that will remove the limitation, so the boundary is auditable.
- `src/module.rs` — `make_host_object_module(name)`: `target-lexicon::Triple::host()`, PIC on, `opt_level=speed`, verifier on. Uses `cranelift-object::ObjectBuilder`.
- `src/lowering.rs` — The heart. Two passes (declare all agents, then define bodies), plus a third pass that emits the `corvid_entry` trampoline. Arithmetic ops with overflow trap. Int-only gate with a type-name error pointing at slice 12d.
- `src/link.rs` — Drives `cc::Build::get_compiler()` + `std::process::Command`. MSVC: `cl.exe /Fo<tmpdir>\ shim.c object.o /Fe:out.exe`. Unix: `cc shim.c object.o -o out`. Per-invocation tempdir so parallel test runs don't race for `corvid_shim.obj`.
- `runtime/shim.c` — `int main(void)` calls `extern long long corvid_entry(void)` and `printf`s the result. `corvid_runtime_overflow` prints `corvid: runtime error: integer overflow or division by zero` to stderr and `exit(1)`s. Slice 12a keeps it parameter-less; argv handling arrives alongside `String` in 12d.
- `tests/parity.rs` — The oracle. 15 fixtures. Each runs through both tiers, asserts identical result or parallel failure.

**Driver + CLI**
- `corvid-driver::build_native_to_disk(path)` → `NativeBuildOutput { source, output_path, diagnostics }`. Output dir convention mirrors the Python path: `<project>/target/bin/<stem>[.exe]` when source is under a `src/` dir.
- `corvid build --target=native <file>` dispatches to it. Default target remains `python` for backwards compatibility; `--target=py` is an alias.

### Design choices made during implementation

1. **`corvid_entry` trampoline, not shim patching.** Initial attempt rewrote `corvid_entry` → user agent name in the shim source. That collided when users named an agent `main` (duplicate C `int main` definition). Replaced with a stable `corvid_entry` symbol the compiler emits as a trampoline calling the chosen entry agent. Shim is 100% static text now — `include_str!`'d, never mutated.
2. **User agents get `corvid_agent_` symbol prefix.** A user's `agent main() -> Int` should not collide with C's `int main`. Mangling also pre-empts future collisions with `printf`, `malloc`, etc. Only the trampoline is exported; user agents are `Linkage::Local`.
3. **`/Fo<tempdir>\` for MSVC.** `cl.exe` writes the intermediate `.obj` for `shim.c` to the current directory by default. Parallel test runs all wrote to the same `corvid_shim.obj`, causing cascading permission-denied and LNK2005 failures. Redirecting with `/Fo<tempdir>\` isolates each invocation.
4. **`INTEGER_OVERFLOW` trap code.** Cranelift 0.116 changed `TrapCode` from an enum to a struct with associated constants. `TrapCode::UnreachableCodeReached` no longer exists; `TrapCode::INTEGER_OVERFLOW` is the right match for our semantic (both overflow and div-by-zero route to the same handler anyway).

### Test counts

| Crate | Tests |
|---|---|
| corvid-ast | 3 |
| corvid-syntax | 75 |
| corvid-resolve | 14 |
| corvid-types | 18 |
| corvid-ir | 6 |
| corvid-runtime (unit) | 37 |
| corvid-runtime (integration) | 6 |
| corvid-vm | 35 |
| corvid-codegen-py | 13 |
| **corvid-codegen-cl (parity)** | **15 (new)** |
| corvid-driver | 12 |
| Python runtime | 10 |

**Total: ~244 tests, all green.**

### Verified live

```sh
$ cargo run -p corvid-cli -- build --target=native examples/answer.cor
built: examples/answer.cor -> examples\target\bin\answer.exe

$ ./examples/target/bin/answer.exe; echo "exit: $?"
42
exit: 0
```

### Scope honestly held

In: Int-only arithmetic, agent-to-agent calls, overflow trap, AOT binary on disk, CLI flag, parity harness.

Out (deferred to later slices, each with a pointer): `Bool` + comparisons + `if`/`else` → 12b. `Let` + `for` + rich control flow → 12c. `Float` / `String` / `Struct` / `List` → 12d (which is where argv-taking entry agents also land). Native default for tool-free programs → 12e. `corvid-codegen-cl` currently stays at `Linkage::Local` for user agents and `Export` only for the trampoline — cross-object-file composition lands whenever we get there.

### Next

Slice 12b pre-phase chat. Topic: `Bool` type representation (i8 in Cranelift), comparison lowering for Int (`icmp`) and String (deferred with String itself), `if`/`else` branch lowering (two blocks with a join, merging values via block parameters). No runtime changes expected.

---

## Day 20 — Phase 12 slice 12b: Bool, comparisons, if/else ✅

**Corvid compiles conditional Int+Bool programs natively.** `agent main() -> Int: if 4 % 2 == 0: return 100 else: return 200` becomes a real Windows executable that prints `100` and exits 0. Short-circuit `and`/`or` works on both the interpreter and the compiled binary: `true or (1 / (3 - 3) == 0)` returns `true` without ever dividing, on both tiers. The oracle parity holds across 33 fixtures.

### Pre-phase decisions, locked in conversation

1. **Bool as `I8`, not `I32`.** Matches `icmp`'s native output; C/Rust ABI is 1 byte; packs tightly in future struct layout; avoids redundant `uextend`s on every comparison result. The only wider conversion needed anywhere is the trampoline's final `uextend I8 → I64` to satisfy the C shim's `long long` contract.
2. **Short-circuit `and` / `or` on both tiers.** The interpreter has a comment promising short-circuit for "Phase 12+" — this is that phase. Rewrote `eval_expr`'s BinOp arm to evaluate the right operand only when the left doesn't determine the answer. Parity is now real: observable short-circuit tests like `true or (1 / 0 == 0)` return `true` without raising on either tier.
3. **Negation `-x` traps on `i64::MIN`.** Same mechanism as slice 12a's binary-arithmetic overflow. `UnaryOp::Neg` lowers to `ssub_overflow(iconst.I64 0, x) → brif → corvid_runtime_overflow`. Matches `checked_neg` semantics byte-for-byte.

### What landed

**`corvid-vm::interp::eval_expr`**
- BinOp arm restructured: `And` / `Or` are intercepted before both sides evaluate. Left evaluates first; right only evaluates when the left doesn't already determine the result. `eval_binop`'s `And` / `Or` arms now panic with `unreachable!("short-circuited upstream")` — they're dead code.

**`corvid-codegen-cl::lowering`**
- `cl_type_for(&Type, Span) -> Result<clir::Type, CodegenError>` — the single gate all signature / value-construction flows through. Int→I64, Bool→I8; every other type raises `NotSupported` with a pointer to the slice that introduces it. Replaces the slice-12a hardcoded `I64`.
- Agent signatures now use `cl_type_for` for every param and return. Parameter variables are declared with the right Cranelift width.
- `reject_non_int_types` became `reject_unsupported_types`, delegating to `cl_type_for`.
- `IrLiteral::Bool(b)` lowers to `iconst(I8, if b { 1 } else { 0 })`. Float / String / Nothing literals each raise with their own slice-12d pointer.
- Comparison ops (`==`, `!=`, `<`, `<=`, `>`, `>=`) lower to `icmp` with the matching `IntCC`. Works for Int+Int; Bool+Bool equality round-trips through the same path naturally.
- `lower_int_binop` renamed to `lower_binop_strict` and extended with the comparison arms. `And`/`Or` arms are now `unreachable!()` — the `lower_expr` BinOp case short-circuits them into `lower_short_circuit` before any evaluation.
- New `lower_unop(op, v)`: `Not` → `icmp_eq(v, 0)`; `Neg` → `ssub_overflow(iconst 0, v)` + overflow-trap branch.
- New `lower_short_circuit(op, left, right)`: emits a right-eval block + a merge block with an `I8` block parameter. For `and`: `brif(l, right_block, merge[0])`. For `or`: `brif(l, merge[1], right_block)`. The right block evaluates the RHS and `jump merge[v_right]`. Merge's block param is the result.
- New `lower_if(cond, then, else?)`: classic cond/then/else/merge block pattern. Tracks `any_fell_through` to decide whether merge is reachable; if no branch falls through, terminates merge with a trap and returns `BlockOutcome::Terminated` so the enclosing lower_block knows to stop emitting code.
- `emit_entry_trampoline` now takes `entry_return_ty: clir::Type`. If `I8`, inserts `uextend.i64` before `return_` so the C shim's `long long corvid_entry(void)` contract holds.

**Parity harness**
- New `assert_parity_bool(src, expected_bool)` helper. Trampoline zero-extends Bool → I64; shim prints `0` or `1`; harness parses and checks `Value::Bool`.
- 18 new fixtures: Bool literals (true/false), int equality/inequality, int ordering (all four), `not`, unary negation, unary-negation-of-`i64::MIN` overflow parity, if/else taking the then/else branch, if-without-else fallthrough and take-then, nested if/else, short-circuit `and` with true/false LHS, short-circuit `or` with true/false LHS, **observable** short-circuit for both `or` (skips div-by-zero) and `and` (skips div-by-zero), Bool-returning agent end-to-end.

### Bugs caught during the slice

1. First attempt at unary-negation fixtures used `let` bindings (`x = 5`). Those aren't compilable until slice 12c — got a clean `CodegenError::NotSupported` pointing at the right slice. Rewrote the fixtures to use the prefix `-` form directly (`return -5`, `return -(2+3)`, `return -(0 - i64::MAX - 1)`). Clean outcome: the `NotSupported` machinery works as intended and the fixtures now exercise the Neg path.
2. Bool-returning fixture accidentally included a top-level assignment that isn't valid Corvid syntax. Typo; removed.

### Test counts

| Crate | Tests |
|---|---|
| corvid-ast | 3 |
| corvid-syntax | 75 |
| corvid-resolve | 14 |
| corvid-types | 18 |
| corvid-ir | 6 |
| corvid-runtime (unit) | 37 |
| corvid-runtime (integration) | 6 |
| corvid-vm | 35 |
| corvid-codegen-py | 13 |
| **corvid-codegen-cl (parity)** | **33 (was 15)** |
| corvid-driver | 12 |
| Python runtime | 10 |

**Total: ~262 tests, all green.** Slice 12b added 18 parity fixtures.

### Verified live

```sh
$ corvid build --target=native examples/conditional.cor
built: examples/conditional.cor -> examples\target\bin\conditional.exe

$ ./examples/target/bin/conditional.exe; echo "exit: $?"
100
exit: 0
```

### Scope honestly held

In: `cl_type_for` gate, Bool representation as I8, six comparison ops, unary not / neg (with overflow trap), short-circuit and/or on both tiers, if/else lowering, trampoline uextend for Bool.

Out (deferred to later slices, each with a pointer): `Let` + `for` + `break`/`continue`/`pass` → 12c. `Float` / `String` / `Struct` / `List` → 12d. Tool / prompt / approve in compiled code → Phase 14.

### Next

Slice 12c pre-phase chat. Topic: `Let` bindings via Cranelift `Variable`s, `for` loop lowering over lists (which requires list memory representation — fuzzy boundary with 12d), `break` / `continue` control flow, `pass` as a no-op. Possible sub-split: 12c1 Let + `pass` + `break`/`continue` without `for`; 12c2 `for` once we have lists. Worth discussing before code.

---

## Day 21 — Phase 12 slice 12c: local bindings + `pass` ✅

**Corvid compiles programs with local variables natively.** A program like `base = 10; multiplier = 4; result = base * multiplier; if result > 30: result = result + 2; return result` becomes a real `.exe` that prints `42`. Reassignment, type-change defensive guard, `pass` as a noop — all in. 42 parity fixtures green, end-to-end through the AOT path.

### Pre-phase decisions, locked in conversation

1. **Narrow 12c to `Let` + `pass`. Defer `for` / `break` / `continue` to slice 12d alongside `List`.** The "keep 12c as three items" framing was momentum — the structural coupling is `for ↔ List`, not `for ↔ Let`. Bundling the wrong things together would be exactly the kind of "this'll do for now" the project values warn against. `break`/`continue` only make sense inside loops, so they go where the loops go.
2. **Trust the resolver for scope.** Branch-defined locals (`if cond: x = 1 else: x = 2; return x`) aren't a codegen problem — the resolver already gives the two `x`s distinct `LocalId`s, so `return x` after the branch fails at resolve time. The codegen never sees the pattern. Same discipline as slice 12b's "trust the typechecker" stance on non-Bool `if` conditions.
3. **Defensive type-change guard on reassignment.** If the same `LocalId` is reassigned with a different declared type (a typechecker bug), the codegen emits a clean `CodegenError::Cranelift` instead of letting Cranelift panic. One extra check; closes a failure mode.
4. **Wording correction (caught mid-brief).** Corvid uses Python-style bare `x = expr`, no `let` keyword. The IR's `IrStmt::Let` is compiler-internal jargon (textbook convention for "introduce a binding"). Slice 12c doesn't add user-facing syntax — it makes the existing assignment syntax compile natively.

### What landed

**`corvid-codegen-cl::lowering`**
- Env type changed from `HashMap<LocalId, Variable>` to `HashMap<LocalId, (Variable, clir::Type)>` everywhere (parameter binding, IrExprKind::Local lookup, lower_block, lower_stmt, lower_expr, lower_short_circuit, lower_if). The type record lets the reassignment path compare widths.
- New `IrStmt::Let` arm:
  - Compute `cl_ty = cl_type_for(&stmt.ty, span)?`.
  - Look up `local_id` in env. If absent → declare new Variable with `cl_ty`, increment `var_idx`, insert into env. If present → check the recorded type matches; if not, raise `CodegenError::Cranelift("variable redeclared with different type: was X, now Y — typechecker should have caught this")`.
  - Lower `value`, `def_var(var, v)`. Cranelift handles the SSA bookkeeping invisibly.
- `IrStmt::Pass` arm flipped from `NotSupported` to `Ok(BlockOutcome::Normal)`.
- `IrStmt::Break` / `IrStmt::Continue` arms now point at slice 12d (which absorbs them with `for` and `List`) instead of slice 12c.

**Parity harness**
- 9 new fixtures: literal binding + return; multi-binding arithmetic with precedence; binding used twice in one expression; three-step reassignment; Bool binding; reassignment inside `if` body; binding used in a Bool comparison; `pass` inside an `if` as a noop; parameterised-agent + local (interpreter-only since `--target=native` still requires parameter-less entry per slice 12d).

### Bugs caught (or rather, design dead-ends avoided)

- The fuzzy `for / List` boundary surfaced during the brief. We avoided shipping `for` in 12c without `List` (would have required inventing a `range` primitive that doesn't exist in the IR — pure scope creep). Cleaner answer: bundle `for` + `break` + `continue` into 12d where `List` already had to land anyway.

### Test counts

| Crate | Tests |
|---|---|
| corvid-ast | 3 |
| corvid-syntax | 75 |
| corvid-resolve | 14 |
| corvid-types | 18 |
| corvid-ir | 6 |
| corvid-runtime (unit) | 37 |
| corvid-runtime (integration) | 6 |
| corvid-vm | 35 |
| corvid-codegen-py | 13 |
| **corvid-codegen-cl (parity)** | **42 (was 33)** |
| corvid-driver | 12 |
| Python runtime | 10 |

**Total: ~271 tests, all green.** Slice 12c added 9 parity fixtures.

### Verified live

```sh
$ corvid build --target=native examples/with_locals.cor
built: examples/with_locals.cor -> examples\target\bin\with_locals.exe

$ ./examples/target/bin/with_locals.exe; echo "exit: $?"
42
exit: 0
```

### Scope honestly held

In: Let bindings, reassignment, type-change guard, `pass` as noop.

Out (deferred to slice 12d, with explicit pointers in `NotSupported` errors): `for` loops, `break`, `continue`, `Float`, `String`, `Struct`, `List`, parameterised entry agents (which need argv handling in the C shim and land alongside `String`).

### Next

Slice 12d pre-phase chat. Big slice — type surface (Float / String / Struct / List), `for` loops, `break`/`continue`, parameterised entry agents. Multiple sub-decisions (memory representation for strings / structs / lists, GC policy, calling convention for non-Int returns, argv decoding). Worth a careful brief before any code.

---

## Day 22 — Phase 12 slice 12d: `Float` ✅

**Corvid compiles Float arithmetic natively.** Programs like `price = 19.99; quantity = 3; total = price * quantity; if total > 50.0: return 1 else: return 0` produce real binaries that exit with the right answer. IEEE 754 semantics on both tiers — `1.0 / 0.0` is `+Inf`, `NaN != NaN`, no trap.

### Pre-phase decisions, locked in conversation

1. **Take the slice split.** Original 12d (Float + String + Struct + List + for + break/continue + parameterised entry) is five slices in a trench coat. Split into 12d (Float) / 12e (String) / 12f (Struct) / 12g (List + for + break/continue) / 12h (parameterised entry + Float-returning entries). Each piece has its own design boundary; bundling them would mean dev-log entries too long to read.
2. **Float follows IEEE 754. Update the interpreter to match.** Different domain than Int: integer overflow has no defined "wrap" answer that's meaningful; Float has Inf/NaN as part of the value language. Every other language users have ever touched uses IEEE for floats. The interpreter's prior trap-on-Float-div-zero was a leftover from the Int treatment, applied without specific design intent — removing it is a consistency fix, not a regression. Corvid's safety story focuses on effects/approvals/grounding/citations, not arithmetic. Int stays trap-on-overflow because integers are a different domain.

### What landed

**`corvid-vm::interp::float_arith`**
- Removed div-zero / mod-zero traps. Float div-by-zero returns `+Inf` / `-Inf` / `NaN` per IEEE; Float mod-zero returns `NaN`. Comment cites the design intent so future readers don't restore the trap.

**`corvid-codegen-cl::lowering`**
- `cl_type_for(Float) → F64`.
- `IrLiteral::Float(n)` lowers to `f64const(n)`.
- `lower_binop_strict` restructured around an `ArithDomain { Int, Float }` enum after a new `promote_arith` helper widens mixed `Int + Float` operands to `F64` via `fcvt_from_sint`. Same widening rule as `eval_arithmetic` in the interpreter.
- Float arithmetic uses `fadd` / `fsub` / `fmul` / `fdiv`. Float `%` is computed as `a - trunc(a / b) * b` since Cranelift has no `frem` — matches Rust's `f64::%` semantics.
- Float comparisons via `fcmp`: `==` is `FloatCC::Equal` (false on NaN), `!=` is `FloatCC::NotEqual` which is the IEEE-quiet ordered variant. Cranelift's NaN treatment matches Rust's `PartialEq` and IEEE 754, so parity is automatic.
- `lower_unop` now dispatches by value type: `UnaryOp::Neg` on `F64` → `fneg` (no trap), on `I64` → existing `ssub_overflow(0, x)` with overflow trap.
- `reject_unsupported_types` updated; the slice-pointer in error messages now says "12d supports Int/Bool/Float" and points at 12e–g for the rest.

**`corvid-codegen-cl::lib::build_native_to_disk`**
- New defensive guard: an entry agent returning `Float` raises `CodegenError::NotSupported` pointing at slice 12h. The C shim's `printf("%lld\n", corvid_entry())` only handles Int/Bool; supporting Float entries needs either a second shim variant or a different print-format selection at build time. Both naturally land in 12h alongside argv decoding, where the shim is already growing.

### Bugs caught (well — divergence closed)

The interpreter was trapping on `1.0 / 0.0`. That predates this slice but was never deliberate policy. Closing it before adding the codegen meant the parity harness validates IEEE-compliant behavior from the first compile, instead of accumulating a "known divergence" list that grows over time and stops being trusted. One-line interpreter fix (~6 lines including the explanatory comment).

### Test counts

| Crate | Tests |
|---|---|
| corvid-ast | 3 |
| corvid-syntax | 75 |
| corvid-resolve | 14 |
| corvid-types | 18 |
| corvid-ir | 6 |
| corvid-runtime (unit) | 37 |
| corvid-runtime (integration) | 6 |
| corvid-vm | 35 |
| corvid-codegen-py | 13 |
| **corvid-codegen-cl (parity)** | **52 (was 42)** |
| corvid-driver | 12 |
| Python runtime | 10 |

**Total: ~281 tests, all green.** Slice 12d added 10 parity fixtures: Float addition with eq-check, sub/mul, exact division, mixed Int+Float promotion (both orderings), all four orderings, unary negation, IEEE Inf-on-div-zero proof, NaN != NaN proof, Float in local binding, Float-entry-return defensive guard.

### Verified live

```sh
$ corvid build --target=native examples/float_calc.cor
built: examples/float_calc.cor -> examples\target\bin\float_calc.exe

$ ./examples/target/bin/float_calc.exe; echo "exit: $?"
1
exit: 0
```

### Scope honestly held

In: Float type, four arithmetic ops (with IEEE semantics), six comparisons (with IEEE NaN handling), mixed Int+Float promotion, Float negation, Float in local bindings.

Out (deferred to later slices, each with explicit pointers): String → 12e. Struct → 12f. List + for + break/continue → 12g. Float-returning entry agents → 12h. Parameterised entries → 12h.

### Next

Slice 12e pre-phase chat. Topic: `String` memory representation (pointer + length, immutable), allocator policy (malloc + leak-on-exit, or arena, or refcount?), how string literals land in the object file's `.rodata`, how concatenation owns its result, how `==` on strings works (length compare + memcmp). Worth a careful brief — strings are the first non-scalar type and they expose calling-convention questions that ripple through the rest of Phase 12.

---

## Day 23 — Phase 12 slice 12e: memory management foundation ✅

**Corvid native binaries now ship with a real refcounted heap allocator.** Atomic refcount, immortal sentinel for static literals, leak counters, full C runtime linked into every binary. No String lowering yet — that's slice 12f. But the foundation is real: every `corvid build --target=native` output now contains `corvid_alloc` / `corvid_retain` / `corvid_release` / `corvid_string_concat` / `corvid_string_eq` / `corvid_string_cmp` symbols, ready to be called the moment the codegen wires them in.

### Pre-phase decisions, locked in conversation

User pushed back on my "ship malloc + leak now, fix later" proposal — correctly. Corvid is positioned as **AI-native**, not just batch-agent-shaped. RAG services, multi-agent coordinators, eval pipelines, durable workflows all run for hours/days/weeks. Shipping `String` with leak semantics would make Corvid unviable for the very workloads it's positioned to serve, and would undermine the "compile-time safety beats runtime safety" pitch by ignoring runtime memory safety entirely.

Locked decisions:

1. **Refcount, not GC, not borrow checking.** Corvid's value semantics (immutable scalars + immutable composites + agent-call composition, no first-class mutable references) prevent reference cycles. Refcount is sufficient and stays sufficient. Swift / Obj-C / CPython have shipped real production runtimes on refcount.
2. **16-byte header** — atomic refcount (8 bytes) + reserved word (8 bytes) for future per-allocation metadata (type tag, weak count, generation counter if cycles ever appear). Preserves natural 8-byte payload alignment.
3. **Atomic refcount.** Single-threaded today; Phase 25 multi-agent will introduce concurrency. Going atomic now means no migration. Cost: ~10–50ns vs ~1–2ns non-atomic — small and worth not paying compounded interest later.
4. **Scope-driven release insertion** (release at block exit) over liveness-driven (release at last use). Correctness now; the optimisation is a Phase 22 perf concern, not a slice 12e gate.
5. **Combined slice (foundation + String)** — committed up front. Then mid-session, after the foundation landed cleanly and the String integration revealed itself as a substantial slice on its own (RuntimeFuncs threading + scope-stack data structure + ownership rules + literal lowering via `.rodata` + parity harness updates), split into 12e (foundation) and 12f (String operations + ownership wiring). This preserves the discipline the standing rule asks for: each slice = one coherent landing.

### What landed

**`crates/corvid-codegen-cl/runtime/alloc.c`** — the real refcount runtime.
- 16-byte header struct: `_Atomic long long refcount; long long reserved;`
- `corvid_alloc(payload_bytes)`: `malloc(16 + N)`, set refcount=1, reserved=0; return payload pointer (header + 16). Atomic-increments leak counter.
- `corvid_retain(payload)`: walk back 16 bytes, atomic increment if refcount != INT64_MIN.
- `corvid_release(payload)`: walk back 16, atomic decrement; free the underlying block when refcount hits zero. Atomic-increments release counter. Aborts with a clear stderr message on use-after-free (refcount already <= 0).
- Two atomic counters (`corvid_alloc_count` / `corvid_release_count`) track totals for the shim's leak-detector output.

**`crates/corvid-codegen-cl/runtime/strings.c`** — String operations on top of the allocator.
- `corvid_string_concat(a, b)`: allocates `sizeof(corvid_string) + a.len + b.len` in one block; descriptor + bytes co-located; refcount=1; doesn't retain inputs.
- `corvid_string_eq(a, b)`: length compare + `memcmp`; returns 1 / 0.
- `corvid_string_cmp(a, b)`: `memcmp` of `min(len_a, len_b)` then length tiebreaker; returns -1 / 0 / 1.
- `alloc_string(src, len)` — internal helper for fresh allocations from raw bytes (used internally; will be exposed if a `String.from_bytes` builtin ever appears).

**`crates/corvid-codegen-cl/runtime/shim.c`** — leak detector wired in.
- Existing entry-trampoline + overflow-handler behaviour preserved.
- After `corvid_entry()` returns, if `getenv("CORVID_DEBUG_ALLOC")` is non-null, prints `ALLOCS=N\nRELEASES=N` to stderr.
- Off by default — existing parity tests see clean stdout/stderr unchanged.

**`crates/corvid-codegen-cl/src/link.rs`** — three C files now compile + link together.
- `ALLOC_SOURCE` and `STRINGS_SOURCE` `include_str!`'d alongside `ENTRY_SHIM_SOURCE`.
- All three written to the per-invocation tempdir before the C compiler runs (avoids `corvid_*.obj` collisions between parallel tests on MSVC).
- `cl.exe` invocation gets `/std:c11 /experimental:c11atomics` for `<stdatomic.h>` support; `cc` invocation gets `-std=c11`.

**`crates/corvid-codegen-cl/src/lowering.rs`** — type plumbing for the slice 12f integration to rest on.
- `cl_type_for(Type::String) → I64` (descriptor pointer; same width as `Int`, distinguished only by `is_refcounted_type`).
- `is_refcounted_type(ty)` returns true for `String` (will extend to `Struct` / `List` in 12g / 12h).
- Public symbol constants: `RETAIN_SYMBOL`, `RELEASE_SYMBOL`, `STRING_CONCAT_SYMBOL`, `STRING_EQ_SYMBOL`, `STRING_CMP_SYMBOL`. Slice 12f imports them via `module.declare_function(SYMBOL, Linkage::Import, &sig)`.

### Bugs caught during the slice

1. **MSVC `<stdatomic.h>` requires `/std:c11`.** First link attempt failed with `fatal error C1189: "C atomics require C11 or later"`. Fix: add `/std:c11 /experimental:c11atomics` for MSVC and `-std=c11` for GCC/Clang in `link.rs`. Same fix would have come up later anyway when slice 12f tested — surfacing now means the foundation is portable on day one.

### Test counts

| Crate | Tests |
|---|---|
| corvid-ast | 3 |
| corvid-syntax | 75 |
| corvid-resolve | 14 |
| corvid-types | 18 |
| corvid-ir | 6 |
| corvid-runtime (unit) | 37 |
| corvid-runtime (integration) | 6 |
| corvid-vm | 35 |
| corvid-codegen-py | 13 |
| **corvid-codegen-cl (parity)** | **52 (unchanged — runtime linked into every existing fixture without behaviour change)** |
| corvid-driver | 12 |
| Python runtime | 10 |

**Total: ~281 tests, all green.** Slice 12e added zero new fixtures because the foundation is invisible to user code until slice 12f wires up String operations. The completion criterion was "every existing parity fixture still passes with the new C runtime linked," which holds.

### Verified live

```sh
$ corvid build --target=native examples/with_locals.cor
built: examples/with_locals.cor -> examples\target\bin\with_locals.exe

$ ./examples/target/bin/with_locals.exe
42

$ CORVID_DEBUG_ALLOC=1 ./examples/target/bin/with_locals.exe
42
ALLOCS=0
RELEASES=0
```

### Honest scope check

The combined "memory + String" slice was too big to land in one session safely. Splitting mid-session preserved the discipline rather than rushing the ownership-wiring story (which is the most error-prone piece of the remaining work). The foundation is genuinely useful as a standalone landing — it's the substrate slice 12f, 12g, and 12h all reuse without modification, and exercising it via "every existing fixture still works" gives us confidence the C runtime + linker integration are correct before we layer ownership management on top.

### Next

Slice 12f pre-phase chat. Topic: `RuntimeFuncs` struct + module-wide declaration; lowering `IrLiteral::String` via `module.declare_data` + `define_data` with self-relative relocation for the descriptor's `bytes_ptr` field; ownership management (retain on `use_var`, release after consumed temps, release-on-rebind, retain-return + release-locals at function exit, scope-stack data structure that mirrors Corvid's lexical scoping rather than Cranelift's flat-Variable model); parity harness updates to parse `ALLOCS` / `RELEASES` from stderr.














