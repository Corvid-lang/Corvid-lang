# Corvid Developer Production Guide

This guide is the production path for backend developers using Corvid. It assumes the developer wants a real backend service with migrations, jobs, approvals, connectors, traces, deployment artifacts, and release checks.

## Backend Tutorial

1. Create or open an app under `examples/backend/<app>/`.
2. Put the service source in `src/main.cor`.
3. Define effects before tools and routes.
4. Use `server Name:` with route declarations for HTTP entrypoints.
5. Store migrations under `migrations/` and run:

```bash
corvid check examples/backend/refund_api/src/refund_api.cor
corvid build examples/backend/refund_api/src/refund_api.cor --target=server
corvid migrate status --dir examples/backend/refund_api/migrations
corvid deploy package examples/backend/refund_api --out target/refund_api-package
```

The minimum production backend has a checked source file, migration status, deployment package, env schema, health/readiness config, and signed release or deploy attestation.

## Personal Executive Agent Tutorial

The Personal Executive Agent is the reference app for high-value personal-agent workflows:

- daily brief generation,
- meeting prep,
- inbox triage,
- follow-up drafting,
- calendar scheduling,
- task updates,
- approval-gated external writes.

Run the production-shaped checks:

```bash
corvid check examples/backend/personal_executive_agent/src/main.cor
corvid audit examples/backend/personal_executive_agent/src/main.cor --json
corvid deploy compose examples/backend/personal_executive_agent --out target/pea-compose
corvid deploy k8s examples/backend/personal_executive_agent --out target/pea-k8s
```

External write tools are marked `dangerous` and must sit behind approval routes. Durable schedules are declared in source and are included in signed claim coverage.

## Connector Guide

Connector manifests must declare:

- provider scope,
- data classes,
- approval requirement for writes,
- replay policy,
- rate limits,
- sensitive redaction rules.

Use mock or replay mode during development. Move to real provider mode only after scope minimization, write approval, webhook signature verification, and rate-limit behavior have tests or explicit non-scope notes.

## Approval Guide

Use approvals for every external write, money movement, irreversible message, data deletion, or privileged tenant action.

Production approval surfaces need:

- a typed approval request,
- a dangerous tool,
- an `approve` boundary,
- an audit record,
- a denial path,
- an expiry path,
- a replay key.

The approval route should return the proposed action and evidence, not execute the action before review.

## Observability Guide

Production services should emit:

- request id,
- trace id,
- route or job name,
- effect names,
- approval status,
- cost and token counters when LLMs are used,
- replay key,
- connector mode,
- migration state.

Use traces for replay and claim audit. Do not log plaintext API keys, connector tokens, approval secrets, or raw sensitive data classes without redaction.

## Production Checklist

Before shipping a Corvid backend:

- `corvid check` passes,
- migrations run and drift detection is clean,
- `corvid upgrade check` is clean,
- dangerous tools have approval coverage,
- deploy package exists,
- Compose/PaaS/Kubernetes/systemd manifest exists for the target runtime,
- env schema is complete,
- health and readiness endpoints are configured,
- connector mode is explicit,
- traces are enabled or explicitly disabled,
- release artifacts are signed,
- launch claims have runnable evidence.

## No-Prototype Rule

A Corvid app is not production-shaped until it has source checks, migrations, deployment artifacts, operational docs, approval boundaries for dangerous work, and a claim/audit path. Demo-only mocks are allowed only when they are clearly isolated from production connector mode.
