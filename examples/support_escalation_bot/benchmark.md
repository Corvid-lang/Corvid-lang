# Support Escalation Bot Benchmark Notes

This demo is performance-relevant at the multi-tool boundary: one order lookup
plus either a human escalation write or an approval-gated refund write.

Local smoke measurements should focus on:

- `corvid run` with `CORVID_TEST_MOCK_TOOLS`
- `corvid test examples/support_escalation_bot/tests/unit.cor`
- `corvid eval examples/support_escalation_bot/evals/support_escalation_bot.cor`
- `corvid replay examples/support_escalation_bot/traces/support_escalation_bot_escalation.jsonl`
- `corvid replay examples/support_escalation_bot/traces/support_escalation_bot_approved_refund.jsonl`

Real provider latency depends on database location, refund provider latency,
Slack API latency, and approval wait time. Keep provider latency notes separate
from committed mock fixtures so CI remains deterministic.
