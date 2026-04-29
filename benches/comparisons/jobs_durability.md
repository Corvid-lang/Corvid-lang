# Phase 38 — durable agent jobs side-by-side

## Headline

For an idiomatic durable-job that triages an inbox, drafts a reply,
waits for human approval, and survives a worker crash mid-step, the
Corvid implementation declares the safety contract in the language and
delegates persistence + retry + approval-wait to the runtime; the
Python (Celery), Node (BullMQ), and Go (Temporal SDK) baselines
re-implement those primitives at the application layer with explicit
glue.

The governance-line delta below is a static, line-by-line count of the
lines that exist *only* for safety / approval / audit / provenance /
replay / confidence — not feature lines.

## Reproduce

The Corvid implementation is `apps/refund_bot/corvid/refund_bot.cor`
plus the `jobs.cron_schedule_durable` row in
`crates/corvid-guarantees/src/lib.rs`. The line count uses the
governance-lines benchmark counter:

```bash
python benches/moat/governance_lines/runner/count.py \
    --apps-dir benches/moat/governance_lines/apps \
    --out      benches/moat/governance_lines/RESULTS.md
```

The Python and TypeScript baselines below are *open for bounty
submission* — see [`docs/effects-spec/bounty.md`](../../docs/effects-spec/bounty.md).
The numbers stay marked `bounty-open` until a submission lands.

## Side-by-side (sketch)

### Corvid

```corvid
@budget($0.20)
@retry(max_attempts: 3, backoff: exponential(base: 30s, cap: 5m))
@idempotency(key: brief.user_id)
@replayable
job daily_brief(user_id: String) uses email_effect, summary_effect:
    inbox = gmail.recent(user_id, since: yesterday())
    summary = summarise(inbox)
    approve SendBrief(user_id, summary)
    gmail.send(user_id, summary)

schedule "0 8 * * *" zone "America/New_York" -> daily_brief(every_user())
```

The compiler rejects the program if `gmail.send` is not behind a
matching `approve` token; the runtime guarantees retry budget,
idempotency-key uniqueness, replayable side-effect quarantine, and
SIGKILL-safe checkpoint resume per the Phase 38 audit-correction
slices (38K/L/M).

### Python (Celery + custom approval middleware) — bounty-open

A Celery-equivalent ships with no first-class approval boundary; the
typical pattern is a custom middleware decorator that checks an
in-memory token map, plus a manually-written audit log entry per
state transition, plus a separate idempotency table managed by the
application, plus chrono-tz-aware cron scheduling supplied by
`celery-beat` with manual DST policy. Submission lands the actual
implementation under `runs/python/` (parallel to the existing
governance_lines benchmark structure).

### TypeScript (BullMQ + custom approval middleware) — bounty-open

BullMQ ships durable queues, retries, and a worker abstraction; it
does not ship approval boundaries, idempotency-key conventions,
audit logging, or DST-aware cron policy. The application layer
adds those, typically in a few hundred lines of Express/Fastify
middleware + Redis state. Submission lands under `runs/typescript/`.

### Go (Temporal SDK) — bounty-open

Temporal ships durable workflows + activity retries + signal-based
human-in-the-loop. The application layer still adds the
approval-contract typing (target / max-cost / required-role / data
class / expiry), audit log, and cost telemetry. Submission lands
under `runs/go/`.

## Governance line count

| Implementation | Feature lines | Governance lines | Total | % governance |
|---|---|---|---|---|
| Corvid (`apps/refund_bot/corvid/`) | 18 | 9 | 27 | 33.3% |
| Python (Celery) | bounty-open | bounty-open | — | — |
| Node (BullMQ) | bounty-open | bounty-open | — | — |
| Go (Temporal) | bounty-open | bounty-open | — | — |

Numbers regenerate from
[`benches/moat/governance_lines/RESULTS.md`](../moat/governance_lines/RESULTS.md).

## What Corvid wins on

- **Approval boundary at typecheck**: removing the `approve` line
  fails to compile in Corvid. The Celery / BullMQ / Temporal
  baselines surface that requirement only at runtime, if at all.
- **Idempotency typed**: `@idempotency(key: ...)` is the language
  surface; the baselines manage idempotency keys via a separate
  table the application owns.
- **Replay quarantine typed**: `@replayable` causes the runtime
  to quarantine outbound side-effects in replay mode. The
  baselines' replay story is "re-run the workflow with logging on,"
  with no compile-time prevention of double-spend.
- **Cron + DST in the language**: `schedule "0 8 * * *" zone
  "America/New_York"` is a typed declaration; the baselines
  configure cron timing in deployment YAML or a separate
  scheduler service.

## What Corvid does not claim

- **Raw worker throughput** is comparable to Temporal at scale; the
  Corvid worker pool (slice 38K) targets correctness and resume
  semantics, not millions of jobs per second.
- **Distributed execution across services** is post-v1.0; Corvid's
  worker pool is single-backend (SQLite default, Postgres
  configurable).
- **The `bounty-open` cells above are not yet measured.** They
  remain `bounty-open` until a submission lands; the headline
  above describes the expected dimension of the win, not a
  measured number.
