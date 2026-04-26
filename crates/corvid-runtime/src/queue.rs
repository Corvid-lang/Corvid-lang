use crate::errors::RuntimeError;
use crate::tracing::now_ms;
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
}
