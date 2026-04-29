INSERT INTO support_tickets
    (id, tenant_id, customer_fingerprint, subject_fingerprint, priority, status)
VALUES
    ('ticket-1', 'tenant-1', 'sha256:customer-demo', 'sha256:subject-demo', 'high', 'open');

INSERT INTO support_policy_citations
    (id, policy_id, title_fingerprint, section, provenance_id)
VALUES
    ('citation-1', 'policy-refund-1', 'sha256:refund-policy', 'refund.window', 'policy:refund:window');

INSERT INTO support_draft_replies
    (id, ticket_id, body_fingerprint, policy_citation_id, status, approval_label)
VALUES
    ('draft-1', 'ticket-1', 'sha256:support-draft', 'citation-1', 'drafted', 'SendSupportReply');
