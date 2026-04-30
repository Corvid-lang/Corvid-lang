//! SQLite schema initialisation for the durable queue runtime —
//! slice 38 / persistent jobs surface, decomposed in Phase 20j-A2.
//!
//! Three private methods on [`DurableQueueRuntime`]:
//!
//! - `init` runs once on `open` / `open_in_memory`. Creates
//!   every table the queue needs (jobs, schedules, checkpoints,
//!   approvals, loop limits, stall policies, concurrency limits,
//!   audit events, etc.) idempotently — `CREATE TABLE IF NOT
//!   EXISTS` plus `ensure_column` for additive migrations.
//! - `ensure_column` is the additive-only schema migrator.
//!   PRAGMA table_info → ALTER TABLE ADD COLUMN if missing.
//!   Existing rows keep their data.
//! - `seed_next_id` rehydrates the in-memory `next_id` counter
//!   from `MAX(id)` after `open` so a restart picks up where
//!   the previous instance left off.

use std::sync::atomic::Ordering;

use super::DurableQueueRuntime;
use crate::errors::RuntimeError;

impl DurableQueueRuntime {
    pub(super) fn init(&self) -> Result<(), RuntimeError> {
        self.conn
            .lock()
            .unwrap()
            .execute_batch(
                "create table if not exists queue_jobs (
                    id text primary key,
                    task text not null,
                    payload_json text not null,
                    input_schema text,
                    status text not null,
                    attempts integer not null,
                    max_retries integer not null,
                    budget_usd real not null,
                    effect_summary text,
                    replay_key text,
                    idempotency_key text,
                    output_kind text,
                    output_fingerprint text,
                    failure_kind text,
                    failure_fingerprint text,
                    next_run_ms integer,
                    lease_owner text,
                    lease_expires_ms integer,
                    approval_id text,
                    approval_expires_ms integer,
                    approval_reason text,
                    created_ms integer not null,
                    updated_ms integer not null
                );
                create index if not exists queue_jobs_status on queue_jobs(status);
                create index if not exists queue_jobs_replay_key on queue_jobs(replay_key);
                /* Slice 38L: enforce that no two pending/active jobs share an
                 * idempotency_key. The partial-unique-index form (WHERE
                 * idempotency_key IS NOT NULL) is supported since SQLite
                 * 3.8.0 and lets jobs without an idempotency key coexist
                 * (the column is NULL by default for non-idempotent jobs).
                 * Combined with the existing on-collision fallback to
                 * get_by_idempotency_key in enqueue_typed_idempotent, the
                 * 4-concurrent-worker test from the Phase 38 audit
                 * correction provably collapses duplicates to one. */
                create unique index if not exists queue_jobs_idempotency_key
                    on queue_jobs(idempotency_key)
                    where idempotency_key is not null;
                create table if not exists queue_schedules (
                    id text primary key,
                    cron text not null,
                    zone text not null,
                    task text not null,
                    payload_json text not null,
                    max_retries integer not null,
                    budget_usd real not null,
                    effect_summary text,
                    replay_key_prefix text,
                    missed_policy text not null,
                    last_checked_ms integer not null,
                    last_fire_ms integer,
                    created_ms integer not null,
                    updated_ms integer not null
                );
                create index if not exists queue_schedules_task on queue_schedules(task);
                create table if not exists queue_schedule_fires (
                    event_id text primary key,
                    schedule_id text not null,
                    fire_ms integer not null,
                    job_id text not null,
                    created_ms integer not null
                );
                create unique index if not exists queue_schedule_fires_unique on queue_schedule_fires(schedule_id, fire_ms);
                create table if not exists queue_concurrency_limits (
                    scope text primary key,
                    max_leased integer not null,
                    updated_ms integer not null
                );
                create table if not exists queue_job_checkpoints (
                    id text primary key,
                    job_id text not null,
                    sequence integer not null,
                    kind text not null,
                    label text not null,
                    payload_json text not null,
                    payload_fingerprint text,
                    created_ms integer not null
                );
                create unique index if not exists queue_job_checkpoints_sequence on queue_job_checkpoints(job_id, sequence);
                create table if not exists queue_job_audit_events (
                    id text primary key,
                    job_id text not null,
                    event_kind text not null,
                    actor text not null,
                    approval_id text,
                    status_before text not null,
                    status_after text not null,
                    reason text,
                    created_ms integer not null
                );
                create index if not exists queue_job_audit_events_job on queue_job_audit_events(job_id, created_ms);
                create table if not exists queue_job_loop_limits (
                    job_id text primary key,
                    max_steps integer,
                    max_wall_ms integer,
                    max_spend_usd real,
                    max_tool_calls integer,
                    updated_ms integer not null
                );
                create table if not exists queue_job_loop_usage (
                    job_id text primary key,
                    steps integer not null,
                    wall_ms integer not null,
                    spend_usd real not null,
                    tool_calls integer not null,
                    updated_ms integer not null
                );
                create table if not exists queue_job_loop_heartbeats (
                    job_id text primary key,
                    actor text not null,
                    message text,
                    last_heartbeat_ms integer not null,
                    updated_ms integer not null
                );
                create table if not exists queue_job_stall_policies (
                    job_id text primary key,
                    stall_after_ms integer not null,
                    action text not null,
                    updated_ms integer not null
                );
                create table if not exists queue_controls (
                    name text primary key,
                    value text not null,
                    reason text,
                    updated_ms integer not null
                );",
            )
            .map_err(|err| {
                RuntimeError::Other(format!("failed to initialize durable queue: {err}"))
            })?;
        self.ensure_column("input_schema", "text")?;
        self.ensure_column("idempotency_key", "text")?;
        self.ensure_column("output_kind", "text")?;
        self.ensure_column("output_fingerprint", "text")?;
        self.ensure_column("failure_kind", "text")?;
        self.ensure_column("failure_fingerprint", "text")?;
        self.ensure_column("next_run_ms", "integer")?;
        self.ensure_column("lease_owner", "text")?;
        self.ensure_column("lease_expires_ms", "integer")?;
        self.ensure_column("approval_id", "text")?;
        self.ensure_column("approval_expires_ms", "integer")?;
        self.ensure_column("approval_reason", "text")?;
        self.conn
            .lock()
            .unwrap()
            .execute(
                "create unique index if not exists queue_jobs_idempotency_key on queue_jobs(idempotency_key) where idempotency_key is not null",
                [],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to initialize idempotency index: {err}")))?;
        Ok(())
    }

