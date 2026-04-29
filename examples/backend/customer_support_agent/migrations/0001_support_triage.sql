CREATE TABLE support_tickets (
    id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    customer_fingerprint TEXT NOT NULL,
    subject_fingerprint TEXT NOT NULL,
    priority TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE support_policy_citations (
    id TEXT PRIMARY KEY,
    policy_id TEXT NOT NULL,
    title_fingerprint TEXT NOT NULL,
    section TEXT NOT NULL,
    provenance_id TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE support_draft_replies (
    id TEXT PRIMARY KEY,
    ticket_id TEXT NOT NULL REFERENCES support_tickets(id) ON DELETE CASCADE,
    body_fingerprint TEXT NOT NULL,
    policy_citation_id TEXT NOT NULL REFERENCES support_policy_citations(id),
    status TEXT NOT NULL,
    approval_label TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
