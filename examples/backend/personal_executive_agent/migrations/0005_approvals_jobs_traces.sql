CREATE TABLE executive_job_runs (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES executive_tenants(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES executive_users(id) ON DELETE CASCADE,
    job_kind TEXT NOT NULL,
    status TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    replay_key TEXT NOT NULL,
    budget_usd REAL NOT NULL CHECK (budget_usd >= 0),
    approval_label TEXT NOT NULL,
    trace_id TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, idempotency_key)
);

CREATE TABLE executive_approval_audits (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES executive_tenants(id) ON DELETE CASCADE,
    approval_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    action_kind TEXT NOT NULL,
    resource_kind TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    status_before TEXT NOT NULL,
    status_after TEXT NOT NULL,
    trace_id TEXT NOT NULL,
    replay_key TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
