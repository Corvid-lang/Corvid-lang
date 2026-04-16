# Corvid

> A general-purpose programming language with AI built into the compiler. Write servers, CLIs, data pipelines, agents, anything — and when you reach for an LLM, the compiler has your back instead of getting out of your way.

## What v1.0 will be

A **standalone, natively-compiled, general-purpose programming language** that happens to be the best language in the world for writing AI-powered software.

Table-stakes for a modern general-purpose language:

- **Fast startup, native binaries.** Compiles `.cor` source directly to machine code via Cranelift. One binary, no runtime installer.
- **TypeScript-grade type safety.** Sound type checker, resolution, effect checker — catches bugs at `corvid check`, not at 3am.
- **Pythonic, readable syntax.** Indentation-based, 22 keywords, designed to be the language LLMs generate best against.
- **Predictable memory.** Reference-counted with deterministic destructors; cycle collector planned. No stop-the-world pauses, no manual `free`.
- **Embeddable + portable.** C ABI + library mode for embedding in other apps; WASM target for browser and edge; Python FFI via `import python "..."` when you need the ecosystem.

What makes it *AI-native* — the differentiator, not the whole story:

- **Agents, tools, prompts, `approve`, `dangerous` are keywords** — not library decorators. Enforced at compile time.
- **The compiler reasons about AI code** the same way it reasons about the rest: types flow through prompt inputs, effects propagate through tool chains, future cost bounds can be checked statically, and executions are designed to become replayable by construction.
- **No runtime magic.** Every safety property has a corresponding compiler rule that produced it. Read the error, trace it to the rule, understand why.

v1.0 is a multi-year effort. v0.1 (complete) is the internal milestone that proved the language design end-to-end using a Python transpile backend. v0.2+ builds the native runtime. See [`ROADMAP.md`](./ROADMAP.md) for the full build plan.

---

## What Corvid aims to win at

The ambition is simple: **be the default choice for the widest possible range of applications.** That doesn't mean best at every dimension — no language has ever pulled that off, and "best at everything" is what killed PL/I, Ada, and early Scala. It means: best in class on the dimensions where winning matters most, competitive on everything else, disqualified on nothing.

### The moat (things Corvid is genuinely best at)

- **Safety for AI-shaped software.** Type system + effect checker + approve-before-dangerous + compile-time cost bounds + contract verification. No other language is even trying.
- **AI-native ergonomics.** `agent`, `tool`, `prompt`, `approve`, `dangerous` as keywords. Replay, grounding contracts, cost budgets as language constructs. Structurally impossible to match without owning the whole pipeline.
- **Readability for human + LLM.** Pythonic surface, shallow hierarchies, no pointer aliasing, explicit effects. The language machines read *and write* best.

### Table stakes (top-tier, competitive with the best in category)

- **Performance.** Go / Swift class — startup in milliseconds, throughput where compute rarely bottlenecks real applications.
- **Memory.** Refcount + cycle collector. Predictable release without Java-style pauses.
- **Deployment.** Single native binary + WASM + C ABI embedding. Match Go's "one binary" pitch and add WASM.
- **Tooling.** LSP, formatter, package manager, REPL. Polished, not novel.
- **Cross-platform.** macOS + Linux + Windows, all first-class by v1.0.

### Deliberately not competing (and that's fine)

- **Systems-level control.** Rust and Zig win. Pointer arithmetic isn't on the table — Corvid is memory-safe general-purpose, not systems.
- **Raw hot-loop numerics.** C++ and Fortran win. Most applications aren't this; the ones that are can FFI.
- **Dynamic metaprogramming.** Ruby and Lisp win. Compile-time checking is the opposite trade-off.
- **Ecosystem size at launch.** Python and JS have 20-year head starts. Python FFI closes the gap; our own ecosystem grows over time.

Every proposed feature gets tested against this frame: does it strengthen a moat dimension, or bring us to parity on a table-stakes dimension where we're below the floor? If yes, build it. If it's a cool feature that moves neither bar, defer it.

---

## What makes it different

Every other language treats AI as a library — a set of imports you glue onto a general-purpose base. Corvid is a general-purpose base where AI is part of the language.

```corvid
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

Remove the `approve` line, and this file **will not compile**:

```
[E0101] error: dangerous tool `issue_refund` called without a prior `approve`
   ╭─[refund_bot.cor:7:12]
   │
 7 │     issue_refund(order.id, order.amount)
   │     ──────────────┬───────────────────────
   │                   ╰── this call needs prior approval
   │
   │  Help: add `approve IssueRefund(arg1, arg2)` on the line before this call
───╯
```

No runtime check. No decorator magic. A real type-system rule enforced at compile time — the one thing Python + Pydantic AI structurally cannot provide.

---

## Architecture

```
         ┌───────────────────────────────────────────────────────────────┐
         │                       Frontend (shared)                       │
         │  lex → parse → resolve → typecheck + effect check → lower     │
         └───────────────────────────────┬───────────────────────────────┘
                                         │  typed IR
                            ┌────────────┴─────────────┐
                            ▼                          ▼
                    ┌───────────────┐        ┌──────────────────┐
                    │ Interpreter   │        │ Native codegen   │
                    │ (dev tier +   │        │ (Cranelift, v1.0)│
                    │ correctness)  │        │                  │
                    └───────────────┘        └────────┬─────────┘
                                                      │
                                                      ▼
                                              target/bin/<name>
                                              (native binary,
                                               no Python needed)
```

An opt-in `--target=python` backend is retained for users who want to deploy to Python environments. Users pick their backend; the language stays the same.

---

## Status

**v0.0.1 — pre-alpha.**

- ✅ Frontend complete (lexer, parser, resolver, type/effect checker, IR).
- ✅ Python transpile backend works end-to-end.
- ✅ CLI works (`corvid new`, `check`, `build`, `run`, `doctor`).
- ✅ Offline demo at [`examples/refund_bot_demo/`](./examples/refund_bot_demo/).
- 🚧 Native interpreter in progress.
- ⏳ Cranelift native compiler under active development.

Tests: **134 Rust + 10 Python, all green.**

---

## Install (pre-alpha, from source)

```bash
# compiler binary
cargo install --path crates/corvid-cli

# Python runtime (only needed if you use --target=python)
pip install -e './runtime/python[anthropic]'

# environment check
corvid doctor
```

At v1.0 this becomes `curl -fsSL corvid.dev/install.sh | sh` with no Python step.

---

## Documentation

- [`CONTRIBUTING.md`](./CONTRIBUTING.md) — mission, vision, values, and the rules for working on Corvid. **Read this first if you're joining the project.**
- [`ARCHITECTURE.md`](./ARCHITECTURE.md) — compiler design, pipeline, conventions.
- [`FEATURES.md`](./FEATURES.md) — feature roadmap v0.1 → v1.0.
- [`ROADMAP.md`](./ROADMAP.md) — long-range build plan.
- [`dev-log.md`](./dev-log.md) — build journal.
- [`examples/`](./examples/) — runnable `.cor` programs.

---

## License

MIT OR Apache-2.0
