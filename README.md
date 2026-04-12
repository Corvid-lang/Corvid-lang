# Corvid

> An AI-native programming language. Agents, prompts, tools, and effects as first-class citizens. Compile-time safety for things that matter.

## Status

**v0.0.1 — pre-alpha.** Not usable yet. Building in public.

## Why

Every mainstream language was designed before LLMs. In all of them, AI is a library import — prompts are strings, model outputs are untyped, dangerous tool calls cannot be prevented at compile time.

Corvid makes AI native. The compiler refuses to compile agent code that calls an irreversible tool without prior approval. No other language can do this.

## Example

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

Without the `approve` line, this file will not compile.

## Documentation

- [`ARCHITECTURE.md`](./ARCHITECTURE.md) — how Corvid is built
- [`FEATURES.md`](./FEATURES.md) — roadmap from v0.1 through v1.0
- [`dev-log.md`](./dev-log.md) — weekly journal

## License

MIT OR Apache-2.0
