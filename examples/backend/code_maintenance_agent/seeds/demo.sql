INSERT INTO code_repositories
    (id, tenant_id, provider, repo, commit_sha, tree_hash)
VALUES
    ('repo-1', 'tenant-1', 'github', 'org/app', 'abc123', 'sha256:tree-demo');

INSERT INTO code_issues
    (id, tenant_id, repo_id, title_fingerprint, body_fingerprint, status)
VALUES
    ('issue-1', 'tenant-1', 'repo-1', 'sha256:issue-title', 'sha256:issue-body', 'open');

INSERT INTO code_ci_signals
    (id, repo_id, commit_sha, status, failing_job, log_fingerprint)
VALUES
    ('ci-1', 'repo-1', 'abc123', 'failed', 'unit-tests', 'sha256:ci-log');

INSERT INTO code_risk_labels
    (id, issue_id, ci_signal_id, category, severity, confidence, replay_key)
VALUES
    ('risk-1', 'issue-1', 'ci-1', 'test_regression', 'high', 0.87, 'code:triage:issue-1');
