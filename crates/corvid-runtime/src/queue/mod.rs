use crate::errors::RuntimeError;
use crate::tracing::now_ms;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub mod model;
pub use model::*;

mod approvals;
mod audit;
mod checkpoints;
mod durable;
mod enqueue;
mod leases;
mod loops;
mod parsers;
mod schedule;
mod schedules;
mod sqlite_init;
use audit::insert_job_audit_event;
use parsers::*;
use schedule::*;

#[cfg(test)]
#[path = "tests/durable_basics.rs"]
mod durable_basics_tests;

#[cfg(test)]
#[path = "tests/leases.rs"]
mod leases_tests;

#[cfg(test)]
#[path = "tests/checkpoints.rs"]
mod checkpoints_tests;

#[cfg(test)]
#[path = "tests/approvals.rs"]
mod approvals_tests;

#[cfg(test)]
#[path = "tests/loops.rs"]
mod loops_tests;

#[cfg(test)]
#[path = "tests/scheduler.rs"]
mod scheduler_tests;

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
