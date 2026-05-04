# Support Escalation Bot

Customer support agent with typed tool calls, human escalation, and an
approval-gated refund path.

## Setup

From this directory:

```sh
set CORVID_TEST_MOCK_TOOLS={"lookup_order":[{"id":"ord_1001","customer_id":"cust_42","status":"delivered","total":149.99},{"id":"ord_1003","customer_id":"cust_42","status":"delivered","total":19.95}],"escalate_to_human":{"ticket_id":"esc_9001","status":"queued","channel":"slack"},"issue_refund":{"receipt_id":"rf_7001","status":"approved","audit_id":"audit_refund_7001"}}
cargo run -q -p corvid-cli -- build
cargo run -q -p corvid-cli -- run
```

On macOS or Linux, use `export` instead of `set`.

Expected output includes:

```text
SupportOutcome(... action: "escalate_to_human" ... audit_id: "esc_9001" ...)
```

## What It Shows

- `lookup_order`, `issue_refund`, and `escalate_to_human` share typed tool
  surfaces across mock, replay, and future real provider mode.
- `issue_refund` is dangerous and cannot be called without an explicit
  `approve IssueRefund(...)` statement.
- The one-command `main` path stays noninteractive by escalating to a human.
- Replay fixtures cover escalation, approved refund, and approval denial.

## Verify

From the repository root:

```sh
set CORVID_TEST_MOCK_TOOLS={"lookup_order":[{"id":"ord_1001","customer_id":"cust_42","status":"delivered","total":149.99},{"id":"ord_1003","customer_id":"cust_42","status":"delivered","total":19.95}],"escalate_to_human":{"ticket_id":"esc_9001","status":"queued","channel":"slack"},"issue_refund":{"receipt_id":"rf_7001","status":"approved","audit_id":"audit_refund_7001"}}
cargo run -q -p corvid-cli -- test examples/support_escalation_bot/tests/unit.cor
cargo run -q -p corvid-cli -- test examples/support_escalation_bot/tests/integration.cor
cargo run -q -p corvid-cli -- eval examples/support_escalation_bot/evals/support_escalation_bot.cor
cargo run -q -p corvid-cli -- replay examples/support_escalation_bot/traces/support_escalation_bot_escalation.jsonl
cargo run -q -p corvid-cli -- replay examples/support_escalation_bot/traces/support_escalation_bot_approved_refund.jsonl
```

The approval-denied trace is a negative replay fixture and exits nonzero with
`approval denied for IssueRefund`.

## How To Modify

Add new order scenarios under `seed/`, update the mock tool queue, then update
the tests, eval assertions, and replay fixtures together. Keep any new
money-moving tool behind an explicit `approve` statement and add a compile
rejection test for the unapproved variant.
