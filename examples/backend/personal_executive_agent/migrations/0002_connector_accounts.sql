CREATE TABLE executive_connector_accounts (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES executive_tenants(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    scope TEXT NOT NULL,
    mode TEXT NOT NULL CHECK (mode IN ('mock', 'replay', 'real')),
    approval_required INTEGER NOT NULL CHECK (approval_required IN (0, 1)),
    replay_policy TEXT NOT NULL,
    token_state_ref TEXT NOT NULL,
    last_refresh_at TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, provider, scope)
);
