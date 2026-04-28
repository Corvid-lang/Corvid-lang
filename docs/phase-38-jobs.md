# Phase 38 Jobs, Schedules, And Durable Agent Execution

Phase 38 makes long-lived backend work a Corvid core surface instead of an
application-side convention. The production target is a Personal Executive Agent
that can create daily briefs, prepare meetings, triage inbox work, and follow up
after meetings without losing state across process restarts.

## Goals

- Persist every job before execution.
- Make retry, delay, cron, leases, idempotency, budget, approval waits, and replay
  visible in typed Corvid metadata.
- Keep dangerous AI work behind an approval boundary before a worker can execute
  the side effect.
- Give operators local commands to pause, drain, inspect, retry, cancel, and
  export job traces.

## Queue Semantics

Jobs are append-only records with explicit lifecycle transitions:

- `pending`: accepted and waiting for eligibility.
- `leased`: claimed by one worker until lease expiry.
- `running`: executing inside a bounded runtime context.
- `approval_wait`: paused until a human approves, denies, or the wait expires.
- `succeeded`: completed with typed output metadata.
- `retry_wait`: failed but scheduled for another attempt.
- `dead_lettered`: terminal failure with retry evidence.
- `canceled`: operator or application requested stop before completion.

Workers must not execute a job that is not persisted. A job claim is valid only
while the lease is held. If a process crashes, expired leases return eligible jobs
to `pending` or `retry_wait` according to the last durable transition.

## Durability Model

The v1 implementation is a single-backend durable queue. SQLite is the first
local state store, with the Phase 37 Postgres subset as the production migration
target. Distributed cross-service orchestration remains post-v1.0 scope.

Each persisted job stores:

- stable job id and queue name
- job kind and typed input fingerprint
- idempotency key
- status and attempt counters
- delay/cron eligibility timestamp
- lease owner and lease expiry
- retry/backoff policy
- budget limits
- effect summary and approval requirement
- provenance policy
- trace id and replay key
- redacted output or failure summary

## Scheduler Model

Delayed jobs are ordinary jobs with a future eligibility timestamp. Cron schedules
are durable manifests with owner, schedule expression, target job kind, max runtime,
budget, effect summary, approval policy, and replay policy. Missed schedules after
restart are recovered deterministically according to a catch-up policy:

- `skip_missed`: schedule only the next future occurrence.
- `enqueue_one`: enqueue one recovery job for the latest missed occurrence.
- `enqueue_all_bounded`: enqueue missed jobs up to a configured maximum.

Every cron manifest must appear in `corvid audit` so recurring AI work can be
reviewed before deploy.

## Approval Waits

Jobs that request dangerous effects can enter `approval_wait` with a durable
approval id, approver policy, expiry, and audit record. Approval transitions are
state changes, not worker-local callbacks:

- `approved`: job becomes eligible to resume.
- `denied`: job stops with an audited denial.
- `expired`: job stops or escalates according to policy.

No raw prompt, token, connector secret, or unredacted tool payload may be stored in
the approval wait record.

## Replay Behavior

Replay records summarize job execution without storing secrets or full database
rows. Each attempt records deterministic metadata: job id, attempt number, worker
id, input fingerprint, effect summary, tool-call fingerprints, approval transition
ids, DB replay summaries, output fingerprint, and terminal status.

Replay must be useful for debugging and audit, but it is not a hidden second job
runner. Replays should explain what happened and allow deterministic comparison of
metadata. They should not re-run dangerous effects.

## Loop Bounds

Agent-backed jobs must declare max steps, wall-clock limit, spend limit, and tool
call limit. Exceeding a bound moves the job to a terminal or escalation state with
trace evidence. A loop cannot rely on model self-discipline as its only guard.

## Operator Commands

Phase 38 will add local-first commands:

- `corvid jobs enqueue`
- `corvid jobs run-one`
- `corvid jobs inspect`
- `corvid jobs retry`
- `corvid jobs cancel`
- `corvid jobs pause`
- `corvid jobs drain`
- `corvid jobs export-trace`
- `corvid jobs dlq`

The command output must be redacted by default and stable enough for CI checks.

## Non-Scope

- Distributed multi-service workflow graphs.
- Exactly-once side effects across external APIs.
- A hosted job control plane.
- Visual workflow editing.
- Provider-specific email/calendar connector implementations.

The design still supports those later by keeping job state, effect metadata,
approval waits, and replay summaries explicit.

## Benchmark Posture

Phase 38 should be measured against Celery, BullMQ, Sidekiq-style queues, and
Temporal-style workflows. Corvid should not claim to beat them on distributed
workflow maturity in v1. The benchmark target is AI-backend safety: typed job
metadata, approval waits, replay keys, cost/step limits, effect summaries, and
deploy-time visibility as language-level surfaces.
