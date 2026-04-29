CREATE TABLE executive_inbox_threads (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES executive_tenants(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES executive_users(id) ON DELETE CASCADE,
    provider_thread_id TEXT NOT NULL,
    sender TEXT NOT NULL,
    subject_fingerprint TEXT NOT NULL,
    priority TEXT NOT NULL,
    triage_status TEXT NOT NULL,
    replay_key TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, provider_thread_id)
);

CREATE TABLE executive_draft_replies (
    id TEXT PRIMARY KEY,
    thread_id TEXT NOT NULL REFERENCES executive_inbox_threads(id) ON DELETE CASCADE,
    tenant_id TEXT NOT NULL REFERENCES executive_tenants(id) ON DELETE CASCADE,
    body_fingerprint TEXT NOT NULL,
    approval_label TEXT NOT NULL,
    status TEXT NOT NULL,
    replay_key TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE executive_tasks (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES executive_tenants(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES executive_users(id) ON DELETE CASCADE,
    source_kind TEXT NOT NULL,
    title_fingerprint TEXT NOT NULL,
    external_ref TEXT NOT NULL,
    status TEXT NOT NULL,
    replay_key TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE executive_follow_ups (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES executive_tenants(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES executive_users(id) ON DELETE CASCADE,
    thread_id TEXT NOT NULL REFERENCES executive_inbox_threads(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES executive_tasks(id) ON DELETE CASCADE,
    due_at TEXT NOT NULL,
    status TEXT NOT NULL,
    approval_label TEXT NOT NULL,
    replay_key TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
