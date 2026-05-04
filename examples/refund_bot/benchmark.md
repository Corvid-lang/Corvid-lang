# Refund Bot Benchmark Notes

This demo is intentionally small; its performance target is fast compile-time
rejection of unsafe money-moving code, not runtime throughput.

Local smoke measurements should focus on:

- `corvid check examples/refund_bot/src/main.cor`
- `corvid test examples/refund_bot/tests/unit.cor`
- `corvid replay examples/refund_bot/traces/refund_bot_approval_gate.jsonl`

The reference moat result is the compile-time rejection: a source variant that
calls `issue_refund` without the matching approval fails before execution.
