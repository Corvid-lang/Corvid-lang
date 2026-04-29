# Q01 — List every refund issued

For each trace in the corpus where the agent issued a refund (an
`issue_refund` tool call followed by an `ok=true` `run_completed`),
extract:

- `order_id` — the order that was refunded
- `user_id`  — the user the refund was credited to
- `amount`   — the refund amount in USD
- `refund_id` — the refund identifier returned by the `issue_refund` tool
- `llm_rationale` — the `reason` field from the `decide_refund` LLM result that justified the refund

The answer is a JSON array sorted by `order_id` (ASCII).

A trace where `decide_refund` returned `should_refund: false` (or the
approver denied) is excluded — no refund was actually issued.

## Why this matters

This is the most basic audit query: "show me every refund this agent
has issued." A regulator must be able to answer this without writing
custom parsers, joining multiple log streams, or hitting a vendor
cloud. Every required field — order, user, amount, refund-id, LLM
rationale — must reach the auditor through the language's canonical
trace surface.
