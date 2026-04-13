# Corvid

> An AI-native programming language. Agents, tools, and prompts are first-class primitives — and the compiler refuses to let you call a dangerous action without getting approval first.

## What v1.0 will be

A **standalone, natively-compiled, AI-native programming language**.

- One binary. No Python, no runtime installer, nothing else to download.
- Compiles `.cor` source directly to machine code via Cranelift.
- Python FFI is available via `import python "..."` when users want Python libraries, loaded lazily and only when needed.
- AI primitives (`agent`, `tool`, `prompt`, `approve`, `dangerous`) are compiler-native — enforced at compile time, not at runtime.

v1.0 is a multi-year effort. v0.1 (complete) is the internal milestone that proved the language design end-to-end using a Python transpile backend. v0.2+ builds the native runtime. See [`ROADMAP.md`](./ROADMAP.md) for the full phase plan.

---

## What makes it different

Every other language treats AI as a library. Corvid treats it as language.

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
- 🚧 Native interpreter in progress (Phase 11).
- ⏳ Cranelift native compiler starts Phase 12.

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
- [`ROADMAP.md`](./ROADMAP.md) — phase-by-phase build plan.
- [`dev-log.md`](./dev-log.md) — build journal.
- [`examples/`](./examples/) — runnable `.cor` programs.

---

## License

MIT OR Apache-2.0
