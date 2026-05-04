# Support Escalation Bot Real Providers

Real provider mode is opt-in. CI and normal development use deterministic mock
tool responses through `CORVID_TEST_MOCK_TOOLS`.

## Order Database

Environment:

```sh
CORVID_RUN_REAL=1
SUPPORT_DB_URL=postgres://support_app:<redacted-password>@localhost:5432/support
SUPPORT_DB_SCHEMA=public
```

The real `lookup_order` tool must return the same `Order` shape used by the
mock and replay fixtures:

```json
{"id":"ord_1001","customer_id":"cust_42","status":"delivered","total":149.99}
```

## Refund Provider

Environment:

```sh
CORVID_RUN_REAL=1
REFUND_PROVIDER_URL=https://refunds.example.internal
REFUND_PROVIDER_TOKEN=<redacted-refund-token>
```

`issue_refund` is approval-gated in Corvid before the provider call. The real
provider must return `receipt_id`, `status`, and `audit_id`.

## Human Escalation

Environment:

```sh
CORVID_RUN_REAL=1
SLACK_BOT_TOKEN=<redacted-slack-token>
SUPPORT_ESCALATION_CHANNEL=C0123456789
```

`escalate_to_human` must return a deterministic `ticket_id`/`status`/`channel`
surface to match mock and replay mode.

## Mock Mode

Offline tests and CI use:

```sh
CORVID_TEST_MOCK_TOOLS={"lookup_order":[{"id":"ord_1001","customer_id":"cust_42","status":"delivered","total":149.99},{"id":"ord_1003","customer_id":"cust_42","status":"delivered","total":19.95}],"escalate_to_human":{"ticket_id":"esc_9001","status":"queued","channel":"slack"},"issue_refund":{"receipt_id":"rf_7001","status":"approved","audit_id":"audit_refund_7001"}}
```

Do not enable real mode unless both `CORVID_RUN_REAL=1` and the provider-specific
environment variables are present.
