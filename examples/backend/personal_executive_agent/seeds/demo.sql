INSERT INTO executive_tenants (id, display_name, plan, region, data_retention_days)
VALUES ('tenant-1', 'Executive Demo', 'team', 'us', 90);

INSERT INTO executive_users (id, tenant_id, email, timezone, role, preferences_fingerprint)
VALUES ('user-1', 'tenant-1', 'user@example.com', 'America/New_York', 'owner', 'sha256:preferences-demo');

INSERT INTO executive_connector_accounts
    (id, tenant_id, provider, scope, mode, approval_required, replay_policy, token_state_ref)
VALUES
    ('conn-email', 'tenant-1', 'gmail', 'mail.read mail.draft mail.send', 'mock', 1, 'quarantine_writes', 'mock:gmail'),
    ('conn-calendar', 'tenant-1', 'calendar', 'calendar.read calendar.write', 'mock', 1, 'quarantine_writes', 'mock:calendar'),
    ('conn-tasks', 'tenant-1', 'tasks', 'tasks.read tasks.write', 'mock', 1, 'quarantine_writes', 'mock:tasks'),
    ('conn-chat', 'tenant-1', 'slack', 'chat.read chat.write', 'mock', 1, 'quarantine_writes', 'mock:slack'),
    ('conn-files', 'tenant-1', 'files', 'files.read files.write', 'mock', 1, 'quarantine_writes', 'mock:files');

INSERT INTO executive_inbox_threads
    (id, tenant_id, user_id, provider_thread_id, sender, subject_fingerprint, priority, triage_status, replay_key)
VALUES
    ('thread-1', 'tenant-1', 'user-1', 'gmail-thread-1', 'partner@example.com', 'sha256:subject-demo', 'high', 'needs_draft', 'replay:thread-1');

INSERT INTO executive_draft_replies
    (id, thread_id, tenant_id, body_fingerprint, approval_label, status, replay_key)
VALUES
    ('draft-1', 'thread-1', 'tenant-1', 'sha256:draft-demo', 'SendFollowUpEmail', 'approval_pending', 'replay:draft-1');

INSERT INTO executive_calendar_events
    (id, tenant_id, user_id, provider_event_id, starts_at, ends_at, attendee_fingerprint, status, replay_key)
VALUES
    ('event-1', 'tenant-1', 'user-1', 'calendar-event-1', '2026-04-29T14:00:00Z', '2026-04-29T14:30:00Z', 'sha256:attendees-demo', 'confirmed', 'replay:event-1');

INSERT INTO executive_tasks
    (id, tenant_id, user_id, source_kind, title_fingerprint, external_ref, status, replay_key)
VALUES
    ('task-1', 'tenant-1', 'user-1', 'email', 'sha256:task-title-demo', 'linear:PEA-1', 'open', 'replay:task-1');
