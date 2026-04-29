CREATE TABLE code_repositories (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    repo TEXT NOT NULL,
    commit_sha TEXT NOT NULL,
    tree_hash TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE code_issues (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    repo_id TEXT NOT NULL REFERENCES code_repositories(id) ON DELETE CASCADE,
    title_fingerprint TEXT NOT NULL,
    body_fingerprint TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE code_ci_signals (
    id TEXT PRIMARY KEY,
    repo_id TEXT NOT NULL REFERENCES code_repositories(id) ON DELETE CASCADE,
    commit_sha TEXT NOT NULL,
    status TEXT NOT NULL,
    failing_job TEXT NOT NULL,
    log_fingerprint TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE code_risk_labels (
    id TEXT PRIMARY KEY,
    issue_id TEXT NOT NULL REFERENCES code_issues(id) ON DELETE CASCADE,
    ci_signal_id TEXT NOT NULL REFERENCES code_ci_signals(id) ON DELETE CASCADE,
    category TEXT NOT NULL,
    severity TEXT NOT NULL,
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    replay_key TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