    pub(super) fn ensure_column(&self, name: &str, ty: &str) -> Result<(), RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("pragma table_info(queue_jobs)")
            .map_err(|err| {
                RuntimeError::Other(format!("failed to inspect durable queue schema: {err}"))
            })?;
        let columns = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|err| {
                RuntimeError::Other(format!("failed to inspect durable queue columns: {err}"))
            })?;
        for column in columns {
            if column.map_err(|err| {
                RuntimeError::Other(format!("failed to read durable queue column: {err}"))
            })? == name
            {
                return Ok(());
            }
        }
        conn.execute(
            &format!("alter table queue_jobs add column {name} {ty}"),
            [],
        )
        .map_err(|err| {
            RuntimeError::Other(format!(
                "failed to migrate durable queue column `{name}`: {err}"
            ))
        })?;
        Ok(())
    }

    pub(super) fn seed_next_id(&self) -> Result<(), RuntimeError> {
        let next = self
            .conn
            .lock()
            .unwrap()
            .query_row(
                "select coalesce(max(cast(substr(id, 5) as integer)), 0) from queue_jobs where id like 'job_%'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|err| RuntimeError::Other(format!("failed to seed durable queue ids: {err}")))?;
        self.next_id.store(next.max(0) as u64, Ordering::Relaxed);
        Ok(())
    }
}
