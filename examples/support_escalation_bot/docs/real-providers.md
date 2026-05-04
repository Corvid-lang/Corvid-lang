# Support Escalation Bot Real Providers

The support escalation bot defaults to deterministic mock mode. Real-provider
mode is never enabled by `cargo test`, `corvid test`, or `corvid run` on a
clean clone.

## Modes

- `mock`: default offline mode. The three tools read deterministic values from
  `CORVID_TEST_MOCK_TOOLS`.
- `replay`: deterministic regression mode. The app replays committed traces
  under `traces/` or the mirrored fixtures under `seed/traces/`.
- `real`: live provider mode. The app may read a real order store, create a
  real refund receipt, and send a real escalation message only when
  `CORVID_RUN_REAL=1` and every provider-specific variable is present.

## Required Environment

Real mode requires the global opt-in:

```text
CORVID_RUN_REAL=1
```

Configure the order lookup provider:

```text
SUPPORT_ORDER_DB_URL=postgres://replace-with-host/order_lookup
SUPPORT_ORDER_DB_USER=replace-with-user
SUPPORT_ORDER_DB_PASSWORD=replace-with-secret-from-operator-store
```

Configure the refund provider:

```text
REFUND_PROVIDER_BASE_URL=https://refund-provider.example.invalid
REFUND_PROVIDER_TOKEN=replace-with-secret-from-operator-store
```

Configure human escalation:

```text
SLACK_WEBHOOK_URL=https://hooks.slack.example.invalid/services/replace-with-secret
SUPPORT_ESCALATION_CHANNEL=#support-escalations
```

The app must fail closed if `CORVID_RUN_REAL=1` is set without any required
provider variable.

## Minimum Provider Versions

- PostgreSQL 14 or newer for the order lookup example.
- Refund provider API version `2026-05-04` or newer with idempotency keys.
- Slack incoming webhooks or a compatible internal escalation endpoint.

## Provider Contract

Mock, replay, and real modes share the same `SupportOutcome` app-facing shape:

```json
{
  "order_id": "ord_1001",
  "action": "escalate_to_human",
  "status": "queued",
  "audit_id": "esc_9001"
}
```

Tool-specific provider metadata, request ids, latency, and raw response bodies
belong in redacted traces or provider logs. They must not change the
`SupportOutcome` fields for the committed seed path.

## CI Policy

CI runs mock and replay only. A future live-provider workflow must require
`CORVID_RUN_REAL=1`, load credentials through the CI secret store, redact any
recorded trace, and keep raw customer, refund, and Slack payloads out of
committed fixtures.
