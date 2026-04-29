CREATE TABLE code_write_plans (
    id TEXT PRIMARY KEY,
    issue_id TEXT NOT NULL REFERENCES code_issues(id) ON DELETE CASCADE,
    review_comment_fingerprint TEXT NOT NULL,
    patch_fingerprint TEXT NOT NULL,
    approval_count INTEGER NOT NULL CHECK (approval_count >= 0),
    writes_gated INTEGER NOT NULL CHECK (writes_gated IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE code_approval_audits (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    action TEXT NOT NULL,
    approval_id TEXT NOT NULL,
    trace_id TEXT NOT NULL,
    redacted INTEGER NOT NULL CHECK (redacted IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
