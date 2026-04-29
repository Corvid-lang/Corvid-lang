# Customer Support Agent Runbook

1. Run `corvid check examples/backend/customer_support_agent/src/main.cor`.
2. Apply migrations and load `seeds/demo.sql`.
3. Run `corvid eval examples/backend/customer_support_agent/evals/support_ops_eval.cor`.
4. Confirm reply sends and refunds stay approval-gated.
5. Preserve SLA replay keys before job changes.
