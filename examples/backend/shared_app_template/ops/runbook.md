# Shared Backend App Template Runbook

## Purpose

This runbook defines the minimum operator contract every Phase 42 reference app
inherits: deterministic mock mode, explicit database migrations, trace capture,
eval promotion, approval-gated writes, and deployable local packaging.

## Local Boot

1. Copy `deploy/env.example` into the shell environment.
2. Run `corvid check examples/backend/shared_app_template/src/main.cor`.
3. Run `corvid migrate status --dir examples/backend/shared_app_template/migrations --state examples/backend/shared_app_template/target/migrations.json`.
4. Run `corvid build examples/backend/shared_app_template/src/main.cor --target=server`.
5. Start the generated server and verify `GET /healthz`, `GET /readyz`, and `GET /routes`.

## Deployment

1. Build the container from `deploy/docker-compose.yml`.
2. Keep `CORVID_CONNECTOR_MODE=mock` until provider credentials, rate limits,
   replay policies, and approval sinks are configured.
3. Mount persistent storage at `/data` for database state and traces.
4. Expose only the HTTP port required by the runtime environment.

## Operational Checks

- Health: `GET /healthz` returns `status = ok`.
- Readiness: `GET /readyz` returns database, connector, and migration status.
- Route manifest: `GET /routes` shows all public backend routes and effects.
- Eval: `corvid eval examples/backend/shared_app_template/evals/template_eval.cor`.
- Trace fixture: `examples/backend/shared_app_template/traces/demo.lineage.jsonl`
  stays redacted and deterministic.

## Incident Response

1. Freeze external writes by setting `CORVID_CONNECTOR_MODE=mock`.
2. Preserve the trace directory before restart.
3. Run evals against the last known-good trace bundle.
4. Inspect approval records before replaying any operation with side effects.
5. Promote a minimal redacted regression fixture before shipping the fix.

## Production Promotion Gate

Promotion from template to an app-specific backend requires:

- App-specific migrations and seed fixtures.
- At least three connector mocks.
- Approval contracts for every external write.
- Replay evidence for all durable jobs.
- Operator-owned secrets outside source control.
