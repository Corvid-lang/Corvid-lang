# Shared Backend App Template

This template is the Phase 42 base shape for production reference apps. It keeps
routes, config, health/readiness, route documentation, state seams, mock
connectors, traces, evals, deployment, and operator handoff explicit so each
app starts from a production-shaped backend.

## Build

```sh
corvid check examples/backend/shared_app_template/src/main.cor
corvid build examples/backend/shared_app_template/src/main.cor --target=server
corvid eval examples/backend/shared_app_template/evals/template_eval.cor
```

## Routes

- `GET /healthz`
- `GET /readyz`
- `GET /config`
- `GET /routes`
- `GET /jobs/demo`
- `GET /auth/demo`
- `GET /connectors/mock`

The response types are part of the Corvid source, not hand-written host code.

## State

- `migrations/0001_initial.sql` creates tenant, user, and job tables.
- `seeds/demo.sql` inserts deterministic demo rows.
- `/jobs/demo`, `/auth/demo`, and `/connectors/mock` expose the template
  state/auth/connector seams that reference apps fill in later slices.

## Operations

- `evals/template_eval.cor` proves the template contract with deterministic
  value assertions.
- `traces/demo.lineage.jsonl` is a redacted lineage fixture for replay and eval
  promotion flows.
- `deploy/docker-compose.yml` and `deploy/Dockerfile` define the local
  deployment shape.
- `deploy/env.example` documents the required runtime environment.
- `ops/runbook.md` is the operator checklist each reference app extends.
