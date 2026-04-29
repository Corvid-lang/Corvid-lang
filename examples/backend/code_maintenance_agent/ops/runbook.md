# Code Maintenance Agent Runbook

1. Run `corvid check examples/backend/code_maintenance_agent/src/main.cor`.
2. Apply migrations and load `seeds/demo.sql`.
3. Run `corvid eval examples/backend/code_maintenance_agent/evals/write_approval_eval.cor`.
4. Confirm review comments and patch proposals stay approval-gated.
5. Preserve CI signal fingerprints before changing risk labeling.
