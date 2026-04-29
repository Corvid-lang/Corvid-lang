CREATE TABLE finance_payment_intents (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    source_account_id TEXT NOT NULL REFERENCES finance_accounts(id) ON DELETE CASCADE,
    payee_fingerprint TEXT NOT NULL,
    amount_cents INTEGER NOT NULL CHECK (amount_cents > 0),
    currency TEXT NOT NULL,
    approval_label TEXT NOT NULL,
    status TEXT NOT NULL,
    non_advice INTEGER NOT NULL CHECK (non_advice IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE finance_audit_records (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    action TEXT NOT NULL,
    status_before TEXT NOT NULL,
    status_after TEXT NOT NULL,
    approval_id TEXT NOT NULL,
    trace_id TEXT NOT NULL,
    redacted INTEGER NOT NULL CHECK (redacted IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
