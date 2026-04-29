# Q02 — Refunds over $50

Same shape as Q01, but filter to refunds where `amount > 50.0`.

The answer is a JSON array of the same record schema as Q01,
sorted by `order_id` (ASCII).

## Why this matters

The threshold-based query is the second most common audit shape:
"show me everything above the materiality line." Every regulated
domain has one — refunds, payments, data-export volume,
cross-border transfers. The audit query must be able to read
amount as a typed number, not as a string parsed from a log line.
