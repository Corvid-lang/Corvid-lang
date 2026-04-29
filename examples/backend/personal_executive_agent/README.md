# Personal Executive Agent Backend

This example is the Phase 38 production job-contract fixture for Corvid's durable
agent runtime. It is intentionally backend-only: every workflow is expressed as
typed Corvid source, durable job metadata, replay keys, idempotency keys, effect
rows, and audited schedules.

## Jobs

- `daily_brief_job` reads inbox and calendar context, runs a bounded executive
  planning step, and emits a redacted brief output envelope.
- `meeting_prep_job` prepares meeting context from inbox and calendar context.
- `email_triage_job` classifies inbox work into follow-up, archive, and task
  candidates without sending anything externally.
- `follow_up_job` drafts outbound follow-up work and requires the
  `SendExecutiveFollowUp` approval boundary before the external send effect can
  run.

## Production Contract

Each job carries:

- a stable queue name: `personal_executive_agent`
- a deterministic idempotency key
- a deterministic replay key
- a retry policy with bounded exponential jitter
- a budget cap
- redacted input and output fingerprints
- an effect envelope with provenance, cache, approval, and replay metadata

The schedules in `src/main.cor` are first-class `schedule` declarations so
`corvid audit` can report the cron manifest directly from source.
