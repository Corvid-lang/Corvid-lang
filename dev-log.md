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














