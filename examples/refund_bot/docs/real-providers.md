# Refund Bot Real Providers

The refund bot defaults to deterministic mock mode. Real provider mode is
never enabled by `cargo test`, `corvid test`, or `corvid run` on a clean clone.

## Modes

- `mock`: default offline mode. The app returns the committed
  `RefundProviderResult` surface from `seed/data/refund_provider_modes.json`.
- `replay`: deterministic regression mode. The app replays
  `seed/traces/refund_bot_approval_gate.jsonl` and expects the same typed
  result shape as mock mode.
- `real`: live provider mode. The app may contact an operator-owned refund
  service only when every required environment variable below is present.

## Required Environment

Real mode requires all of:

```text
CORVID_RUN_REAL=1
REFUND_PROVIDER_ENDPOINT=https://refund-provider.example.invalid
REFUND_PROVIDER_TOKEN=replace-with-secret-from-operator-store
```

`REFUND_PROVIDER_TOKEN` must come from the operator's secret store or shell
environment. It must not be committed to seed data, trace fixtures, README
examples, or CI workflow files.

## Provider Contract

The provider must accept the same request shape as `RefundRequest`:

```json
{
  "order_id": "order-demo-1001",
  "amount": 42.5,
  "reason": "duplicate charge in demo fixture"
}
```

The provider must return the same typed surface as `RefundProviderResult`:

```json
{
  "receipt_id": "rf_mock_1001",
  "status": "approved",
  "audit_id": "audit_mock_1001",
  "guarantee_id": "approval.reachable_entrypoints_require_contract",
  "amount": 42.5
}
```

Mock, replay, and real mode must keep this surface byte-compatible for the
seed request. If the real provider adds fields, the adapter strips them before
returning to Corvid code.

## CI Policy

CI runs mock and replay only. A future live-provider workflow must require
`CORVID_RUN_REAL=1`, load `REFUND_PROVIDER_TOKEN` from CI secrets, redact trace
output before upload, and refuse to persist raw provider payloads.
