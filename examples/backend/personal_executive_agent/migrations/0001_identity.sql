CREATE TABLE executive_tenants (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    plan TEXT NOT NULL,
    region TEXT NOT NULL,
    data_retention_days INTEGER NOT NULL CHECK (data_retention_days > 0),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE executive_users (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES executive_tenants(id) ON DELETE CASCADE,
    email TEXT NOT NULL,
    timezone TEXT NOT NULL,
    role TEXT NOT NULL,
    preferences_fingerprint TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, email)
);
