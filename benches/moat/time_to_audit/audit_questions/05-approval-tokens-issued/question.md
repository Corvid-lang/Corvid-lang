# Q05 — Approval tokens issued

For each `approval_token_issued` event in any trace, extract:

- `label`     — the approval-site label (e.g. `IssueRefund`)
- `args`      — the args the token was issued for
- `scope`     — the token scope (e.g. `one_time`)
- `order_id`  — the `args[0]` (order id) for grouping
- `user_id`   — the user_id from the trace's run-started args

Token IDs and timestamps are excluded from the answer because
they are deliberately fresh per issuance.

The answer is a JSON array sorted by `order_id` (ASCII).

## Why this matters

A regulator auditing approval issuance asks "how many one-time
tokens were issued in the past 24h, for what actions, on whose
authorization?" Without a stable `approval_token_issued` event in
the trace surface, the auditor has to reconstruct token issuance
from request/response pairs by hand.
