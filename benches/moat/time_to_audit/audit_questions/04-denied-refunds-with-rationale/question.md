# Q04 — Denied refunds with LLM rationale

For each trace where the LLM declined the refund (the
`decide_refund` `llm_result` event has
`result.should_refund == false`), extract:

- `order_id` — the order that was *not* refunded
- `user_id`  — the user whose request was denied
- `reason`   — the `result.reason` field from the LLM result

The answer is a JSON array sorted by `order_id` (ASCII).

Traces where the LLM approved the refund (`should_refund: true`)
are excluded — Q04 is the negative-decision query.

## Why this matters

Audit teams need to see the *negative* decisions as much as the
positive ones — they're often where customer complaints originate
and where bias / fairness concerns land. The trace must carry
the LLM's stated reason for each denial in a typed, queryable
field.
