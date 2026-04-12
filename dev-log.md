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







