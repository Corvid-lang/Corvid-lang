CREATE TABLE app_tenants (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_ms INTEGER NOT NULL
);

CREATE TABLE app_users (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES app_tenants(id),
    email TEXT NOT NULL,
    role TEXT NOT NULL
);

CREATE TABLE app_jobs (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES app_tenants(id),
    kind TEXT NOT NULL,
    status TEXT NOT NULL,
    replay_key TEXT NOT NULL
);
