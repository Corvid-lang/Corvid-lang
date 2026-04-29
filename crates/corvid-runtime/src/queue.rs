use crate::errors::RuntimeError;
use crate::tracing::now_ms;
use chrono::{TimeZone, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueJobStatus {
    Pending,
    Leased,
    ApprovalWait,
    ApprovalDenied,
    ApprovalExpired,
    LoopBudgetExceeded,
    RetryWait,
    Running,
    Succeeded,
    Failed,
    DeadLettered,
    Canceled,
}

impl QueueJobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Leased => "leased",
            Self::ApprovalWait => "approval_wait",
            Self::ApprovalDenied => "approval_denied",
            Self::ApprovalExpired => "approval_expired",
            Self::LoopBudgetExceeded => "loop_budget_exceeded",
            Self::RetryWait => "retry_wait",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::DeadLettered => "dead_lettered",
            Self::Canceled => "canceled",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct QueueJob {
    pub id: String,
    pub task: String,
    pub payload: Value,
    pub input_schema: Option<String>,
    pub status: QueueJobStatus,
    pub attempts: u64,
    pub max_retries: u64,
    pub budget_usd: f64,
    pub effect_summary: Option<String>,
    pub replay_key: Option<String>,
    pub idempotency_key: Option<String>,
    pub output_kind: Option<String>,
    pub output_fingerprint: Option<String>,
    pub failure_kind: Option<String>,
    pub failure_fingerprint: Option<String>,
    pub next_run_ms: Option<u64>,
    pub lease_owner: Option<String>,
    pub lease_expires_ms: Option<u64>,
    pub approval_id: Option<String>,
    pub approval_expires_ms: Option<u64>,
    pub approval_reason: Option<String>,
    pub created_ms: u64,
    pub updated_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleMissedPolicy {
    SkipMissed,
    FireOnceOnRecovery,
    EnqueueAllBounded,
}

impl ScheduleMissedPolicy {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SkipMissed => "skip_missed",
            Self::FireOnceOnRecovery => "fire_once_on_recovery",
            Self::EnqueueAllBounded => "enqueue_all_bounded",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct QueueScheduleManifest {
    pub id: String,
    pub cron: String,
    pub zone: String,
    pub task: String,
    pub payload: Value,
    pub max_retries: u64,
    pub budget_usd: f64,
    pub effect_summary: Option<String>,
    pub replay_key_prefix: Option<String>,
    pub missed_policy: ScheduleMissedPolicy,
    pub last_checked_ms: u64,
    pub last_fire_ms: Option<u64>,
    pub created_ms: u64,
    pub updated_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleRecovery {
    pub schedule_id: String,
    pub task: String,
    pub fire_ms: u64,
    pub job_id: Option<String>,
    pub policy: ScheduleMissedPolicy,
    pub action: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchedulerRecoveryReport {
    pub scanned: usize,
    pub enqueued: usize,
    pub skipped: usize,
    pub recoveries: Vec<ScheduleRecovery>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueConcurrencyLimit {
    pub scope: String,
    pub limit: u64,
    pub updated_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobCheckpointKind {
    AgentStep,
    ToolResult,
    PartialOutput,
}

impl JobCheckpointKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AgentStep => "agent_step",
            Self::ToolResult => "tool_result",
            Self::PartialOutput => "partial_output",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobCheckpoint {
    pub id: String,
    pub job_id: String,
    pub sequence: u64,
    pub kind: JobCheckpointKind,
    pub label: String,
    pub payload: Value,
    pub payload_fingerprint: Option<String>,
    pub created_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobResumeState {
    pub job: QueueJob,
    pub checkpoints: Vec<JobCheckpoint>,
    pub last_checkpoint: Option<JobCheckpoint>,
    pub next_sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobApprovalDecision {
    Approve,
    Deny,
    Expire,
}

impl JobApprovalDecision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Deny => "deny",
            Self::Expire => "expire",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobAuditEvent {
    pub id: String,
    pub job_id: String,
    pub event_kind: String,
    pub actor: String,
    pub approval_id: Option<String>,
    pub status_before: String,
    pub status_after: String,
    pub reason: Option<String>,
    pub created_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobLoopLimits {
    pub job_id: String,
    pub max_steps: Option<u64>,
    pub max_wall_ms: Option<u64>,
    pub max_spend_usd: Option<f64>,
    pub max_tool_calls: Option<u64>,
    pub updated_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobLoopUsage {
    pub job_id: String,
    pub steps: u64,
    pub wall_ms: u64,
    pub spend_usd: f64,
    pub tool_calls: u64,
    pub updated_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobLoopUsageReport {
    pub usage: JobLoopUsage,
    pub violated_bounds: Vec<String>,
}

#[derive(Clone, Default)]
pub struct QueueRuntime {
    next_id: Arc<AtomicU64>,
    jobs: Arc<Mutex<BTreeMap<String, QueueJob>>>,
}

impl QueueRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue(
        &self,
        task: impl Into<String>,
        payload: Value,
        max_retries: u64,
        budget_usd: f64,
        effect_summary: Option<String>,
        replay_key: Option<String>,
    ) -> Result<QueueJob, RuntimeError> {
        self.enqueue_typed(
            task,
            payload,
            None,
            max_retries,
            budget_usd,
            effect_summary,
            replay_key,
        )
    }

    pub fn enqueue_typed(
        &self,
        task: impl Into<String>,
        payload: Value,
        input_schema: Option<String>,
        max_retries: u64,
        budget_usd: f64,
        effect_summary: Option<String>,
        replay_key: Option<String>,
    ) -> Result<QueueJob, RuntimeError> {
        self.enqueue_typed_at(
            task,
            payload,
            input_schema,
            max_retries,
            budget_usd,
            effect_summary,
            replay_key,
            None,
        )
    }

    pub fn enqueue_typed_at(
        &self,
        task: impl Into<String>,
        payload: Value,
        input_schema: Option<String>,
        max_retries: u64,
        budget_usd: f64,
        effect_summary: Option<String>,
        replay_key: Option<String>,
        next_run_ms: Option<u64>,
    ) -> Result<QueueJob, RuntimeError> {
        let task = task.into();
        if task.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue task name must not be empty".to_string(),
            ));
        }
        let now = now_ms();
        let id = format!(
            "job_{}",
            self.next_id
                .fetch_add(1, Ordering::Relaxed)
                .saturating_add(1)
        );
        let job = QueueJob {
            id: id.clone(),
            task,
            payload,
            input_schema,
            status: QueueJobStatus::Pending,
            attempts: 0,
            max_retries,
            budget_usd: if budget_usd.is_finite() && budget_usd > 0.0 {
                budget_usd
            } else {
                0.0
            },
            effect_summary,
            replay_key,
            idempotency_key: None,
            output_kind: None,
            output_fingerprint: None,
            failure_kind: None,
            failure_fingerprint: None,
            next_run_ms,
            lease_owner: None,
            lease_expires_ms: None,
            approval_id: None,
            approval_expires_ms: None,
            approval_reason: None,
            created_ms: now,
            updated_ms: now,
        };
        self.jobs.lock().unwrap().insert(id, job.clone());
        Ok(job)
    }

    pub fn get(&self, id: &str) -> Option<QueueJob> {
        self.jobs.lock().unwrap().get(id).cloned()
    }

    pub fn cancel(&self, id: &str) -> Result<QueueJob, RuntimeError> {
        let mut jobs = self.jobs.lock().unwrap();
        let job = jobs
            .get_mut(id)
            .ok_or_else(|| RuntimeError::Other(format!("std.queue job `{id}` not found")))?;
        job.status = QueueJobStatus::Canceled;
        job.updated_ms = now_ms();
        Ok(job.clone())
    }
}

pub struct DurableQueueRuntime {
    next_id: AtomicU64,
    conn: Mutex<Connection>,
}

impl DurableQueueRuntime {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, RuntimeError> {
        let conn = Connection::open(path.as_ref()).map_err(|err| {
            RuntimeError::Other(format!(
                "failed to open durable queue `{}`: {err}",
                path.as_ref().display()
            ))
        })?;
        let runtime = Self {
            next_id: AtomicU64::new(0),
            conn: Mutex::new(conn),
        };
        runtime.init()?;
        runtime.seed_next_id()?;
        Ok(runtime)
    }

    pub fn open_in_memory() -> Result<Self, RuntimeError> {
        let conn = Connection::open_in_memory()
            .map_err(|err| RuntimeError::Other(format!("failed to open durable queue: {err}")))?;
        let runtime = Self {
            next_id: AtomicU64::new(0),
            conn: Mutex::new(conn),
        };
        runtime.init()?;
        Ok(runtime)
    }

    pub fn enqueue(
        &self,
        task: impl Into<String>,
        payload: Value,
        max_retries: u64,
        budget_usd: f64,
        effect_summary: Option<String>,
        replay_key: Option<String>,
    ) -> Result<QueueJob, RuntimeError> {
        self.enqueue_typed(
            task,
            payload,
            None,
            max_retries,
            budget_usd,
            effect_summary,
            replay_key,
        )
    }

    pub fn enqueue_typed(
        &self,
        task: impl Into<String>,
        payload: Value,
        input_schema: Option<String>,
        max_retries: u64,
        budget_usd: f64,
        effect_summary: Option<String>,
        replay_key: Option<String>,
    ) -> Result<QueueJob, RuntimeError> {
        self.enqueue_typed_at(
            task,
            payload,
            input_schema,
            max_retries,
            budget_usd,
            effect_summary,
            replay_key,
            None,
        )
    }

    pub fn enqueue_typed_at(
        &self,
        task: impl Into<String>,
        payload: Value,
        input_schema: Option<String>,
        max_retries: u64,
        budget_usd: f64,
        effect_summary: Option<String>,
        replay_key: Option<String>,
        next_run_ms: Option<u64>,
    ) -> Result<QueueJob, RuntimeError> {
        let task = task.into();
        if task.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue task name must not be empty".to_string(),
            ));
        }
        let now = now_ms();
        let id = format!(
            "job_{}",
            self.next_id
                .fetch_add(1, Ordering::Relaxed)
                .saturating_add(1)
        );
        let budget_usd = if budget_usd.is_finite() && budget_usd > 0.0 {
            budget_usd
        } else {
            0.0
        };
        let job = QueueJob {
            id: id.clone(),
            task,
            payload,
            input_schema,
            status: QueueJobStatus::Pending,
            attempts: 0,
            max_retries,
            budget_usd,
            effect_summary,
            replay_key,
            idempotency_key: None,
            output_kind: None,
            output_fingerprint: None,
            failure_kind: None,
            failure_fingerprint: None,
            next_run_ms,
            lease_owner: None,
            lease_expires_ms: None,
            approval_id: None,
            approval_expires_ms: None,
            approval_reason: None,
            created_ms: now,
            updated_ms: now,
        };
        let payload_json = serde_json::to_string(&job.payload).map_err(|err| {
            RuntimeError::Other(format!("failed to serialize durable queue payload: {err}"))
        })?;
        self.conn
            .lock()
            .unwrap()
            .execute(
                "insert into queue_jobs
                 (id, task, payload_json, input_schema, status, attempts, max_retries, budget_usd, effect_summary, replay_key, idempotency_key, output_kind, output_fingerprint, failure_kind, failure_fingerprint, next_run_ms, lease_owner, lease_expires_ms, approval_id, approval_expires_ms, approval_reason, created_ms, updated_ms)
                 values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
                params![
                    job.id,
                    job.task,
                    payload_json,
                    job.input_schema,
                    job.status.as_str(),
                    job.attempts as i64,
                    job.max_retries as i64,
                    job.budget_usd,
                    job.effect_summary,
                    job.replay_key,
                    job.idempotency_key,
                    job.output_kind,
                    job.output_fingerprint,
                    job.failure_kind,
                    job.failure_fingerprint,
                    job.next_run_ms.map(|value| value as i64),
                    job.lease_owner,
                    job.lease_expires_ms.map(|value| value as i64),
                    job.approval_id,
                    job.approval_expires_ms.map(|value| value as i64),
                    job.approval_reason,
                    job.created_ms as i64,
                    job.updated_ms as i64,
                ],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to insert durable queue job: {err}")))?;
        Ok(job)
    }

    pub fn enqueue_typed_idempotent(
        &self,
        task: impl Into<String>,
        payload: Value,
        input_schema: Option<String>,
        max_retries: u64,
        budget_usd: f64,
        effect_summary: Option<String>,
        replay_key: Option<String>,
        idempotency_key: Option<String>,
        next_run_ms: Option<u64>,
    ) -> Result<QueueJob, RuntimeError> {
        let Some(idempotency_key) = idempotency_key.filter(|key| !key.trim().is_empty()) else {
            return self.enqueue_typed_at(
                task,
                payload,
                input_schema,
                max_retries,
                budget_usd,
                effect_summary,
                replay_key,
                next_run_ms,
            );
        };
        if let Some(existing) = self.get_by_idempotency_key(&idempotency_key)? {
            return Ok(existing);
        }
        let task = task.into();
        if task.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue task name must not be empty".to_string(),
            ));
        }
        let now = now_ms();
        let id = format!(
            "job_{}",
            self.next_id
                .fetch_add(1, Ordering::Relaxed)
                .saturating_add(1)
        );
        let budget_usd = if budget_usd.is_finite() && budget_usd > 0.0 {
            budget_usd
        } else {
            0.0
        };
        let job = QueueJob {
            id: id.clone(),
            task,
            payload,
            input_schema,
            status: QueueJobStatus::Pending,
            attempts: 0,
            max_retries,
            budget_usd,
            effect_summary,
            replay_key,
            idempotency_key: Some(idempotency_key.clone()),
            output_kind: None,
            output_fingerprint: None,
            failure_kind: None,
            failure_fingerprint: None,
            next_run_ms,
            lease_owner: None,
            lease_expires_ms: None,
            approval_id: None,
            approval_expires_ms: None,
            approval_reason: None,
            created_ms: now,
            updated_ms: now,
        };
        let payload_json = serde_json::to_string(&job.payload).map_err(|err| {
            RuntimeError::Other(format!("failed to serialize durable queue payload: {err}"))
        })?;
        let inserted = self
            .conn
            .lock()
            .unwrap()
            .execute(
                "insert into queue_jobs
                 (id, task, payload_json, input_schema, status, attempts, max_retries, budget_usd, effect_summary, replay_key, idempotency_key, output_kind, output_fingerprint, failure_kind, failure_fingerprint, next_run_ms, lease_owner, lease_expires_ms, approval_id, approval_expires_ms, approval_reason, created_ms, updated_ms)
                 values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
                params![
                    job.id,
                    job.task,
                    payload_json,
                    job.input_schema,
                    job.status.as_str(),
                    job.attempts as i64,
                    job.max_retries as i64,
                    job.budget_usd,
                    job.effect_summary,
                    job.replay_key,
                    job.idempotency_key,
                    job.output_kind,
                    job.output_fingerprint,
                    job.failure_kind,
                    job.failure_fingerprint,
                    job.next_run_ms.map(|value| value as i64),
                    job.lease_owner,
                    job.lease_expires_ms.map(|value| value as i64),
                    job.approval_id,
                    job.approval_expires_ms.map(|value| value as i64),
                    job.approval_reason,
                    job.created_ms as i64,
                    job.updated_ms as i64,
                ],
            );
        match inserted {
            Ok(_) => Ok(job),
            Err(err) => {
                if let Some(existing) = self.get_by_idempotency_key(&idempotency_key)? {
                    Ok(existing)
                } else {
                    Err(RuntimeError::Other(format!(
                        "failed to insert idempotent durable queue job: {err}"
                    )))
                }
            }
        }
    }

    pub fn get(&self, id: &str) -> Result<Option<QueueJob>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select id, task, payload_json, input_schema, status, attempts, max_retries, budget_usd,
                        effect_summary, replay_key, idempotency_key, output_kind, output_fingerprint, failure_kind, failure_fingerprint, next_run_ms, lease_owner, lease_expires_ms, approval_id, approval_expires_ms, approval_reason, created_ms, updated_ms
                 from queue_jobs where id = ?1",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to prepare durable queue read: {err}")))?;
        let mut rows = stmt.query(params![id]).map_err(|err| {
            RuntimeError::Other(format!("failed to query durable queue job: {err}"))
        })?;
        if let Some(row) = rows.next().map_err(|err| {
            RuntimeError::Other(format!("failed to read durable queue row: {err}"))
        })? {
            Ok(Some(read_job_row(row).map_err(|err| {
                RuntimeError::Other(format!("failed to decode durable queue job: {err}"))
            })?))
        } else {
            Ok(None)
        }
    }

    pub fn get_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<QueueJob>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select id, task, payload_json, input_schema, status, attempts, max_retries, budget_usd,
                        effect_summary, replay_key, idempotency_key, output_kind, output_fingerprint, failure_kind, failure_fingerprint, next_run_ms, lease_owner, lease_expires_ms, approval_id, approval_expires_ms, approval_reason, created_ms, updated_ms
                 from queue_jobs where idempotency_key = ?1",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to prepare idempotency read: {err}")))?;
        let mut rows = stmt.query(params![idempotency_key]).map_err(|err| {
            RuntimeError::Other(format!("failed to query idempotency key: {err}"))
        })?;
        if let Some(row) = rows
            .next()
            .map_err(|err| RuntimeError::Other(format!("failed to read idempotency row: {err}")))?
        {
            Ok(Some(read_job_row(row).map_err(|err| {
                RuntimeError::Other(format!("failed to decode idempotent job: {err}"))
            })?))
        } else {
            Ok(None)
        }
    }

    pub fn list(&self) -> Result<Vec<QueueJob>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select id, task, payload_json, input_schema, status, attempts, max_retries, budget_usd,
                        effect_summary, replay_key, idempotency_key, output_kind, output_fingerprint, failure_kind, failure_fingerprint, next_run_ms, lease_owner, lease_expires_ms, approval_id, approval_expires_ms, approval_reason, created_ms, updated_ms
                 from queue_jobs order by created_ms, id",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to prepare durable queue list: {err}")))?;
        let rows = stmt.query_map([], read_job_row).map_err(|err| {
            RuntimeError::Other(format!("failed to list durable queue jobs: {err}"))
        })?;
        let mut jobs = Vec::new();
        for row in rows {
            jobs.push(row.map_err(|err| {
                RuntimeError::Other(format!("failed to decode durable queue job: {err}"))
            })?);
        }
        Ok(jobs)
    }

    pub fn dead_lettered(&self) -> Result<Vec<QueueJob>, RuntimeError> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|job| job.status == QueueJobStatus::DeadLettered)
            .collect())
    }

    pub fn approval_waiting(&self) -> Result<Vec<QueueJob>, RuntimeError> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|job| job.status == QueueJobStatus::ApprovalWait)
            .collect())
    }

    pub fn job_audit_events(&self, job_id: &str) -> Result<Vec<JobAuditEvent>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select id, job_id, event_kind, actor, approval_id, status_before, status_after, reason, created_ms
                 from queue_job_audit_events
                 where job_id = ?1
                 order by created_ms, id",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to prepare job audit read: {err}")))?;
        let rows = stmt
            .query_map(params![job_id], read_job_audit_row)
            .map_err(|err| {
                RuntimeError::Other(format!("failed to list job audit events: {err}"))
            })?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row.map_err(|err| {
                RuntimeError::Other(format!("failed to decode job audit event: {err}"))
            })?);
        }
        Ok(events)
    }

    pub fn set_loop_limits(
        &self,
        job_id: &str,
        max_steps: Option<u64>,
        max_wall_ms: Option<u64>,
        max_spend_usd: Option<f64>,
        max_tool_calls: Option<u64>,
    ) -> Result<JobLoopLimits, RuntimeError> {
        if self.get(job_id)?.is_none() {
            return Err(RuntimeError::Other(format!("std.queue job `{job_id}` not found")));
        }
        let max_spend_usd = max_spend_usd.filter(|value| value.is_finite() && *value >= 0.0);
        let now = now_ms();
        self.conn
            .lock()
            .unwrap()
            .execute(
                "insert into queue_job_loop_limits
                 (job_id, max_steps, max_wall_ms, max_spend_usd, max_tool_calls, updated_ms)
                 values (?1, ?2, ?3, ?4, ?5, ?6)
                 on conflict(job_id) do update set
                    max_steps = excluded.max_steps,
                    max_wall_ms = excluded.max_wall_ms,
                    max_spend_usd = excluded.max_spend_usd,
                    max_tool_calls = excluded.max_tool_calls,
                    updated_ms = excluded.updated_ms",
                params![
                    job_id,
                    max_steps.map(|value| value as i64),
                    max_wall_ms.map(|value| value as i64),
                    max_spend_usd,
                    max_tool_calls.map(|value| value as i64),
                    now as i64,
                ],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to set loop limits: {err}")))?;
        self.loop_limits(job_id)?
            .ok_or_else(|| RuntimeError::Other(format!("std.queue loop limits for `{job_id}` not found")))
    }

    pub fn loop_limits(&self, job_id: &str) -> Result<Option<JobLoopLimits>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select job_id, max_steps, max_wall_ms, max_spend_usd, max_tool_calls, updated_ms
                 from queue_job_loop_limits where job_id = ?1",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to prepare loop limit read: {err}")))?;
        let mut rows = stmt
            .query(params![job_id])
            .map_err(|err| RuntimeError::Other(format!("failed to query loop limits: {err}")))?;
        if let Some(row) = rows
            .next()
            .map_err(|err| RuntimeError::Other(format!("failed to read loop limit row: {err}")))?
        {
            Ok(Some(read_loop_limits_row(row).map_err(|err| {
                RuntimeError::Other(format!("failed to decode loop limits: {err}"))
            })?))
        } else {
            Ok(None)
        }
    }

    pub fn loop_usage(&self, job_id: &str) -> Result<Option<JobLoopUsage>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select job_id, steps, wall_ms, spend_usd, tool_calls, updated_ms
                 from queue_job_loop_usage where job_id = ?1",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to prepare loop usage read: {err}")))?;
        let mut rows = stmt
            .query(params![job_id])
            .map_err(|err| RuntimeError::Other(format!("failed to query loop usage: {err}")))?;
        if let Some(row) = rows
            .next()
            .map_err(|err| RuntimeError::Other(format!("failed to read loop usage row: {err}")))?
        {
            Ok(Some(read_loop_usage_row(row).map_err(|err| {
                RuntimeError::Other(format!("failed to decode loop usage: {err}"))
            })?))
        } else {
            Ok(None)
        }
    }

    pub fn record_loop_usage(
        &self,
        job_id: &str,
        step_delta: u64,
        wall_ms_delta: u64,
        spend_usd_delta: f64,
        tool_call_delta: u64,
        actor: impl Into<String>,
    ) -> Result<JobLoopUsageReport, RuntimeError> {
        self.record_loop_usage_at(
            job_id,
            step_delta,
            wall_ms_delta,
            spend_usd_delta,
            tool_call_delta,
            actor,
            now_ms(),
        )
    }

    pub fn record_loop_usage_at(
        &self,
        job_id: &str,
        step_delta: u64,
        wall_ms_delta: u64,
        spend_usd_delta: f64,
        tool_call_delta: u64,
        actor: impl Into<String>,
        now: u64,
    ) -> Result<JobLoopUsageReport, RuntimeError> {
        let actor = actor.into();
        if actor.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue loop usage actor must not be empty".to_string(),
            ));
        }
        let spend_usd_delta = if spend_usd_delta.is_finite() && spend_usd_delta > 0.0 {
            spend_usd_delta
        } else {
            0.0
        };
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .map_err(|err| RuntimeError::Other(format!("failed to start loop usage transaction: {err}")))?;
        let status_before = tx
            .query_row(
                "select status from queue_jobs where id = ?1",
                params![job_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(|err| RuntimeError::Other(format!("std.queue job `{job_id}` not found or unreadable: {err}")))?;
        if matches!(
            parse_status(&status_before),
            QueueJobStatus::Succeeded
                | QueueJobStatus::DeadLettered
                | QueueJobStatus::Canceled
                | QueueJobStatus::ApprovalDenied
                | QueueJobStatus::ApprovalExpired
                | QueueJobStatus::LoopBudgetExceeded
        ) {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{job_id}` is terminal and cannot record loop usage"
            )));
        }
        tx.execute(
            "insert into queue_job_loop_usage
             (job_id, steps, wall_ms, spend_usd, tool_calls, updated_ms)
             values (?1, ?2, ?3, ?4, ?5, ?6)
             on conflict(job_id) do update set
                steps = steps + excluded.steps,
                wall_ms = wall_ms + excluded.wall_ms,
                spend_usd = spend_usd + excluded.spend_usd,
                tool_calls = tool_calls + excluded.tool_calls,
                updated_ms = excluded.updated_ms",
            params![
                job_id,
                step_delta as i64,
                wall_ms_delta as i64,
                spend_usd_delta,
                tool_call_delta as i64,
                now as i64,
            ],
        )
        .map_err(|err| RuntimeError::Other(format!("failed to record loop usage: {err}")))?;
        let usage = tx
            .query_row(
                "select job_id, steps, wall_ms, spend_usd, tool_calls, updated_ms
                 from queue_job_loop_usage where job_id = ?1",
                params![job_id],
                read_loop_usage_row,
            )
            .map_err(|err| RuntimeError::Other(format!("failed to read loop usage after update: {err}")))?;
        let limits = tx
            .query_row(
                "select job_id, max_steps, max_wall_ms, max_spend_usd, max_tool_calls, updated_ms
                 from queue_job_loop_limits where job_id = ?1",
                params![job_id],
                read_loop_limits_row,
            )
            .optional()
            .map_err(|err| RuntimeError::Other(format!("failed to read loop limits after update: {err}")))?;
        let violated_bounds = limits
            .as_ref()
            .map(|limits| loop_bound_violations(&usage, limits))
            .unwrap_or_default();
        if !violated_bounds.is_empty() {
            let reason = violated_bounds.join(",");
            tx.execute(
                "update queue_jobs
                 set status = 'loop_budget_exceeded',
                     failure_kind = 'loop_bound_exceeded',
                     failure_fingerprint = ?2,
                     next_run_ms = null,
                     lease_owner = null,
                     lease_expires_ms = null,
                     updated_ms = ?3
                 where id = ?1",
                params![job_id, reason, now as i64],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to stop loop-bound job: {err}")))?;
            insert_job_audit_event(
                &tx,
                job_id,
                "loop_bound_exceeded",
                &actor,
                None,
                &status_before,
                QueueJobStatus::LoopBudgetExceeded.as_str(),
                Some(&reason),
                now,
            )?;
        }
        tx.commit()
            .map_err(|err| RuntimeError::Other(format!("failed to commit loop usage: {err}")))?;
        Ok(JobLoopUsageReport {
            usage,
            violated_bounds,
        })
    }

    pub fn upsert_schedule(
        &self,
        manifest: QueueScheduleManifest,
    ) -> Result<QueueScheduleManifest, RuntimeError> {
        validate_schedule(&manifest.cron, &manifest.zone)?;
        if manifest.id.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue schedule id must not be empty".to_string(),
            ));
        }
        if manifest.task.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue schedule task must not be empty".to_string(),
            ));
        }
        let now = now_ms();
        let payload_json = serde_json::to_string(&manifest.payload).map_err(|err| {
            RuntimeError::Other(format!(
                "failed to serialize durable schedule payload: {err}"
            ))
        })?;
        let budget_usd = if manifest.budget_usd.is_finite() && manifest.budget_usd > 0.0 {
            manifest.budget_usd
        } else {
            0.0
        };
        let created_ms = if manifest.created_ms == 0 {
            now
        } else {
            manifest.created_ms
        };
        let last_checked_ms = if manifest.last_checked_ms == 0 {
            now
        } else {
            manifest.last_checked_ms
        };
        self.conn
            .lock()
            .unwrap()
            .execute(
                "insert into queue_schedules
                 (id, cron, zone, task, payload_json, max_retries, budget_usd, effect_summary, replay_key_prefix, missed_policy, last_checked_ms, last_fire_ms, created_ms, updated_ms)
                 values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                 on conflict(id) do update set
                   cron = excluded.cron,
                   zone = excluded.zone,
                   task = excluded.task,
                   payload_json = excluded.payload_json,
                   max_retries = excluded.max_retries,
                   budget_usd = excluded.budget_usd,
                   effect_summary = excluded.effect_summary,
                   replay_key_prefix = excluded.replay_key_prefix,
                   missed_policy = excluded.missed_policy,
                   updated_ms = excluded.updated_ms",
                params![
                    manifest.id,
                    manifest.cron,
                    manifest.zone,
                    manifest.task,
                    payload_json,
                    manifest.max_retries as i64,
                    budget_usd,
                    manifest.effect_summary,
                    manifest.replay_key_prefix,
                    manifest.missed_policy.as_str(),
                    last_checked_ms as i64,
                    manifest.last_fire_ms.map(|value| value as i64),
                    created_ms as i64,
                    now as i64,
                ],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to upsert schedule: {err}")))?;
        self.get_schedule(&manifest.id)?.ok_or_else(|| {
            RuntimeError::Other(format!("schedule `{}` not found after upsert", manifest.id))
        })
    }

    pub fn get_schedule(&self, id: &str) -> Result<Option<QueueScheduleManifest>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select id, cron, zone, task, payload_json, max_retries, budget_usd, effect_summary, replay_key_prefix, missed_policy, last_checked_ms, last_fire_ms, created_ms, updated_ms
                 from queue_schedules where id = ?1",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to prepare schedule read: {err}")))?;
        let mut rows = stmt
            .query(params![id])
            .map_err(|err| RuntimeError::Other(format!("failed to query schedule: {err}")))?;
        if let Some(row) = rows
            .next()
            .map_err(|err| RuntimeError::Other(format!("failed to read schedule row: {err}")))?
        {
            Ok(Some(read_schedule_row(row).map_err(|err| {
                RuntimeError::Other(format!("failed to decode schedule: {err}"))
            })?))
        } else {
            Ok(None)
        }
    }

    pub fn list_schedules(&self) -> Result<Vec<QueueScheduleManifest>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select id, cron, zone, task, payload_json, max_retries, budget_usd, effect_summary, replay_key_prefix, missed_policy, last_checked_ms, last_fire_ms, created_ms, updated_ms
                 from queue_schedules order by id",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to prepare schedule list: {err}")))?;
        let rows = stmt
            .query_map([], read_schedule_row)
            .map_err(|err| RuntimeError::Other(format!("failed to list schedules: {err}")))?;
        let mut schedules = Vec::new();
        for row in rows {
            schedules.push(
                row.map_err(|err| {
                    RuntimeError::Other(format!("failed to decode schedule: {err}"))
                })?,
            );
        }
        Ok(schedules)
    }

    pub fn recover_schedules(
        &self,
        max_missed_per_schedule: usize,
    ) -> Result<SchedulerRecoveryReport, RuntimeError> {
        self.recover_schedules_at(now_ms(), max_missed_per_schedule)
    }

    pub fn recover_schedules_at(
        &self,
        now: u64,
        max_missed_per_schedule: usize,
    ) -> Result<SchedulerRecoveryReport, RuntimeError> {
        let schedules = self.list_schedules()?;
        let mut report = SchedulerRecoveryReport {
            scanned: schedules.len(),
            ..SchedulerRecoveryReport::default()
        };
        for schedule in schedules {
            let due = missed_fire_times(&schedule, now, max_missed_per_schedule)?;
            if due.is_empty() {
                self.mark_schedule_checked(&schedule.id, now, schedule.last_fire_ms)?;
                continue;
            }
            match schedule.missed_policy {
                ScheduleMissedPolicy::SkipMissed => {
                    report.skipped = report.skipped.saturating_add(due.len());
                    for fire_ms in &due {
                        report.recoveries.push(ScheduleRecovery {
                            schedule_id: schedule.id.clone(),
                            task: schedule.task.clone(),
                            fire_ms: *fire_ms,
                            job_id: None,
                            policy: schedule.missed_policy,
                            action: "skipped".to_string(),
                        });
                    }
                    self.mark_schedule_checked(&schedule.id, now, due.last().copied())?;
                }
                ScheduleMissedPolicy::FireOnceOnRecovery => {
                    let Some(&fire_ms) = due.last() else { continue };
                    if let Some(job) = self.enqueue_schedule_fire(&schedule, fire_ms)? {
                        report.enqueued = report.enqueued.saturating_add(1);
                        report.recoveries.push(ScheduleRecovery {
                            schedule_id: schedule.id.clone(),
                            task: schedule.task.clone(),
                            fire_ms,
                            job_id: Some(job.id),
                            policy: schedule.missed_policy,
                            action: "enqueued".to_string(),
                        });
                    }
                    self.mark_schedule_checked(&schedule.id, now, Some(fire_ms))?;
                }
                ScheduleMissedPolicy::EnqueueAllBounded => {
                    let mut last_fire = schedule.last_fire_ms;
                    for fire_ms in due {
                        last_fire = Some(fire_ms);
                        if let Some(job) = self.enqueue_schedule_fire(&schedule, fire_ms)? {
                            report.enqueued = report.enqueued.saturating_add(1);
                            report.recoveries.push(ScheduleRecovery {
                                schedule_id: schedule.id.clone(),
                                task: schedule.task.clone(),
                                fire_ms,
                                job_id: Some(job.id),
                                policy: schedule.missed_policy,
                                action: "enqueued".to_string(),
                            });
                        }
                    }
                    self.mark_schedule_checked(&schedule.id, now, last_fire)?;
                }
            }
        }
        Ok(report)
    }

    pub fn set_global_concurrency_limit(
        &self,
        limit: u64,
    ) -> Result<QueueConcurrencyLimit, RuntimeError> {
        self.set_concurrency_limit("global", limit)
    }

    pub fn set_task_concurrency_limit(
        &self,
        task: &str,
        limit: u64,
    ) -> Result<QueueConcurrencyLimit, RuntimeError> {
        if task.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue task concurrency limit name must not be empty".to_string(),
            ));
        }
        self.set_concurrency_limit(&format!("task:{task}"), limit)
    }

    pub fn list_concurrency_limits(&self) -> Result<Vec<QueueConcurrencyLimit>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select scope, max_leased, updated_ms from queue_concurrency_limits order by scope",
            )
            .map_err(|err| {
                RuntimeError::Other(format!("failed to prepare concurrency limit list: {err}"))
            })?;
        let rows = stmt
            .query_map([], |row| {
                Ok(QueueConcurrencyLimit {
                    scope: row.get(0)?,
                    limit: row.get::<_, i64>(1)? as u64,
                    updated_ms: row.get::<_, i64>(2)? as u64,
                })
            })
            .map_err(|err| {
                RuntimeError::Other(format!("failed to list concurrency limits: {err}"))
            })?;
        let mut limits = Vec::new();
        for row in rows {
            limits.push(row.map_err(|err| {
                RuntimeError::Other(format!("failed to decode concurrency limit: {err}"))
            })?);
        }
        Ok(limits)
    }

    pub fn record_checkpoint(
        &self,
        job_id: &str,
        kind: JobCheckpointKind,
        label: impl Into<String>,
        payload: Value,
        payload_fingerprint: Option<String>,
    ) -> Result<JobCheckpoint, RuntimeError> {
        if self.get(job_id)?.is_none() {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{job_id}` not found"
            )));
        }
        let label = label.into();
        if label.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue checkpoint label must not be empty".to_string(),
            ));
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|err| {
            RuntimeError::Other(format!("failed to start checkpoint transaction: {err}"))
        })?;
        let sequence = tx
            .query_row(
                "select coalesce(max(sequence), 0) + 1 from queue_job_checkpoints where job_id = ?1",
                params![job_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|err| RuntimeError::Other(format!("failed to allocate checkpoint sequence: {err}")))?
            as u64;
        let id = format!("{job_id}:checkpoint:{sequence}");
        let payload_json = serde_json::to_string(&payload).map_err(|err| {
            RuntimeError::Other(format!("failed to serialize checkpoint payload: {err}"))
        })?;
        let now = now_ms();
        tx.execute(
            "insert into queue_job_checkpoints
             (id, job_id, sequence, kind, label, payload_json, payload_fingerprint, created_ms)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                job_id,
                sequence as i64,
                kind.as_str(),
                label,
                payload_json,
                payload_fingerprint,
                now as i64,
            ],
        )
        .map_err(|err| RuntimeError::Other(format!("failed to insert checkpoint: {err}")))?;
        tx.commit()
            .map_err(|err| RuntimeError::Other(format!("failed to commit checkpoint: {err}")))?;
        Ok(JobCheckpoint {
            id,
            job_id: job_id.to_string(),
            sequence,
            kind,
            label,
            payload,
            payload_fingerprint,
            created_ms: now,
        })
    }

    pub fn list_checkpoints(&self, job_id: &str) -> Result<Vec<JobCheckpoint>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select id, job_id, sequence, kind, label, payload_json, payload_fingerprint, created_ms
                 from queue_job_checkpoints where job_id = ?1 order by sequence",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to prepare checkpoint list: {err}")))?;
        let rows = stmt
            .query_map(params![job_id], read_checkpoint_row)
            .map_err(|err| RuntimeError::Other(format!("failed to list checkpoints: {err}")))?;
        let mut checkpoints = Vec::new();
        for row in rows {
            checkpoints.push(row.map_err(|err| {
                RuntimeError::Other(format!("failed to decode checkpoint: {err}"))
            })?);
        }
        Ok(checkpoints)
    }

    pub fn resume_state(&self, job_id: &str) -> Result<JobResumeState, RuntimeError> {
        let job = self
            .get(job_id)?
            .ok_or_else(|| RuntimeError::Other(format!("std.queue job `{job_id}` not found")))?;
        let checkpoints = self.list_checkpoints(job_id)?;
        let last_checkpoint = checkpoints.last().cloned();
        let next_sequence = last_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.sequence.saturating_add(1))
            .unwrap_or(1);
        Ok(JobResumeState {
            job,
            checkpoints,
            last_checkpoint,
            next_sequence,
        })
    }

    fn set_concurrency_limit(
        &self,
        scope: &str,
        limit: u64,
    ) -> Result<QueueConcurrencyLimit, RuntimeError> {
        if limit == 0 {
            return Err(RuntimeError::Other(
                "std.queue concurrency limit must be at least 1".to_string(),
            ));
        }
        let now = now_ms();
        self.conn
            .lock()
            .unwrap()
            .execute(
                "insert into queue_concurrency_limits (scope, max_leased, updated_ms)
                 values (?1, ?2, ?3)
                 on conflict(scope) do update set max_leased = excluded.max_leased, updated_ms = excluded.updated_ms",
                params![scope, limit as i64, now as i64],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to set concurrency limit: {err}")))?;
        Ok(QueueConcurrencyLimit {
            scope: scope.to_string(),
            limit,
            updated_ms: now,
        })
    }

    pub fn cancel(&self, id: &str) -> Result<QueueJob, RuntimeError> {
        let now = now_ms();
        let updated = self
            .conn
            .lock()
            .unwrap()
            .execute(
                "update queue_jobs set status = 'canceled', updated_ms = ?2 where id = ?1",
                params![id, now as i64],
            )
            .map_err(|err| {
                RuntimeError::Other(format!("failed to cancel durable queue job: {err}"))
            })?;
        if updated == 0 {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{id}` not found"
            )));
        }
        self.get(id)?
            .ok_or_else(|| RuntimeError::Other(format!("std.queue job `{id}` not found")))
    }

    pub fn run_one(&self) -> Result<Option<QueueJob>, RuntimeError> {
        self.run_one_with_output(None, None)
    }

    pub fn lease_next(
        &self,
        worker_id: impl Into<String>,
        ttl_ms: u64,
    ) -> Result<Option<QueueJob>, RuntimeError> {
        self.lease_next_at(worker_id, ttl_ms, now_ms())
    }

    pub fn lease_next_at(
        &self,
        worker_id: impl Into<String>,
        ttl_ms: u64,
        now: u64,
    ) -> Result<Option<QueueJob>, RuntimeError> {
        let worker_id = worker_id.into();
        if worker_id.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue lease worker id must not be empty".to_string(),
            ));
        }
        let lease_expires = now.saturating_add(ttl_ms.max(1));
        for candidate in self
            .list()?
            .into_iter()
            .filter(|job| eligible_to_lease(job, now))
        {
            if !self.concurrency_allows(&candidate.task, now)? {
                continue;
            }
            let updated = self
                .conn
                .lock()
                .unwrap()
                .execute(
                    "update queue_jobs
                     set status = 'leased', lease_owner = ?2, lease_expires_ms = ?3, updated_ms = ?4
                     where id = ?1
                       and (
                         status in ('pending', 'retry_wait')
                         or (status = 'leased' and coalesce(lease_expires_ms, 0) <= ?4)
                       )",
                    params![candidate.id, worker_id, lease_expires as i64, now as i64],
                )
                .map_err(|err| {
                    RuntimeError::Other(format!("failed to lease durable queue job: {err}"))
                })?;
            if updated == 1 {
                return self.get(&candidate.id);
            }
        }
        Ok(None)
    }

    fn concurrency_allows(&self, task: &str, now: u64) -> Result<bool, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let global_limit = read_concurrency_limit(&conn, "global")?;
        if let Some(limit) = global_limit {
            let active = count_active_leases(&conn, None, now)?;
            if active >= limit {
                return Ok(false);
            }
        }
        let scope = format!("task:{task}");
        if let Some(limit) = read_concurrency_limit(&conn, &scope)? {
            let active = count_active_leases(&conn, Some(task), now)?;
            if active >= limit {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn complete_leased(
        &self,
        id: &str,
        worker_id: &str,
        output_kind: Option<String>,
        output_fingerprint: Option<String>,
    ) -> Result<QueueJob, RuntimeError> {
        let now = now_ms();
        let updated = self
            .conn
            .lock()
            .unwrap()
            .execute(
                "update queue_jobs
                 set status = 'succeeded', attempts = attempts + 1, output_kind = ?3, output_fingerprint = ?4,
                     next_run_ms = null, lease_owner = null, lease_expires_ms = null, updated_ms = ?5
                 where id = ?1 and status = 'leased' and lease_owner = ?2",
                params![id, worker_id, output_kind, output_fingerprint, now as i64],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to complete leased queue job: {err}")))?;
        if updated == 0 {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{id}` is not leased by `{worker_id}`"
            )));
        }
        self.get(id)?
            .ok_or_else(|| RuntimeError::Other(format!("std.queue job `{id}` not found")))
    }

    pub fn fail_leased(
        &self,
        id: &str,
        worker_id: &str,
        failure_kind: impl Into<String>,
        failure_fingerprint: impl Into<String>,
        base_delay_ms: u64,
    ) -> Result<QueueJob, RuntimeError> {
        let mut job = self
            .get(id)?
            .ok_or_else(|| RuntimeError::Other(format!("std.queue job `{id}` not found")))?;
        if job.status != QueueJobStatus::Leased || job.lease_owner.as_deref() != Some(worker_id) {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{id}` is not leased by `{worker_id}`"
            )));
        }
        let now = now_ms();
        job.attempts = job.attempts.saturating_add(1);
        job.failure_kind = Some(failure_kind.into());
        job.failure_fingerprint = Some(failure_fingerprint.into());
        if job.attempts <= job.max_retries {
            job.status = QueueJobStatus::RetryWait;
            job.next_run_ms = Some(now.saturating_add(base_delay_ms.saturating_mul(job.attempts)));
        } else {
            job.status = QueueJobStatus::DeadLettered;
            job.next_run_ms = None;
        }
        let updated = self
            .conn
            .lock()
            .unwrap()
            .execute(
                "update queue_jobs
                 set status = ?3, attempts = ?4, failure_kind = ?5, failure_fingerprint = ?6,
                     next_run_ms = ?7, lease_owner = null, lease_expires_ms = null, updated_ms = ?8
                 where id = ?1 and status = 'leased' and lease_owner = ?2",
                params![
                    id,
                    worker_id,
                    job.status.as_str(),
                    job.attempts as i64,
                    job.failure_kind,
                    job.failure_fingerprint,
                    job.next_run_ms.map(|value| value as i64),
                    now as i64,
                ],
            )
            .map_err(|err| {
                RuntimeError::Other(format!("failed to fail leased queue job: {err}"))
            })?;
        if updated == 0 {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{id}` is not leased by `{worker_id}`"
            )));
        }
        self.get(id)?
            .ok_or_else(|| RuntimeError::Other(format!("std.queue job `{id}` not found")))
    }

    pub fn enter_approval_wait(
        &self,
        id: &str,
        worker_id: &str,
        approval_id: impl Into<String>,
        approval_expires_ms: u64,
        approval_reason: impl Into<String>,
    ) -> Result<QueueJob, RuntimeError> {
        let approval_id = approval_id.into();
        let approval_reason = approval_reason.into();
        if approval_id.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue approval id must not be empty".to_string(),
            ));
        }
        if approval_reason.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue approval reason must not be empty".to_string(),
            ));
        }
        let now = now_ms();
        let updated = self
            .conn
            .lock()
            .unwrap()
            .execute(
                "update queue_jobs
                 set status = 'approval_wait',
                     approval_id = ?3,
                     approval_expires_ms = ?4,
                     approval_reason = ?5,
                     next_run_ms = null,
                     lease_owner = null,
                     lease_expires_ms = null,
                     updated_ms = ?6
                 where id = ?1 and status = 'leased' and lease_owner = ?2",
                params![
                    id,
                    worker_id,
                    approval_id,
                    approval_expires_ms as i64,
                    approval_reason,
                    now as i64,
                ],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to enter approval wait: {err}")))?;
        if updated == 0 {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{id}` is not leased by `{worker_id}`"
            )));
        }
        self.get(id)?
            .ok_or_else(|| RuntimeError::Other(format!("std.queue job `{id}` not found")))
    }

    pub fn decide_approval_wait(
        &self,
        id: &str,
        approval_id: &str,
        decision: JobApprovalDecision,
        actor: impl Into<String>,
        reason: Option<String>,
    ) -> Result<QueueJob, RuntimeError> {
        self.decide_approval_wait_at(id, approval_id, decision, actor, reason, now_ms())
    }

    pub fn decide_approval_wait_at(
        &self,
        id: &str,
        approval_id: &str,
        decision: JobApprovalDecision,
        actor: impl Into<String>,
        reason: Option<String>,
        now: u64,
    ) -> Result<QueueJob, RuntimeError> {
        let actor = actor.into();
        if actor.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue approval actor must not be empty".to_string(),
            ));
        }
        if approval_id.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue approval id must not be empty".to_string(),
            ));
        }

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|err| {
            RuntimeError::Other(format!(
                "failed to start approval decision transaction: {err}"
            ))
        })?;
        let (stored_status, stored_approval_id, stored_expires_ms) = tx
            .query_row(
                "select status, approval_id, approval_expires_ms from queue_jobs where id = ?1",
                params![id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                    ))
                },
            )
            .map_err(|err| {
                RuntimeError::Other(format!(
                    "std.queue job `{id}` not found or unreadable: {err}"
                ))
            })?;
        if parse_status(&stored_status) != QueueJobStatus::ApprovalWait {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{id}` is not waiting on approval"
            )));
        }
        if stored_approval_id.as_deref() != Some(approval_id) {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{id}` is waiting on a different approval id"
            )));
        }
        if decision == JobApprovalDecision::Expire {
            let Some(expires_ms) = stored_expires_ms else {
                return Err(RuntimeError::Other(format!(
                    "std.queue job `{id}` approval has no expiry"
                )));
            };
            if (expires_ms as u64) > now {
                return Err(RuntimeError::Other(format!(
                    "std.queue job `{id}` approval `{approval_id}` has not expired"
                )));
            }
        }

        let (status_after, next_run_ms) = match decision {
            JobApprovalDecision::Approve => (QueueJobStatus::Pending, Some(now)),
            JobApprovalDecision::Deny => (QueueJobStatus::ApprovalDenied, None),
            JobApprovalDecision::Expire => (QueueJobStatus::ApprovalExpired, None),
        };
        let updated = tx
            .execute(
                "update queue_jobs
                 set status = ?3,
                     next_run_ms = ?4,
                     lease_owner = null,
                     lease_expires_ms = null,
                     updated_ms = ?5
                 where id = ?1 and status = 'approval_wait' and approval_id = ?2",
                params![
                    id,
                    approval_id,
                    status_after.as_str(),
                    next_run_ms.map(|value| value as i64),
                    now as i64,
                ],
            )
            .map_err(|err| {
                RuntimeError::Other(format!("failed to apply approval decision: {err}"))
            })?;
        if updated == 0 {
            return Err(RuntimeError::Other(format!(
                "std.queue job `{id}` approval decision raced with another transition"
            )));
        }
        insert_job_audit_event(
            &tx,
            id,
            &format!("approval_{}", decision.as_str()),
            &actor,
            Some(approval_id),
            &stored_status,
            status_after.as_str(),
            reason.as_deref(),
            now,
        )?;
        tx.commit().map_err(|err| {
            RuntimeError::Other(format!("failed to commit approval decision: {err}"))
        })?;
        drop(conn);
        self.get(id)?
            .ok_or_else(|| RuntimeError::Other(format!("std.queue job `{id}` not found")))
    }

    pub fn run_one_with_output(
        &self,
        output_kind: Option<String>,
        output_fingerprint: Option<String>,
    ) -> Result<Option<QueueJob>, RuntimeError> {
        let Some(job) = self.lease_next("corvid-run-one", 300_000)? else {
            return Ok(None);
        };
        self.complete_leased(&job.id, "corvid-run-one", output_kind, output_fingerprint)
            .map(Some)
    }

    pub fn run_one_failed(
        &self,
        failure_kind: impl Into<String>,
        failure_fingerprint: impl Into<String>,
        base_delay_ms: u64,
    ) -> Result<Option<QueueJob>, RuntimeError> {
        let Some(job) = self.lease_next("corvid-run-one", 300_000)? else {
            return Ok(None);
        };
        self.fail_leased(
            &job.id,
            "corvid-run-one",
            failure_kind,
            failure_fingerprint,
            base_delay_ms,
        )
        .map(Some)
    }

    fn enqueue_schedule_fire(
        &self,
        schedule: &QueueScheduleManifest,
        fire_ms: u64,
    ) -> Result<Option<QueueJob>, RuntimeError> {
        let event_id = format!("{}:{fire_ms}", schedule.id);
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|err| {
            RuntimeError::Other(format!(
                "failed to start schedule recovery transaction: {err}"
            ))
        })?;
        let inserted = tx
            .execute(
                "insert or ignore into queue_schedule_fires (event_id, schedule_id, fire_ms, job_id, created_ms)
                 values (?1, ?2, ?3, '', ?4)",
                params![event_id, schedule.id, fire_ms as i64, now_ms() as i64],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to record schedule fire: {err}")))?;
        if inserted == 0 {
            tx.commit().map_err(|err| {
                RuntimeError::Other(format!(
                    "failed to commit duplicate schedule fire record: {err}"
                ))
            })?;
            return Ok(None);
        }
        let mut payload = serde_json::Map::new();
        payload.insert(
            "schedule_id".to_string(),
            Value::String(schedule.id.clone()),
        );
        payload.insert(
            "scheduled_fire_ms".to_string(),
            Value::Number(serde_json::Number::from(fire_ms)),
        );
        payload.insert("payload".to_string(), schedule.payload.clone());
        tx.commit().map_err(|err| {
            RuntimeError::Other(format!("failed to commit schedule fire record: {err}"))
        })?;
        drop(conn);
        let replay_key = schedule
            .replay_key_prefix
            .as_ref()
            .map(|prefix| format!("{prefix}:{fire_ms}"));
        let job = self.enqueue_typed(
            schedule.task.clone(),
            Value::Object(payload),
            None,
            schedule.max_retries,
            schedule.budget_usd,
            schedule.effect_summary.clone(),
            replay_key,
        )?;
        self.conn
            .lock()
            .unwrap()
            .execute(
                "update queue_schedule_fires set job_id = ?2 where event_id = ?1",
                params![event_id, job.id],
            )
            .map_err(|err| {
                RuntimeError::Other(format!("failed to link schedule fire to job: {err}"))
            })?;
        Ok(Some(job))
    }

    fn mark_schedule_checked(
        &self,
        schedule_id: &str,
        checked_ms: u64,
        last_fire_ms: Option<u64>,
    ) -> Result<(), RuntimeError> {
        self.conn
            .lock()
            .unwrap()
            .execute(
                "update queue_schedules set last_checked_ms = ?2, last_fire_ms = coalesce(?3, last_fire_ms), updated_ms = ?2 where id = ?1",
                params![schedule_id, checked_ms as i64, last_fire_ms.map(|value| value as i64)],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to update schedule recovery cursor: {err}")))?;
        Ok(())
    }

    fn init(&self) -> Result<(), RuntimeError> {
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

    fn ensure_column(&self, name: &str, ty: &str) -> Result<(), RuntimeError> {
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

    fn seed_next_id(&self) -> Result<(), RuntimeError> {
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

fn read_job_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueueJob> {
    let status: String = row.get(4)?;
    let payload_json: String = row.get(2)?;
    Ok(QueueJob {
        id: row.get(0)?,
        task: row.get(1)?,
        payload: serde_json::from_str(&payload_json).unwrap_or(Value::Null),
        input_schema: row.get(3)?,
        status: parse_status(&status),
        attempts: row.get::<_, i64>(5)? as u64,
        max_retries: row.get::<_, i64>(6)? as u64,
        budget_usd: row.get(7)?,
        effect_summary: row.get(8)?,
        replay_key: row.get(9)?,
        idempotency_key: row.get(10)?,
        output_kind: row.get(11)?,
        output_fingerprint: row.get(12)?,
        failure_kind: row.get(13)?,
        failure_fingerprint: row.get(14)?,
        next_run_ms: row.get::<_, Option<i64>>(15)?.map(|value| value as u64),
        lease_owner: row.get(16)?,
        lease_expires_ms: row.get::<_, Option<i64>>(17)?.map(|value| value as u64),
        approval_id: row.get(18)?,
        approval_expires_ms: row.get::<_, Option<i64>>(19)?.map(|value| value as u64),
        approval_reason: row.get(20)?,
        created_ms: row.get::<_, i64>(21)? as u64,
        updated_ms: row.get::<_, i64>(22)? as u64,
    })
}

fn read_schedule_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueueScheduleManifest> {
    let payload_json: String = row.get(4)?;
    Ok(QueueScheduleManifest {
        id: row.get(0)?,
        cron: row.get(1)?,
        zone: row.get(2)?,
        task: row.get(3)?,
        payload: serde_json::from_str(&payload_json).unwrap_or(Value::Null),
        max_retries: row.get::<_, i64>(5)? as u64,
        budget_usd: row.get(6)?,
        effect_summary: row.get(7)?,
        replay_key_prefix: row.get(8)?,
        missed_policy: parse_missed_policy(&row.get::<_, String>(9)?),
        last_checked_ms: row.get::<_, i64>(10)? as u64,
        last_fire_ms: row.get::<_, Option<i64>>(11)?.map(|value| value as u64),
        created_ms: row.get::<_, i64>(12)? as u64,
        updated_ms: row.get::<_, i64>(13)? as u64,
    })
}

fn read_checkpoint_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobCheckpoint> {
    let payload_json: String = row.get(5)?;
    Ok(JobCheckpoint {
        id: row.get(0)?,
        job_id: row.get(1)?,
        sequence: row.get::<_, i64>(2)? as u64,
        kind: parse_checkpoint_kind(&row.get::<_, String>(3)?),
        label: row.get(4)?,
        payload: serde_json::from_str(&payload_json).unwrap_or(Value::Null),
        payload_fingerprint: row.get(6)?,
        created_ms: row.get::<_, i64>(7)? as u64,
    })
}

fn read_job_audit_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobAuditEvent> {
    Ok(JobAuditEvent {
        id: row.get(0)?,
        job_id: row.get(1)?,
        event_kind: row.get(2)?,
        actor: row.get(3)?,
        approval_id: row.get(4)?,
        status_before: row.get(5)?,
        status_after: row.get(6)?,
        reason: row.get(7)?,
        created_ms: row.get::<_, i64>(8)? as u64,
    })
}

fn read_loop_limits_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobLoopLimits> {
    Ok(JobLoopLimits {
        job_id: row.get(0)?,
        max_steps: row.get::<_, Option<i64>>(1)?.map(|value| value as u64),
        max_wall_ms: row.get::<_, Option<i64>>(2)?.map(|value| value as u64),
        max_spend_usd: row.get(3)?,
        max_tool_calls: row.get::<_, Option<i64>>(4)?.map(|value| value as u64),
        updated_ms: row.get::<_, i64>(5)? as u64,
    })
}

fn read_loop_usage_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobLoopUsage> {
    Ok(JobLoopUsage {
        job_id: row.get(0)?,
        steps: row.get::<_, i64>(1)? as u64,
        wall_ms: row.get::<_, i64>(2)? as u64,
        spend_usd: row.get(3)?,
        tool_calls: row.get::<_, i64>(4)? as u64,
        updated_ms: row.get::<_, i64>(5)? as u64,
    })
}

fn loop_bound_violations(usage: &JobLoopUsage, limits: &JobLoopLimits) -> Vec<String> {
    let mut violations = Vec::new();
    if limits.max_steps.is_some_and(|limit| usage.steps > limit) {
        violations.push(format!(
            "max_steps:{}>{}",
            usage.steps,
            limits.max_steps.unwrap()
        ));
    }
    if limits.max_wall_ms.is_some_and(|limit| usage.wall_ms > limit) {
        violations.push(format!(
            "max_wall_ms:{}>{}",
            usage.wall_ms,
            limits.max_wall_ms.unwrap()
        ));
    }
    if limits
        .max_spend_usd
        .is_some_and(|limit| usage.spend_usd > limit)
    {
        violations.push(format!(
            "max_spend_usd:{:.6}>{:.6}",
            usage.spend_usd,
            limits.max_spend_usd.unwrap()
        ));
    }
    if limits
        .max_tool_calls
        .is_some_and(|limit| usage.tool_calls > limit)
    {
        violations.push(format!(
            "max_tool_calls:{}>{}",
            usage.tool_calls,
            limits.max_tool_calls.unwrap()
        ));
    }
    violations
}

fn insert_job_audit_event(
    tx: &rusqlite::Transaction<'_>,
    job_id: &str,
    event_kind: &str,
    actor: &str,
    approval_id: Option<&str>,
    status_before: &str,
    status_after: &str,
    reason: Option<&str>,
    created_ms: u64,
) -> Result<(), RuntimeError> {
    let next = tx
        .query_row(
            "select coalesce(max(cast(substr(id, 7) as integer)), 0) + 1 from queue_job_audit_events where id like 'audit_%'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|err| RuntimeError::Other(format!("failed to allocate job audit id: {err}")))?;
    let id = format!("audit_{}", next.max(1));
    tx.execute(
        "insert into queue_job_audit_events
         (id, job_id, event_kind, actor, approval_id, status_before, status_after, reason, created_ms)
         values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            job_id,
            event_kind,
            actor,
            approval_id,
            status_before,
            status_after,
            reason,
            created_ms as i64,
        ],
    )
    .map_err(|err| RuntimeError::Other(format!("failed to insert job audit event: {err}")))?;
    Ok(())
}

fn parse_status(status: &str) -> QueueJobStatus {
    match status {
        "leased" => QueueJobStatus::Leased,
        "approval_wait" => QueueJobStatus::ApprovalWait,
        "approval_denied" => QueueJobStatus::ApprovalDenied,
        "approval_expired" => QueueJobStatus::ApprovalExpired,
        "loop_budget_exceeded" => QueueJobStatus::LoopBudgetExceeded,
        "retry_wait" => QueueJobStatus::RetryWait,
        "running" => QueueJobStatus::Running,
        "succeeded" => QueueJobStatus::Succeeded,
        "failed" => QueueJobStatus::Failed,
        "dead_lettered" => QueueJobStatus::DeadLettered,
        "canceled" => QueueJobStatus::Canceled,
        _ => QueueJobStatus::Pending,
    }
}

fn parse_missed_policy(policy: &str) -> ScheduleMissedPolicy {
    match policy {
        "skip_missed" => ScheduleMissedPolicy::SkipMissed,
        "enqueue_all_bounded" => ScheduleMissedPolicy::EnqueueAllBounded,
        _ => ScheduleMissedPolicy::FireOnceOnRecovery,
    }
}

fn parse_checkpoint_kind(kind: &str) -> JobCheckpointKind {
    match kind {
        "tool_result" => JobCheckpointKind::ToolResult,
        "partial_output" => JobCheckpointKind::PartialOutput,
        _ => JobCheckpointKind::AgentStep,
    }
}

fn validate_schedule(cron: &str, zone: &str) -> Result<(), RuntimeError> {
    let expression = normalize_cron(cron)?;
    Schedule::from_str(&expression)
        .map_err(|err| RuntimeError::Other(format!("invalid cron expression `{cron}`: {err}")))?;
    zone.parse::<Tz>()
        .map_err(|err| RuntimeError::Other(format!("invalid schedule timezone `{zone}`: {err}")))?;
    Ok(())
}

fn missed_fire_times(
    schedule: &QueueScheduleManifest,
    now: u64,
    max_missed_per_schedule: usize,
) -> Result<Vec<u64>, RuntimeError> {
    if now <= schedule.last_checked_ms || max_missed_per_schedule == 0 {
        return Ok(Vec::new());
    }
    let expression = normalize_cron(&schedule.cron)?;
    let cron = Schedule::from_str(&expression).map_err(|err| {
        RuntimeError::Other(format!(
            "invalid cron expression `{}`: {err}",
            schedule.cron
        ))
    })?;
    let zone = schedule.zone.parse::<Tz>().map_err(|err| {
        RuntimeError::Other(format!(
            "invalid schedule timezone `{}`: {err}",
            schedule.zone
        ))
    })?;
    let start_ms = schedule
        .last_fire_ms
        .unwrap_or(schedule.last_checked_ms)
        .saturating_add(1);
    let start = Utc
        .timestamp_millis_opt(start_ms as i64)
        .single()
        .ok_or_else(|| {
            RuntimeError::Other(format!("invalid schedule recovery start `{start_ms}`"))
        })?;
    let end = Utc
        .timestamp_millis_opt(now as i64)
        .single()
        .ok_or_else(|| RuntimeError::Other(format!("invalid schedule recovery end `{now}`")))?;
    let start_local = start.with_timezone(&zone);
    let end_local = end.with_timezone(&zone);
    let mut fires = Vec::new();
    for fire in cron.after(&start_local).take(max_missed_per_schedule) {
        if fire > end_local {
            break;
        }
        fires.push(fire.with_timezone(&Utc).timestamp_millis() as u64);
    }
    Ok(fires)
}

fn normalize_cron(cron: &str) -> Result<String, RuntimeError> {
    let fields = cron.split_whitespace().collect::<Vec<_>>();
    match fields.len() {
        5 => Ok(format!(
            "0 {} {} {} {} {} *",
            fields[0], fields[1], fields[2], fields[3], fields[4]
        )),
        6 => Ok(format!(
            "{} {} {} {} {} {} *",
            fields[0], fields[1], fields[2], fields[3], fields[4], fields[5]
        )),
        7 => Ok(cron.to_string()),
        _ => Err(RuntimeError::Other(format!(
            "invalid cron expression `{cron}`: expected 5, 6, or 7 fields"
        ))),
    }
}

fn read_concurrency_limit(conn: &Connection, scope: &str) -> Result<Option<u64>, RuntimeError> {
    let mut stmt = conn
        .prepare("select max_leased from queue_concurrency_limits where scope = ?1")
        .map_err(|err| {
            RuntimeError::Other(format!("failed to prepare concurrency limit read: {err}"))
        })?;
    let mut rows = stmt
        .query(params![scope])
        .map_err(|err| RuntimeError::Other(format!("failed to query concurrency limit: {err}")))?;
    if let Some(row) = rows
        .next()
        .map_err(|err| RuntimeError::Other(format!("failed to read concurrency limit: {err}")))?
    {
        Ok(Some(row.get::<_, i64>(0).map_err(|err| {
            RuntimeError::Other(format!("failed to decode concurrency limit: {err}"))
        })? as u64))
    } else {
        Ok(None)
    }
}

fn count_active_leases(
    conn: &Connection,
    task: Option<&str>,
    now: u64,
) -> Result<u64, RuntimeError> {
    let sql = if task.is_some() {
        "select count(*) from queue_jobs where status = 'leased' and coalesce(lease_expires_ms, 0) > ?1 and task = ?2"
    } else {
        "select count(*) from queue_jobs where status = 'leased' and coalesce(lease_expires_ms, 0) > ?1"
    };
    let count = if let Some(task) = task {
        conn.query_row(sql, params![now as i64, task], |row| row.get::<_, i64>(0))
    } else {
        conn.query_row(sql, params![now as i64], |row| row.get::<_, i64>(0))
    }
    .map_err(|err| RuntimeError::Other(format!("failed to count active leases: {err}")))?;
    Ok(count.max(0) as u64)
}

fn eligible_to_run(job: &QueueJob) -> bool {
    match job.status {
        QueueJobStatus::Pending => job.next_run_ms.map(|next| next <= now_ms()).unwrap_or(true),
        QueueJobStatus::RetryWait => job.next_run_ms.map(|next| next <= now_ms()).unwrap_or(true),
        _ => false,
    }
}

fn eligible_to_lease(job: &QueueJob, now: u64) -> bool {
    match job.status {
        QueueJobStatus::Pending | QueueJobStatus::RetryWait => {
            job.next_run_ms.map(|next| next <= now).unwrap_or(true)
        }
        QueueJobStatus::Leased => job
            .lease_expires_ms
            .map(|expires| expires <= now)
            .unwrap_or(true),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_enqueue_and_cancel_preserve_metadata() {
        let queue = QueueRuntime::new();
        let job = queue
            .enqueue(
                "embed",
                serde_json::json!({"doc": "a"}),
                3,
                0.25,
                Some("llm+io".to_string()),
                Some("trace:1".to_string()),
            )
            .unwrap();

        assert_eq!(job.status, QueueJobStatus::Pending);
        assert_eq!(job.max_retries, 3);
        assert_eq!(job.effect_summary.as_deref(), Some("llm+io"));
        assert_eq!(
            queue.get(&job.id).unwrap().replay_key.as_deref(),
            Some("trace:1")
        );

        let canceled = queue.cancel(&job.id).unwrap();
        assert_eq!(canceled.status, QueueJobStatus::Canceled);
    }

    #[test]
    fn durable_queue_persists_jobs_and_supports_cancel_and_list() {
        let queue = DurableQueueRuntime::open_in_memory().unwrap();
        let job = queue
            .enqueue(
                "embed",
                serde_json::json!({"doc": "a"}),
                2,
                1.25,
                Some("llm+io".to_string()),
                Some("trace:2".to_string()),
            )
            .unwrap();

        let fetched = queue.get(&job.id).unwrap().unwrap();
        assert_eq!(fetched.task, "embed");
        assert_eq!(fetched.replay_key.as_deref(), Some("trace:2"));

        let listed = queue.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].effect_summary.as_deref(), Some("llm+io"));

        let canceled = queue.cancel(&job.id).unwrap();
        assert_eq!(canceled.status, QueueJobStatus::Canceled);
    }

    #[test]
    fn durable_queue_run_one_persists_success() {
        let queue = DurableQueueRuntime::open_in_memory().unwrap();
        let job = queue
            .enqueue_typed(
                "daily_brief",
                serde_json::json!({"user": "u1"}),
                Some("DailyBriefInput".to_string()),
                2,
                0.5,
                Some("llm+db".to_string()),
                Some("replay:job".to_string()),
            )
            .unwrap();

        let ran = queue
            .run_one_with_output(
                Some("DailyBriefOutput".to_string()),
                Some("sha256:daily-output".to_string()),
            )
            .unwrap()
            .expect("pending job");
        assert_eq!(ran.id, job.id);
        assert_eq!(ran.status, QueueJobStatus::Succeeded);
        assert_eq!(ran.attempts, 1);
        assert_eq!(ran.input_schema.as_deref(), Some("DailyBriefInput"));
        assert_eq!(ran.output_kind.as_deref(), Some("DailyBriefOutput"));
        assert_eq!(
            ran.output_fingerprint.as_deref(),
            Some("sha256:daily-output")
        );

        let fetched = queue.get(&job.id).unwrap().unwrap();
        assert_eq!(fetched.status, QueueJobStatus::Succeeded);
        assert_eq!(fetched.attempts, 1);
        assert_eq!(fetched.output_kind.as_deref(), Some("DailyBriefOutput"));
        assert!(queue.run_one().unwrap().is_none());
    }

    #[test]
    fn durable_queue_leases_prevent_duplicate_workers() {
        let queue = DurableQueueRuntime::open_in_memory().unwrap();
        let job = queue
            .enqueue(
                "dangerous_send",
                serde_json::json!({"draft": "d1"}),
                1,
                2.0,
                Some("email_send".to_string()),
                Some("replay:send:d1".to_string()),
            )
            .unwrap();

        let leased = queue
            .lease_next_at("worker-a", 60_000, 1_000_000)
            .unwrap()
            .expect("worker-a lease");
        assert_eq!(leased.id, job.id);
        assert_eq!(leased.status, QueueJobStatus::Leased);
        assert_eq!(leased.lease_owner.as_deref(), Some("worker-a"));
        assert_eq!(leased.lease_expires_ms, Some(1_060_000));

        assert!(
            queue
                .lease_next_at("worker-b", 60_000, 1_000_001)
                .unwrap()
                .is_none(),
            "second worker must not lease an active lease"
        );
        let wrong_owner = queue.complete_leased(&job.id, "worker-b", None, None);
        assert!(wrong_owner.is_err(), "non-owner completion must fail");

        let succeeded = queue
            .complete_leased(
                &job.id,
                "worker-a",
                Some("SendOutput".to_string()),
                Some("sha256:send".to_string()),
            )
            .unwrap();
        assert_eq!(succeeded.status, QueueJobStatus::Succeeded);
        assert!(succeeded.lease_owner.is_none());
        assert!(succeeded.lease_expires_ms.is_none());
        assert_eq!(succeeded.attempts, 1);
    }

    #[test]
    fn durable_queue_expired_lease_can_be_reclaimed() {
        let queue = DurableQueueRuntime::open_in_memory().unwrap();
        let job = queue
            .enqueue("brief", serde_json::json!({}), 1, 0.1, None, None)
            .unwrap();

        let first = queue
            .lease_next_at("worker-a", 10, 2_000)
            .unwrap()
            .expect("first lease");
        assert_eq!(first.id, job.id);

        let reclaimed = queue
            .lease_next_at("worker-b", 10, 2_011)
            .unwrap()
            .expect("reclaimed lease");
        assert_eq!(reclaimed.id, job.id);
        assert_eq!(reclaimed.lease_owner.as_deref(), Some("worker-b"));
        assert_eq!(reclaimed.lease_expires_ms, Some(2_021));
    }

    #[test]
    fn durable_queue_enforces_global_and_task_concurrency_limits() {
        let queue = DurableQueueRuntime::open_in_memory().unwrap();
        queue.set_global_concurrency_limit(1).unwrap();
        queue.set_task_concurrency_limit("email", 1).unwrap();
        queue
            .enqueue("email", serde_json::json!({"n": 1}), 1, 0.1, None, None)
            .unwrap();
        queue
            .enqueue("email", serde_json::json!({"n": 2}), 1, 0.1, None, None)
            .unwrap();
        queue
            .enqueue("brief", serde_json::json!({"n": 3}), 1, 0.1, None, None)
            .unwrap();

        let first = queue
            .lease_next_at("worker-a", 60_000, 5_000)
            .unwrap()
            .expect("first lease");
        assert_eq!(first.task, "email");
        assert!(
            queue
                .lease_next_at("worker-b", 60_000, 5_001)
                .unwrap()
                .is_none(),
            "global limit should block every other task while one lease is active"
        );

        queue
            .complete_leased(&first.id, "worker-a", None, None)
            .unwrap();
        let second = queue
            .lease_next_at("worker-b", 60_000, 5_002)
            .unwrap()
            .expect("second email lease");
        assert_eq!(second.task, "email");
        assert!(
            queue
                .lease_next_at("worker-c", 60_000, 5_003)
                .unwrap()
                .is_none(),
            "task limit should block another email lease"
        );

        let limits = queue.list_concurrency_limits().unwrap();
        assert_eq!(limits.len(), 2);
        assert!(limits
            .iter()
            .any(|limit| limit.scope == "global" && limit.limit == 1));
        assert!(limits
            .iter()
            .any(|limit| limit.scope == "task:email" && limit.limit == 1));
    }

    #[test]
    fn durable_queue_idempotency_key_collapses_duplicate_jobs() {
        let queue = DurableQueueRuntime::open_in_memory().unwrap();
        let first = queue
            .enqueue_typed_idempotent(
                "charge_card",
                serde_json::json!({"invoice": "i1"}),
                Some("ChargeInput".to_string()),
                1,
                10.0,
                Some("payment".to_string()),
                Some("replay:charge:i1".to_string()),
                Some("charge:i1".to_string()),
                None,
            )
            .unwrap();
        let duplicate = queue
            .enqueue_typed_idempotent(
                "charge_card",
                serde_json::json!({"invoice": "i1", "changed": true}),
                Some("ChargeInput".to_string()),
                1,
                10.0,
                Some("payment".to_string()),
                Some("replay:charge:i1:duplicate".to_string()),
                Some("charge:i1".to_string()),
                None,
            )
            .unwrap();

        assert_eq!(first.id, duplicate.id);
        assert_eq!(duplicate.payload["invoice"], "i1");
        assert!(duplicate.payload.get("changed").is_none());
        assert_eq!(duplicate.idempotency_key.as_deref(), Some("charge:i1"));
        assert_eq!(queue.list().unwrap().len(), 1);
    }

    #[test]
    fn durable_queue_records_ordered_agent_checkpoints() {
        let queue = DurableQueueRuntime::open_in_memory().unwrap();
        let job = queue
            .enqueue(
                "agent_run",
                serde_json::json!({"goal": "brief"}),
                1,
                0.5,
                None,
                None,
            )
            .unwrap();

        let step = queue
            .record_checkpoint(
                &job.id,
                JobCheckpointKind::AgentStep,
                "plan",
                serde_json::json!({"step": 1}),
                Some("sha256:plan".to_string()),
            )
            .unwrap();
        let tool = queue
            .record_checkpoint(
                &job.id,
                JobCheckpointKind::ToolResult,
                "gmail.search",
                serde_json::json!({"result_count": 3}),
                Some("sha256:gmail".to_string()),
            )
            .unwrap();
        let partial = queue
            .record_checkpoint(
                &job.id,
                JobCheckpointKind::PartialOutput,
                "draft",
                serde_json::json!({"chars": 120}),
                None,
            )
            .unwrap();

        assert_eq!(step.sequence, 1);
        assert_eq!(tool.sequence, 2);
        assert_eq!(partial.sequence, 3);
        let checkpoints = queue.list_checkpoints(&job.id).unwrap();
        assert_eq!(checkpoints.len(), 3);
        assert_eq!(checkpoints[0].kind, JobCheckpointKind::AgentStep);
        assert_eq!(checkpoints[1].label, "gmail.search");
        assert_eq!(checkpoints[2].payload["chars"], 120);
    }

    #[test]
    fn durable_queue_resume_state_survives_restart_and_expired_lease() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.sqlite");
        let job_id = {
            let queue = DurableQueueRuntime::open(&path).unwrap();
            let job = queue
                .enqueue(
                    "agent_run",
                    serde_json::json!({"goal": "brief"}),
                    1,
                    0.5,
                    None,
                    None,
                )
                .unwrap();
            let leased = queue
                .lease_next_at("worker-a", 10, 10_000)
                .unwrap()
                .expect("lease");
            assert_eq!(leased.id, job.id);
            queue
                .record_checkpoint(
                    &job.id,
                    JobCheckpointKind::AgentStep,
                    "plan",
                    serde_json::json!({"step": 1}),
                    Some("sha256:plan".to_string()),
                )
                .unwrap();
            queue
                .record_checkpoint(
                    &job.id,
                    JobCheckpointKind::ToolResult,
                    "gmail.search",
                    serde_json::json!({"result_count": 3}),
                    Some("sha256:gmail".to_string()),
                )
                .unwrap();
            job.id
        };

        let queue = DurableQueueRuntime::open(&path).unwrap();
        let resume = queue.resume_state(&job_id).unwrap();
        assert_eq!(resume.job.status, QueueJobStatus::Leased);
        assert_eq!(resume.checkpoints.len(), 2);
        assert_eq!(resume.next_sequence, 3);
        assert_eq!(
            resume
                .last_checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.label.as_str()),
            Some("gmail.search")
        );

        let reclaimed = queue
            .lease_next_at("worker-b", 10, 10_011)
            .unwrap()
            .expect("reclaimed after restart");
        assert_eq!(reclaimed.id, job_id);
        assert_eq!(reclaimed.lease_owner.as_deref(), Some("worker-b"));
    }

    #[test]
    fn durable_queue_enters_approval_wait_and_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.sqlite");
        let approval_expires_ms = now_ms().saturating_add(600_000);
        let job_id = {
            let queue = DurableQueueRuntime::open(&path).unwrap();
            let job = queue
                .enqueue(
                    "send_email",
                    serde_json::json!({"draft": "d1"}),
                    1,
                    0.25,
                    Some("email:write approve:send".to_string()),
                    Some("replay:email:d1".to_string()),
                )
                .unwrap();
            let leased = queue
                .lease_next_at("worker-a", 60_000, now_ms())
                .unwrap()
                .expect("lease");
            assert_eq!(leased.id, job.id);
            let waiting = queue
                .enter_approval_wait(
                    &job.id,
                    "worker-a",
                    "approval:send:d1",
                    approval_expires_ms,
                    "send external email draft d1",
                )
                .unwrap();
            assert_eq!(waiting.status, QueueJobStatus::ApprovalWait);
            assert_eq!(waiting.approval_id.as_deref(), Some("approval:send:d1"));
            assert_eq!(waiting.approval_expires_ms, Some(approval_expires_ms));
            assert_eq!(
                waiting.approval_reason.as_deref(),
                Some("send external email draft d1")
            );
            assert!(waiting.lease_owner.is_none());
            assert!(waiting.lease_expires_ms.is_none());
            assert!(
                queue
                    .lease_next_at("worker-b", 60_000, now_ms())
                    .unwrap()
                    .is_none(),
                "approval-wait jobs must not be leased as runnable work"
            );
            job.id
        };

        let queue = DurableQueueRuntime::open(&path).unwrap();
        let stored = queue.get(&job_id).unwrap().expect("stored job");
        assert_eq!(stored.status, QueueJobStatus::ApprovalWait);
        assert_eq!(stored.approval_id.as_deref(), Some("approval:send:d1"));
        assert_eq!(stored.approval_expires_ms, Some(approval_expires_ms));
        assert_eq!(queue.approval_waiting().unwrap().len(), 1);
        assert!(queue.lease_next("worker-c", 60_000).unwrap().is_none());
    }

    #[test]
    fn durable_queue_approval_decisions_resume_or_stop_with_audit() {
        let queue = DurableQueueRuntime::open_in_memory().unwrap();
        let approved = queue
            .enqueue(
                "send_email",
                serde_json::json!({"draft": "a"}),
                1,
                0.25,
                None,
                None,
            )
            .unwrap();
        let denied = queue
            .enqueue(
                "send_email",
                serde_json::json!({"draft": "d"}),
                1,
                0.25,
                None,
                None,
            )
            .unwrap();
        let expired = queue
            .enqueue(
                "send_email",
                serde_json::json!({"draft": "e"}),
                1,
                0.25,
                None,
                None,
            )
            .unwrap();

        for (job, approval_id, expires_ms) in [
            (&approved, "approval:a", 20_000),
            (&denied, "approval:d", 20_000),
            (&expired, "approval:e", 10_000),
        ] {
            let leased = queue
                .lease_next_at("worker-a", 60_000, 1_000)
                .unwrap()
                .expect("lease");
            assert_eq!(leased.id, job.id);
            queue
                .enter_approval_wait(
                    &job.id,
                    "worker-a",
                    approval_id,
                    expires_ms,
                    format!("decide {approval_id}"),
                )
                .unwrap();
        }

        let resumed = queue
            .decide_approval_wait_at(
                &approved.id,
                "approval:a",
                JobApprovalDecision::Approve,
                "reviewer:u1",
                Some("approved by policy".to_string()),
                12_000,
            )
            .unwrap();
        assert_eq!(resumed.status, QueueJobStatus::Pending);
        assert_eq!(resumed.next_run_ms, Some(12_000));
        let runnable = queue
            .lease_next_at("worker-b", 60_000, 12_001)
            .unwrap()
            .expect("approved job resumes");
        assert_eq!(runnable.id, approved.id);

        let stopped = queue
            .decide_approval_wait_at(
                &denied.id,
                "approval:d",
                JobApprovalDecision::Deny,
                "reviewer:u1",
                Some("recipient mismatch".to_string()),
                12_002,
            )
            .unwrap();
        assert_eq!(stopped.status, QueueJobStatus::ApprovalDenied);
        assert!(stopped.next_run_ms.is_none());

        let too_early = queue.decide_approval_wait_at(
            &expired.id,
            "approval:e",
            JobApprovalDecision::Expire,
            "system",
            Some("timer fired early".to_string()),
            9_999,
        );
        assert!(too_early.is_err());
        let expired_job = queue
            .decide_approval_wait_at(
                &expired.id,
                "approval:e",
                JobApprovalDecision::Expire,
                "system",
                Some("approval expired".to_string()),
                10_001,
            )
            .unwrap();
        assert_eq!(expired_job.status, QueueJobStatus::ApprovalExpired);

        let approved_events = queue.job_audit_events(&approved.id).unwrap();
        assert_eq!(approved_events.len(), 1);
        assert_eq!(approved_events[0].event_kind, "approval_approve");
        assert_eq!(approved_events[0].status_before, "approval_wait");
        assert_eq!(approved_events[0].status_after, "pending");
        assert_eq!(
            approved_events[0].approval_id.as_deref(),
            Some("approval:a")
        );
        let denied_events = queue.job_audit_events(&denied.id).unwrap();
        assert_eq!(denied_events[0].event_kind, "approval_deny");
        assert_eq!(denied_events[0].status_after, "approval_denied");
        let expired_events = queue.job_audit_events(&expired.id).unwrap();
        assert_eq!(expired_events[0].event_kind, "approval_expire");
        assert_eq!(expired_events[0].status_after, "approval_expired");
    }

    #[test]
    fn durable_queue_enforces_loop_budget_limits_with_audit() {
        let queue = DurableQueueRuntime::open_in_memory().unwrap();
        let job = queue
            .enqueue(
                "daily_brief_agent",
                serde_json::json!({"user": "u1"}),
                1,
                0.20,
                Some("llm+tools".to_string()),
                Some("replay:brief:u1".to_string()),
            )
            .unwrap();
        queue
            .set_loop_limits(&job.id, Some(3), Some(1_000), Some(0.20), Some(2))
            .unwrap();

        let first = queue
            .record_loop_usage_at(&job.id, 1, 250, 0.05, 1, "worker-a", 10_000)
            .unwrap();
        assert!(first.violated_bounds.is_empty());
        assert_eq!(first.usage.steps, 1);
        assert_eq!(queue.get(&job.id).unwrap().unwrap().status, QueueJobStatus::Pending);

        let exceeded = queue
            .record_loop_usage_at(&job.id, 3, 900, 0.16, 2, "worker-a", 10_100)
            .unwrap();
        assert_eq!(exceeded.usage.steps, 4);
        assert_eq!(exceeded.usage.wall_ms, 1_150);
        assert_eq!(exceeded.usage.tool_calls, 3);
        assert!(exceeded
            .violated_bounds
            .iter()
            .any(|bound| bound.starts_with("max_steps:4>3")));
        assert!(exceeded
            .violated_bounds
            .iter()
            .any(|bound| bound.starts_with("max_wall_ms:1150>1000")));
        assert!(exceeded
            .violated_bounds
            .iter()
            .any(|bound| bound.starts_with("max_spend_usd:0.210000>0.200000")));
        assert!(exceeded
            .violated_bounds
            .iter()
            .any(|bound| bound.starts_with("max_tool_calls:3>2")));

        let stopped = queue.get(&job.id).unwrap().unwrap();
        assert_eq!(stopped.status, QueueJobStatus::LoopBudgetExceeded);
        assert_eq!(stopped.failure_kind.as_deref(), Some("loop_bound_exceeded"));
        assert!(stopped
            .failure_fingerprint
            .as_deref()
            .unwrap_or_default()
            .contains("max_steps:4>3"));
        let audit = queue.job_audit_events(&job.id).unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].event_kind, "loop_bound_exceeded");
        assert_eq!(audit[0].actor, "worker-a");
        assert_eq!(audit[0].status_after, "loop_budget_exceeded");
        assert!(queue
            .record_loop_usage_at(&job.id, 1, 1, 0.01, 0, "worker-a", 10_200)
            .is_err());
    }

    #[test]
    fn durable_queue_delays_jobs_until_next_run() {
        let queue = DurableQueueRuntime::open_in_memory().unwrap();
        let future = now_ms().saturating_add(60_000);
        let delayed = queue
            .enqueue_typed_at(
                "scheduled_digest",
                serde_json::json!({"team": "eng"}),
                Some("DigestInput".to_string()),
                1,
                0.25,
                Some("llm+email".to_string()),
                Some("replay:digest".to_string()),
                Some(future),
            )
            .unwrap();
        let immediate = queue
            .enqueue(
                "immediate_digest",
                serde_json::json!({"team": "ops"}),
                1,
                0.25,
                None,
                None,
            )
            .unwrap();

        let ran = queue.run_one().unwrap().expect("immediate job");
        assert_eq!(ran.id, immediate.id);
        assert_eq!(ran.status, QueueJobStatus::Succeeded);
        assert!(
            queue.run_one().unwrap().is_none(),
            "future job should not run"
        );

        let stored = queue.get(&delayed.id).unwrap().unwrap();
        assert_eq!(stored.status, QueueJobStatus::Pending);
        assert_eq!(stored.next_run_ms, Some(future));
    }

    #[test]
    fn durable_scheduler_recovers_latest_missed_fire_after_restart() {
        let queue = DurableQueueRuntime::open_in_memory().unwrap();
        let start = Utc
            .with_ymd_and_hms(2026, 4, 29, 8, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis() as u64;
        let now = Utc
            .with_ymd_and_hms(2026, 4, 29, 8, 5, 30)
            .single()
            .unwrap()
            .timestamp_millis() as u64;

        queue
            .upsert_schedule(QueueScheduleManifest {
                id: "daily_brief".to_string(),
                cron: "* * * * *".to_string(),
                zone: "UTC".to_string(),
                task: "daily_brief".to_string(),
                payload: serde_json::json!({"user": "u1"}),
                max_retries: 2,
                budget_usd: 0.5,
                effect_summary: Some("llm+email".to_string()),
                replay_key_prefix: Some("schedule:daily_brief".to_string()),
                missed_policy: ScheduleMissedPolicy::FireOnceOnRecovery,
                last_checked_ms: start,
                last_fire_ms: None,
                created_ms: start,
                updated_ms: start,
            })
            .unwrap();

        let report = queue.recover_schedules_at(now, 100).unwrap();
        assert_eq!(report.scanned, 1);
        assert_eq!(report.enqueued, 1);
        assert_eq!(report.recoveries.len(), 1);
        assert_eq!(report.recoveries[0].action, "enqueued");

        let jobs = queue.list().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].task, "daily_brief");
        assert_eq!(jobs[0].payload["schedule_id"], "daily_brief");
        assert_eq!(
            jobs[0].replay_key.as_deref(),
            Some(format!("schedule:daily_brief:{}", report.recoveries[0].fire_ms).as_str())
        );

        let duplicate = queue.recover_schedules_at(now, 100).unwrap();
        assert_eq!(duplicate.enqueued, 0);
        assert_eq!(queue.list().unwrap().len(), 1);
    }

    #[test]
    fn durable_scheduler_enqueues_all_bounded_and_skips_by_policy() {
        let queue = DurableQueueRuntime::open_in_memory().unwrap();
        let start = Utc
            .with_ymd_and_hms(2026, 4, 29, 8, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis() as u64;
        let now = Utc
            .with_ymd_and_hms(2026, 4, 29, 8, 5, 0)
            .single()
            .unwrap()
            .timestamp_millis() as u64;

        for (id, policy) in [
            ("all", ScheduleMissedPolicy::EnqueueAllBounded),
            ("skip", ScheduleMissedPolicy::SkipMissed),
        ] {
            queue
                .upsert_schedule(QueueScheduleManifest {
                    id: id.to_string(),
                    cron: "* * * * *".to_string(),
                    zone: "UTC".to_string(),
                    task: format!("{id}_task"),
                    payload: serde_json::json!({}),
                    max_retries: 1,
                    budget_usd: 0.0,
                    effect_summary: None,
                    replay_key_prefix: Some(format!("schedule:{id}")),
                    missed_policy: policy,
                    last_checked_ms: start,
                    last_fire_ms: None,
                    created_ms: start,
                    updated_ms: start,
                })
                .unwrap();
        }

        let report = queue.recover_schedules_at(now, 2).unwrap();
        assert_eq!(report.scanned, 2);
        assert_eq!(report.enqueued, 2);
        assert_eq!(report.skipped, 2);
        let jobs = queue.list().unwrap();
        assert_eq!(jobs.len(), 2);
        assert!(jobs.iter().all(|job| job.task == "all_task"));
    }

    #[test]
    fn durable_queue_records_retry_wait_and_dead_letter() {
        let queue = DurableQueueRuntime::open_in_memory().unwrap();
        let job = queue
            .enqueue(
                "send_email",
                serde_json::json!({"draft": "d1"}),
                1,
                1.0,
                Some("llm+email".to_string()),
                Some("replay:email".to_string()),
            )
            .unwrap();

        let retry = queue
            .run_one_failed("provider_timeout", "sha256:failure-1", 1000)
            .unwrap()
            .expect("retry job");
        assert_eq!(retry.id, job.id);
        assert_eq!(retry.status, QueueJobStatus::RetryWait);
        assert_eq!(retry.attempts, 1);
        assert_eq!(retry.failure_kind.as_deref(), Some("provider_timeout"));
        assert_eq!(
            retry.failure_fingerprint.as_deref(),
            Some("sha256:failure-1")
        );
        assert!(retry.next_run_ms.is_some());

        let stored = queue.get(&job.id).unwrap().unwrap();
        assert_eq!(stored.status, QueueJobStatus::RetryWait);
        assert!(
            queue.run_one().unwrap().is_none(),
            "backoff should delay retry"
        );

        let terminal = queue
            .enqueue(
                "send_email",
                serde_json::json!({"draft": "d2"}),
                0,
                1.0,
                Some("llm+email".to_string()),
                Some("replay:email:dead".to_string()),
            )
            .unwrap();
        let dead = queue
            .run_one_failed("provider_timeout", "sha256:failure-2", 0)
            .unwrap()
            .expect("dead-letter job");
        assert_eq!(dead.id, terminal.id);
        assert_eq!(dead.status, QueueJobStatus::DeadLettered);
        assert_eq!(dead.attempts, 1);
        assert_eq!(
            dead.failure_fingerprint.as_deref(),
            Some("sha256:failure-2")
        );
        assert!(dead.next_run_ms.is_none());
        let dlq = queue.dead_lettered().unwrap();
        assert_eq!(dlq.len(), 1);
        assert_eq!(dlq[0].id, terminal.id);
    }
}
