INSERT INTO finance_accounts
    (id, tenant_id, provider, display_name, balance_cents, currency, data_fingerprint)
VALUES
    ('acct-1', 'tenant-1', 'mock_bank', 'Operating', 250000, 'USD', 'sha256:account-demo');

INSERT INTO finance_budgets
    (id, tenant_id, category, monthly_limit_cents, spent_cents, status)
VALUES
    ('budget-1', 'tenant-1', 'software', 50000, 42000, 'watch');

INSERT INTO finance_subscriptions
    (id, tenant_id, merchant, amount_cents, cadence, next_due_at, status)
VALUES
    ('sub-1', 'tenant-1', 'Cloud Tools', 9900, 'monthly', '2026-05-01', 'active');

INSERT INTO finance_reminders
    (id, tenant_id, kind, due_at, message_fingerprint)
VALUES
    ('reminder-1', 'tenant-1', 'subscription_renewal', '2026-04-30', 'sha256:renewal-reminder');

INSERT INTO finance_anomalies
    (id, tenant_id, account_id, category, amount_cents, confidence, explanation_fingerprint)
VALUES
    ('anomaly-1', 'tenant-1', 'acct-1', 'software', 18000, 0.86, 'sha256:anomaly-explanation');
