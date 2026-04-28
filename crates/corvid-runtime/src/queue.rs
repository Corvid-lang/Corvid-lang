use crate::errors::RuntimeError;
use crate::tracing::now_ms;
use rusqlite::{params, Connection};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueJobStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Canceled,
}

impl QueueJobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct QueueJob {
    pub id: String,
    pub task: String,
    pub payload: Value,
    pub status: QueueJobStatus,
    pub attempts: u64,
    pub max_retries: u64,
    pub budget_usd: f64,
    pub effect_summary: Option<String>,
    pub replay_key: Option<String>,
    pub created_ms: u64,
    pub updated_ms: u64,
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
        let task = task.into();
        if task.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue task name must not be empty".to_string(),
            ));
        }
        let now = now_ms();
        let id = format!(
            "job_{}",
            self.next_id.fetch_add(1, Ordering::Relaxed).saturating_add(1)
        );
        let job = QueueJob {
            id: id.clone(),
            task,
            payload,
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
        let task = task.into();
        if task.trim().is_empty() {
            return Err(RuntimeError::Other(
                "std.queue task name must not be empty".to_string(),
            ));
        }
        let now = now_ms();
        let id = format!(
            "job_{}",
            self.next_id.fetch_add(1, Ordering::Relaxed).saturating_add(1)
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
            status: QueueJobStatus::Pending,
            attempts: 0,
            max_retries,
            budget_usd,
            effect_summary,
            replay_key,
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
                 (id, task, payload_json, status, attempts, max_retries, budget_usd, effect_summary, replay_key, created_ms, updated_ms)
                 values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    job.id,
                    job.task,
                    payload_json,
                    job.status.as_str(),
                    job.attempts as i64,
                    job.max_retries as i64,
                    job.budget_usd,
                    job.effect_summary,
                    job.replay_key,
                    job.created_ms as i64,
                    job.updated_ms as i64,
                ],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to insert durable queue job: {err}")))?;
        Ok(job)
    }

    pub fn get(&self, id: &str) -> Result<Option<QueueJob>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select id, task, payload_json, status, attempts, max_retries, budget_usd,
                        effect_summary, replay_key, created_ms, updated_ms
                 from queue_jobs where id = ?1",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to prepare durable queue read: {err}")))?;
        let mut rows = stmt
            .query(params![id])
            .map_err(|err| RuntimeError::Other(format!("failed to query durable queue job: {err}")))?;
        if let Some(row) = rows
            .next()
            .map_err(|err| RuntimeError::Other(format!("failed to read durable queue row: {err}")))? {
            Ok(Some(
                read_job_row(row)
                    .map_err(|err| RuntimeError::Other(format!("failed to decode durable queue job: {err}")))?,
            ))
        } else {
            Ok(None)
        }
    }

    pub fn list(&self) -> Result<Vec<QueueJob>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select id, task, payload_json, status, attempts, max_retries, budget_usd,
                        effect_summary, replay_key, created_ms, updated_ms
                 from queue_jobs order by created_ms, id",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to prepare durable queue list: {err}")))?;
        let rows = stmt
            .query_map([], read_job_row)
            .map_err(|err| RuntimeError::Other(format!("failed to list durable queue jobs: {err}")))?;
        let mut jobs = Vec::new();
        for row in rows {
            jobs.push(
                row.map_err(|err| RuntimeError::Other(format!("failed to decode durable queue job: {err}")))?,
            );
        }
        Ok(jobs)
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
            .map_err(|err| RuntimeError::Other(format!("failed to cancel durable queue job: {err}")))?;
        if updated == 0 {
            return Err(RuntimeError::Other(format!("std.queue job `{id}` not found")));
        }
        self.get(id)?
            .ok_or_else(|| RuntimeError::Other(format!("std.queue job `{id}` not found")))
    }

    pub fn run_one(&self) -> Result<Option<QueueJob>, RuntimeError> {
        let pending = self
            .list()?
            .into_iter()
            .find(|job| job.status == QueueJobStatus::Pending);
        let Some(mut job) = pending else {
            return Ok(None);
        };
        let now = now_ms();
        job.status = QueueJobStatus::Succeeded;
        job.attempts = job.attempts.saturating_add(1);
        job.updated_ms = now;
        self.conn
            .lock()
            .unwrap()
            .execute(
                "update queue_jobs set status = ?2, attempts = ?3, updated_ms = ?4 where id = ?1 and status = 'pending'",
                params![job.id, job.status.as_str(), job.attempts as i64, job.updated_ms as i64],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to run durable queue job: {err}")))?;
        self.get(&job.id)?
            .map(Some)
            .ok_or_else(|| RuntimeError::Other(format!("std.queue job `{}` not found", job.id)))
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
                    status text not null,
                    attempts integer not null,
                    max_retries integer not null,
                    budget_usd real not null,
                    effect_summary text,
                    replay_key text,
                    created_ms integer not null,
                    updated_ms integer not null
                );
                create index if not exists queue_jobs_status on queue_jobs(status);
                create index if not exists queue_jobs_replay_key on queue_jobs(replay_key);",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to initialize durable queue: {err}")))?;
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
    let status: String = row.get(3)?;
    let payload_json: String = row.get(2)?;
    Ok(QueueJob {
        id: row.get(0)?,
        task: row.get(1)?,
        payload: serde_json::from_str(&payload_json).unwrap_or(Value::Null),
        status: parse_status(&status),
        attempts: row.get::<_, i64>(4)? as u64,
        max_retries: row.get::<_, i64>(5)? as u64,
        budget_usd: row.get(6)?,
        effect_summary: row.get(7)?,
        replay_key: row.get(8)?,
        created_ms: row.get::<_, i64>(9)? as u64,
        updated_ms: row.get::<_, i64>(10)? as u64,
    })
}

fn parse_status(status: &str) -> QueueJobStatus {
    match status {
        "running" => QueueJobStatus::Running,
        "succeeded" => QueueJobStatus::Succeeded,
        "failed" => QueueJobStatus::Failed,
        "canceled" => QueueJobStatus::Canceled,
        _ => QueueJobStatus::Pending,
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
        assert_eq!(queue.get(&job.id).unwrap().replay_key.as_deref(), Some("trace:1"));

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
            .enqueue(
                "daily_brief",
                serde_json::json!({"user": "u1"}),
                2,
                0.5,
                Some("llm+db".to_string()),
                Some("replay:job".to_string()),
            )
            .unwrap();

        let ran = queue.run_one().unwrap().expect("pending job");
        assert_eq!(ran.id, job.id);
        assert_eq!(ran.status, QueueJobStatus::Succeeded);
        assert_eq!(ran.attempts, 1);

        let fetched = queue.get(&job.id).unwrap().unwrap();
        assert_eq!(fetched.status, QueueJobStatus::Succeeded);
        assert_eq!(fetched.attempts, 1);
        assert!(queue.run_one().unwrap().is_none());
    }
}
