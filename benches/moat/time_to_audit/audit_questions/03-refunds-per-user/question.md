# Q03 — Refunds issued per user

For each `user_id` that appears in the corpus, count the number of
refunds issued (same definition as Q01). The answer is a JSON
object mapping `user_id` to refund count, with keys sorted ASCII.

Users who never received a refund are excluded.

## Why this matters

The grouping / aggregation query is the third common audit shape:
"who is using this agent, how often?" It exercises the trace
surface's ability to expose typed user identifiers consistently
across runs, not just embedded in free-form log strings.
