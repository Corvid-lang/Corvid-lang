INSERT INTO app_tenants (id, name, created_ms)
VALUES ('tenant-demo', 'Demo Tenant', 1700000000000);

INSERT INTO app_users (id, tenant_id, email, role)
VALUES ('user-demo', 'tenant-demo', 'demo@example.com', 'owner');

INSERT INTO app_jobs (id, tenant_id, kind, status, replay_key)
VALUES ('job-demo-daily', 'tenant-demo', 'daily_brief', 'pending', 'template:daily:demo');
