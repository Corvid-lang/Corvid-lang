# Corvid — Architecture

> An AI-native programming language where LLM calls, prompts, tools, effects, and approvals are first-class primitives.

This document is the source of truth for how Corvid is built. It evolves with the project. Every significant architectural change updates this file.

---

## 1. Thesis

Every mainstream programming language was designed before LLMs existed. In all of them, AI is bolted on as a library: prompts are strings, LLM calls return untyped blobs, non-determinism is invisible to the compiler, and dangerous tool calls cannot be prevented at compile time.

**Corvid makes AI native.** The compiler understands prompts, tools, agents, and effects as language constructs — and enforces safety properties that no library can:

- Code that calls an irreversible tool without prior approval **does not compile**.
- Code that ignores low-confidence model output **does not compile** (v0.3+).
- Code that exceeds a declared cost budget **does not compile** (v0.3+).

This is the one thing Python + Pydantic AI structurally cannot provide.

---

## 2. Naming and conventions

- Language name: **Corvid**
- Source file extension: `.cor`
- CLI binary: `corvid` (short alias: `cor`)
- Project manifest: `corvid.toml`
- Runtime package: `corvid-runtime`
- Generated output location: `target/` (gitignored, mirrors Cargo's convention)
- Generated Python files: plain `.py` inside `target/py/`

### User project layout

```
my_project/
├── corvid.toml              # project config (name, deps, target, etc.)
├── src/
│   ├── main.cor
│   ├── refund_bot.cor
│   └── tools.cor
├── tests/
│   └── refund_bot_test.cor
└── target/                  # generated, gitignored
    ├── py/                  # Python transpile output (v0.1)
    │   ├── main.py
    │   ├── refund_bot.py
    │   └── tools.py
    ├── wasm/                # WASM output (v0.2+)
    └── trace/               # run traces (.jsonl)
```

---

## 3. Compiler repo layout (Rust workspace)

```
corvid/
├── Cargo.toml              # workspace root
├── ARCHITECTURE.md
├── FEATURES.md
├── README.md
├── dev-log.md              # weekly journal
│
├── crates/
│   ├── corvid-syntax/      # lexer + parser (chumsky), emits AST
│   ├── corvid-ast/         # AST types, shared across crates
│   ├── corvid-resolve/     # name resolution, import handling
│   ├── corvid-types/       # type system (effects, inference, checking)
│   ├── corvid-ir/          # intermediate representation (post-typecheck)
│   ├── corvid-codegen-py/  # Python emitter (v0.1 target)
│   ├── corvid-codegen-wasm/# WASM emitter (v0.2+, stub now)
│   ├── corvid-runtime/     # runtime glue: LLM calls, approvals, tracing
│   ├── corvid-driver/      # orchestrates the full pipeline
│   ├── corvid-cli/         # the `corvid` binary
│   └── corvid-lsp/         # language server (v0.5+, stub now)
│
├── examples/
│   ├── hello.cor
│   ├── refund_bot.cor
│   └── approve_demo.cor
│
├── tests/
│   ├── parse/
│   ├── typecheck/
│   ├── codegen/
│   └── e2e/
│
└── docs/                   # user-facing docs
```

---

## 4. Compiler pipeline

```
source.cor
    │
    ▼
┌──────────────────┐
│  Lexer           │  corvid-syntax (chumsky)
└────────┬─────────┘
         ▼
       tokens
         │
         ▼
┌──────────────────┐
│  Parser          │  chumsky grammar → AST
└────────┬─────────┘
         ▼
        AST
         │
         ▼
┌──────────────────┐
│  Name resolution │  corvid-resolve
└────────┬─────────┘
         ▼
   resolved AST
         │
         ▼
┌──────────────────┐
│  Type checker    │  corvid-types — enforces effect rules
└────────┬─────────┘
         ▼
     typed AST
         │
         ▼
┌──────────────────┐
│  Lowering        │  desugar, normalize, inline
└────────┬─────────┘
         ▼
        IR
         │
         ▼
┌──────────────────┐
│  Code gen        │  Python (v0.1), WASM (v0.2), native (v0.3)
└────────┬─────────┘
         ▼
    target/py/*.py
         │
         ▼
┌──────────────────┐
│  Runtime         │  corvid-runtime — LLM calls, approvals, traces
└──────────────────┘
```

Each arrow is a crate boundary. Each stage is independently testable.

---

## 5. Core data structures

### AST (crate: `corvid-ast`)

```rust
pub enum Decl {
    Import(ImportDecl),
    Prompt(PromptDecl),
    Tool(ToolDecl),
    Agent(AgentDecl),
    Type(TypeDecl),
}

pub struct ToolDecl {
    pub name: Ident,
    pub params: Vec<Param>,
    pub return_ty: TypeRef,
    pub effect: Effect,
    pub body: Option<Block>,     // Some = inline, None = extern
    pub span: Span,
}

pub enum Effect {
    Pure,
    Compensable,
    Irreversible,
    Unknown,                      // inferred or unannotated
}

pub struct AgentDecl {
    pub name: Ident,
    pub params: Vec<Param>,
    pub return_ty: TypeRef,
    pub body: Block,
    pub span: Span,
}

pub struct PromptDecl {
    pub name: Ident,
    pub params: Vec<Param>,
    pub return_ty: TypeRef,
    pub template: String,         // the prompt text itself
    pub examples: Vec<Example>,   // few-shot
    pub span: Span,
}
```

### Types (crate: `corvid-types`)

```rust
pub enum Type {
    Prim(Prim),                   // String, Int, Float, Bool
    Struct(StructTy),
    Enum(EnumTy),
    List(Box<Type>),
    Map(Box<Type>, Box<Type>),
    Function(FnTy),
    Confident(Box<Type>),         // T?confidence (v0.3)
    External(ExternTy),           // Python/JS opaque types
}

pub struct FnTy {
    pub params: Vec<Type>,
    pub ret: Box<Type>,
    pub effect: Effect,
    pub budget: Option<Cost>,     // v0.3
}
```

### Effect enforcement — the killer invariant

> Any call to a tool or function with `effect = Irreversible` must be preceded in the same block by an `approve(Action)` where `Action` matches the tool's signature.
>
> Violation produces compile error `E0101`.

This rule is the soul of the language. Everything else is supporting infrastructure.

---

## 6. Runtime architecture

The `corvid-runtime` crate provides:

- **LLM abstraction** — unified interface over Anthropic, OpenAI, Google, local models.
- **Tool execution** — async dispatch with effect gating.
- **Approval flow** — suspension, state persistence, webhook/CLI resume.
- **Trace emission** — structured JSONL events (`target/trace/*.jsonl`).
- **Memory primitives** — `session`, `memory` backed by SQLite (v0.2).

### Generated Python structure

```python
# target/py/refund_bot.py (generated from refund_bot.cor)
from corvid_runtime import llm, tool_call, approve_gate, trace

async def refund_bot(ticket):
    with trace.run("refund_bot") as t:
        order = await tool_call(
            "get_order",
            effect="pure",
            args={"id": ticket.order_id},
        )
        ...
        await approve_gate(
            action=IssueRefund(order.id, order.amount),
            effect="irreversible",
        )
        await tool_call("issue_refund", ...)
```

Generated files are plain Python. Pytest, mypy, ruff, and IDEs treat them as regular modules.

---

## 7. Interop architecture

```
corvid-runtime/src/interop/
├── python.rs       # v0.1 — PyO3 bindings, type marshaling
├── wasm.rs         # v0.2 — WASM host imports
└── ffi.rs          # v0.3 — C ABI
```

### User-facing import syntax

```
import python "anthropic" as anthropic {
  fn Messages.create(model: String, messages: List[Message]) -> Response
}

import python "pandas" as pd
```

Year 1 strategy: users declare Python function signatures; the compiler trusts them and emits runtime assertions. Year 3: auto-generate from `.pyi` stubs.

### Interop timeline

| Year | Target | What users get |
|------|--------|----------------|
| 1 | Python | Full Python ecosystem (Anthropic SDK, Pydantic, Pandas, etc.) |
| 2 | WASM + JS/TS | Runs in browsers, Node, Deno, edge |
| 3 | C ABI | Native FFI to Rust and C |
| 1+ | Subprocess | Shell out to any CLI tool |

---

## 8. Error reporting

`ariadne` from day one. Every error carries:
- File path + span (start, end bytes)
- Error code (`E0001`, `E0101`, ...)
- Primary message
- Suggestion / fix-it hint

### Target quality

```
error[E0101]: irreversible tool called without approve
  ┌─ refund_bot.cor:8:5
  │
8 │     issue_refund(order.id, order.amount)
  │     ^^^^^^^^^^^^ this call cannot happen without prior `approve`
  │
help: add an approval before the call
  │
7 + │   approve(IssueRefund(order.id, order.amount))
8   │   issue_refund(order.id, order.amount)
```

This level of polish is non-negotiable from v0.1. First impressions live or die here.

---

## 9. Testing strategy

Three levels:

- **Unit tests** — `#[test]` inside each crate.
- **Snapshot tests** — `insta` crate. For each `.cor` example, snapshot the AST, typed IR, and generated Python. Catches regressions instantly.
- **End-to-end tests** — `tests/e2e/` runs the full pipeline on real `.cor` files and asserts runtime behavior with mocked LLM calls.

Test fixtures live under `examples/` and `tests/` — same files serve both as documentation and as regression suite.

---

## 10. CLI

Target UX (keep Cargo-shaped — developers know the mental model):

```
corvid new my_project        # scaffold a new project
corvid check                 # type-check only
corvid build                 # compile to target/py/
corvid run <file>            # build + run
corvid test                  # run tests
corvid fmt                   # format code (v0.2+)
corvid fmt --check           # CI mode
corvid doc                   # generate docs (v0.3+)
corvid add <package>         # add dependency (v0.5+)
```

---

## 11. Dependencies (v0.1)

Keep disciplined. Only these Rust crates:

```toml
[workspace.dependencies]
chumsky        = "..."    # parser
ariadne        = "..."    # error messages
insta          = "..."    # snapshot tests
serde          = "..."    # serialization
serde_json     = "..."
tokio          = "..."    # async runtime
pyo3           = "..."    # Python interop (optional feature)
anyhow         = "..."    # error handling in CLI
clap           = "..."    # CLI argument parsing
```

Not yet:
- `salsa` — add in v0.2 when incremental compilation matters
- `tower-lsp` — add in v0.5 for IDE support
- `cranelift` — add in v0.3 for native codegen

**Rule:** if you want to add a crate not listed here, justify in `dev-log.md` first.

---

## 12. Versioning

Semantic versioning with honest stability commitments:

- `0.x.y` — expect breaking changes between minor versions. No stability promise.
- `1.0` — language stable. No breaking changes without major bump.
- Target `1.0` no earlier than year 3. Possibly later. Don't rush it.

---

## 13. Design principles

Rules the language holds to, in order of importance:

1. **Safety at compile time beats safety at runtime.** If the compiler can catch it, it must.
2. **AI primitives are first-class.** If a concept needs a keyword, give it a keyword.
3. **Interop is non-negotiable.** Users must be able to import any Python library from day one.
4. **Polish over features.** Seven features done perfectly beats thirty half-done.
5. **Error messages are product.** Every error has a suggested fix.
6. **Cargo mental model.** Don't invent new paradigms for tooling; copy what works.
7. **Scope discipline.** Every feature must earn its place. "No" is the default answer.
8. **Dev log always.** Weekly entries in `dev-log.md`. Non-negotiable.

---

## 14. Non-goals (things Corvid explicitly is not)

- **Not a general-purpose language.** Corvid is for AI agents. Don't write kernels or game engines in it.
- **Not a replacement for Python.** Corvid calls Python. Use Python for data work, training, web backends.
- **Not a framework.** Corvid is a compiler + runtime. Frameworks can be built in Corvid.
- **Not a workflow engine.** Durable execution is a feature, not the core identity.
- **Not a vector DB or RAG tool.** Those are libraries users import.

---

## 15. Syntax philosophy: Pythonic with AI primitives

Corvid uses **Python-shaped syntax** — indentation + colons for structure, familiar expression and statement forms, readable English keywords. The language adds *first-class AI constructs* (`agent`, `tool`, `prompt`, `effect`, `approve`) on top of syntax that Python developers already know.

### Design principles

1. **Pythonic is the baseline.** If a construct already has a clear Python form, use it. `if`, `else`, `for`, `while`, `return`, `=` for binding.
2. **New concepts get new keywords.** `agent`, `tool`, `prompt`, `effect`, `approve`, `when` (for AI confidence branching, later). These are the things Python cannot express.
3. **Indentation + colons for blocks.** No braces required. No semicolons.
4. **Type annotations are native, not bolted on.** Every parameter and return is typed. Not optional like Python.
5. **English readability where it costs nothing.** `effect pure` / `effect irreversible` as inline annotations reads better than sigils or attributes.

### Keyword budget (v0.1)

**22 keywords total**, chosen for readability — every word is a real English word a non-programmer can guess at:

```
agent   tool   prompt   type   import   as
approve   dangerous
if   else   for   in   return   break   continue   pass
true   false   nothing
and   or   not
```

No `let`, no `function`, no `effect`/`pure`/`compensable`. Assignment is bare `x = expr` (Python-style). Helpers are agents that don't call an LLM. Tools are `Safe` by default; only the ones that need approval are marked `dangerous`.

### Example

```
tool get_order(id: String) -> Order
tool issue_refund(id: String, amount: Float) -> Receipt dangerous

agent refund_bot(ticket: Ticket) -> Decision:
    order = get_order(ticket.order_id)
    decision = decide_refund(ticket, order)

    if decision.should_refund:
        approve IssueRefund(order.id, order.amount)
        issue_refund(order.id, order.amount)

    return decision
```

### Notes

- Python devs read this instantly. That is the point.
- The only unfamiliar parts — `agent`, `tool`, `prompt`, `approve`, `dangerous` — are exactly the concepts Python can't express natively. Their novelty is load-bearing.
- `dangerous` replaces the technical `effect irreversible` — it tells the reader *why* the rule exists, not the internal classification.
- Parser is simple recursive descent; no surprises at scale.
- At scale (100+ line agents), Pythonic stays tight.

## 16. Open questions (decide by end of v0.1)

These are deliberately unresolved. Revisit before v0.1 ships.

- [ ] Exact keyword set (finalize from the draft mapping above)
- [ ] Error handling: exceptions vs `Result` type vs algebraic effects
- [ ] Concurrency primitive: async/await vs green threads vs actors
- [ ] String interpolation syntax for prompt templates
- [ ] How to represent few-shot examples in `prompt` declarations
- [ ] Whether `agent` is a separate keyword or syntactic sugar over `function`
- [ ] Whether dual syntax (`f(x)` and `do f with x`) stays or collapses to one form

---

## 17. References

Prior art worth studying deeply:

- **Rust** — compiler architecture, Cargo, error messages
- **OCaml** — type inference, module system
- **Koka** — effect system (most directly relevant)
- **DSPy** — program-as-prompt composition
- **BAML** — schema-first LLM function definitions
- **Roc** — modern compiler in Rust, pure FP

Books:
- *Crafting Interpreters* — Bob Nystrom (start here)
- *Types and Programming Languages* — Benjamin Pierce (reference)
- *Engineering a Compiler* — Cooper & Torczon (advanced)
