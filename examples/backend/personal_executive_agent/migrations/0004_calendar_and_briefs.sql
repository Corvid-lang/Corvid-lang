CREATE TABLE executive_calendar_events (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES executive_tenants(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES executive_users(id) ON DELETE CASCADE,
    provider_event_id TEXT NOT NULL,
    starts_at TEXT NOT NULL,
    ends_at TEXT NOT NULL,
    attendee_fingerprint TEXT NOT NULL,
    status TEXT NOT NULL,
    replay_key TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, provider_event_id)
);

CREATE TABLE executive_meeting_prep_packets (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES executive_tenants(id) ON DELETE CASCADE,
    event_id TEXT NOT NULL REFERENCES executive_calendar_events(id) ON DELETE CASCADE,
    source_thread_count INTEGER NOT NULL CHECK (source_thread_count >= 0),
    source_file_count INTEGER NOT NULL CHECK (source_file_count >= 0),
    packet_fingerprint TEXT NOT NULL,
    replay_key TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE executive_daily_briefs (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL REFERENCES executive_tenants(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES executive_users(id) ON DELETE CASCADE,
    brief_date TEXT NOT NULL,
    priority_count INTEGER NOT NULL CHECK (priority_count >= 0),
    meeting_count INTEGER NOT NULL CHECK (meeting_count >= 0),
    task_count INTEGER NOT NULL CHECK (task_count >= 0),
    output_fingerprint TEXT NOT NULL,
    replay_key TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, user_id, brief_date)
);
