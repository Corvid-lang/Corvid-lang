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

The response types are part of the Corvid source, not hand-written host code.
