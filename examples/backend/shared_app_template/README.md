# Shared Backend App Template

This template is the Phase 42 base shape for production reference apps. It keeps
routes, config, health/readiness, and route documentation explicit before adding
state, jobs, connectors, approvals, traces, and deployment manifests in later
slices.

## Build

```sh
corvid check examples/backend/shared_app_template/src/main.cor
corvid build examples/backend/shared_app_template/src/main.cor --target=server
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
