# Support Escalation Bot Runbook

This runbook is for operators running the support escalation bot as a reference
app. The default operating mode is deterministic mock mode; real order,
refund, and escalation providers are explicitly opt-in.

## Deploy

1. Clone the repository and enter `examples/support_escalation_bot`.
2. Keep real mode disabled for first boot:

   ```text
   CORVID_RUN_REAL=0
   ```

3. Configure deterministic mock mode:

   ```text
   CORVID_TEST_MOCK_TOOLS={"lookup_order":[{"id":"ord_1001","customer_id":"cust_42","status":"delivered","total":149.99},{"id":"ord_1003","customer_id":"cust_42","status":"delivered","total":19.95}],"escalate_to_human":{"ticket_id":"esc_9001","status":"queued","channel":"slack"},"issue_refund":{"receipt_id":"rf_7001","status":"approved","audit_id":"audit_refund_7001"}}
   ```

4. Build and run the app:

   ```text
   cargo run -q -p corvid-cli -- build
   cargo run -q -p corvid-cli -- run
   ```

5. Verify the one-command output is an `escalate_to_human` outcome with
   `audit_id` set to `esc_9001`.
6. Run the local hardening checks from the repository root:

   ```text
   cargo run -q -p corvid-cli -- test examples/support_escalation_bot/tests/unit.cor
   cargo run -q -p corvid-cli -- test examples/support_escalation_bot/tests/integration.cor
   cargo run -q -p corvid-cli -- test examples/support_escalation_bot/tests/replay_invariant.cor
   cargo run -q -p corvid-cli -- eval examples/support_escalation_bot/evals/support_escalation_bot.cor
   cargo run -q -p corvid-cli -- replay examples/support_escalation_bot/traces/support_escalation_bot_escalation.jsonl
   cargo run -q -p corvid-cli -- replay examples/support_escalation_bot/traces/support_escalation_bot_approved_refund.jsonl
   cargo run -q -p corvid-cli -- replay examples/support_escalation_bot/seed/traces/support_escalation_bot_escalation.jsonl
   cargo run -q -p corvid-cli -- replay examples/support_escalation_bot/seed/traces/support_escalation_bot_approved_refund.jsonl
   ```

7. Confirm the approval-denied fixtures fail closed:

   ```text
   cargo run -q -p corvid-cli -- replay examples/support_escalation_bot/traces/support_escalation_bot_approval_denied.jsonl
   cargo run -q -p corvid-cli -- replay examples/support_escalation_bot/seed/traces/support_escalation_bot_approval_denied.jsonl
   ```

8. For real mode, configure every variable in `docs/real-providers.md` from an
   operator-owned secret store before setting `CORVID_RUN_REAL=1`.

## Observe

1. Preserve every generated trace directory under `target/trace/` for the run
   being investigated.
2. Confirm all committed replay fixtures still pass or fail with their expected
   outcome in a plain replay environment with provider env cleared.
3. Compare order lookup output to `seed/data/orders.json` and mode expectations
   to `seed/data/tool_modes.json`.
4. Check that every app-facing support outcome includes:

   ```text
   order_id
   action
   status
   audit_id
   ```

5. Treat a missing `audit_id`, unexpected `issue_refund` action, or successful
   approval-denied replay as an incident.

## Rollback

1. Disable real provider mode immediately:

   ```text
   CORVID_RUN_REAL=0
   ```

2. Clear provider env that affects real calls or plain replay:

   ```text
   CORVID_TEST_MOCK_TOOLS=
   SUPPORT_ORDER_DB_URL=
   SUPPORT_ORDER_DB_USER=
   SUPPORT_ORDER_DB_PASSWORD=
   REFUND_PROVIDER_BASE_URL=
   REFUND_PROVIDER_TOKEN=
   SLACK_WEBHOOK_URL=
   SUPPORT_ESCALATION_CHANNEL=
   ```

3. Re-run all committed replay fixtures from the repository root.
4. Restore the last known-good commit that passed `demo-verify.yml`.
5. Re-run the replay invariant and adversarial guarantee harness before
   re-enabling any real provider mode.

## Incident Response

1. Stop live refund calls by removing `REFUND_PROVIDER_TOKEN` and setting
   `CORVID_RUN_REAL=0`.
2. Stop Slack or internal escalation writes by removing `SLACK_WEBHOOK_URL`.
3. Preserve traces, seed files, shell environment names, and commit SHA. Do not
   preserve raw provider credentials, customer PII, or Slack payloads outside
   the redacted incident bundle.
4. Run the adversarial guarantee-id harness:

   ```text
   cargo test -p corvid-cli --test demo_project_defaults support_escalation_bot_adversarial_cases_carry_registered_guarantee_ids -- --nocapture
   ```

5. If the harness fails, block deployment until the approval-boundary rejection
   path is restored.
6. If the harness passes but a live provider moved money unexpectedly, handle it
   as a provider or operator-approval incident. Corvid enforces the dangerous
   tool approval boundary; settlement and provider fraud controls are outside
   this demo's trust boundary.
7. Add a minimal redacted fixture reproducing the issue before shipping the
   fix.

## Release Checklist

- `cargo check --workspace` passes.
- `cargo test -p corvid-cli --test demo_project_defaults` passes.
- `cargo run -q -p corvid-cli -- verify --corpus tests/corpus` returns the
  established platform baseline.
- The credential-pattern scan over `examples/support_escalation_bot` returns no
  matches.
- `CORVID_RUN_REAL=1` is absent from default CI.
- Plain replay is run with provider env cleared, and approval-denied replay
  remains nonzero.
