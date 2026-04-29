# Finance Operations Agent Runbook

1. Keep `CORVID_REGULATED_ADVICE=false`.
2. Run `corvid check examples/backend/finance_operations_agent/src/main.cor`.
3. Apply migrations and load `seeds/demo.sql`.
4. Run `corvid eval examples/backend/finance_operations_agent/evals/payment_audit_eval.cor`.
5. Treat payment outputs as approval-gated intents only.
