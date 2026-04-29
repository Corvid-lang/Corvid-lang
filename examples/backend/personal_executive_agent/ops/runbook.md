# Personal Executive Agent Runbook

## Local Boot

1. Export the values in `deploy/env.example`.
2. Run `corvid check examples/backend/personal_executive_agent/src/main.cor`.
3. Run the five migrations in `migrations/` against a local SQLite database.
4. Load `seeds/demo.sql`.
5. Keep `CORVID_CONNECTOR_MODE=mock` unless provider credentials, scopes,
   rate limits, replay policies, and approval sinks are configured.

## Verification

- Run `corvid eval examples/backend/personal_executive_agent/evals/hardening_eval.cor`.
- Inspect `traces/demo.lineage.jsonl` for route, job, and approval spans.
- Confirm `mocks/approval_surface.json` matches the dangerous write routes.
- Confirm every external write route passes through an `approve` statement.

## Operations

- Freeze writes by setting `CORVID_CONNECTOR_MODE=mock`.
- Preserve `/data/traces` before restart or replay.
- Re-run the hardening eval after any route, approval, or connector change.
- Promote new redacted trace fixtures before changing job behavior.

## Non-Goals

This reference app does not send email, edit calendars, write tasks, or send
chat messages without human approval. Demo mode is offline and deterministic.
