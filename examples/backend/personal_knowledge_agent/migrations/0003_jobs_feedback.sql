CREATE TABLE knowledge_ingestion_jobs (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    root_id TEXT NOT NULL,
    status TEXT NOT NULL,
    discovered_count INTEGER NOT NULL CHECK (discovered_count >= 0),
    indexed_count INTEGER NOT NULL CHECK (indexed_count >= 0),
    replay_key TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE knowledge_feedback (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    question_fingerprint TEXT NOT NULL,
    answer_fingerprint TEXT NOT NULL,
    accepted INTEGER NOT NULL CHECK (accepted IN (0, 1)),
    citation_count INTEGER NOT NULL CHECK (citation_count >= 0),
    trace_id TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
