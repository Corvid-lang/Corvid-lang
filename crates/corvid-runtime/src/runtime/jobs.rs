//! Durable-job queue dispatch methods on `Runtime` — `enqueue_job`
//! and `cancel_job`. Each delegates to the `QueueRuntime`
//! collaborator and emits a `std.queue.*` host event for the
//! resulting state transition. The queue itself (lifecycle,
//! lease, retry, approval, loop limits) lives in `crate::queue`;
//! the methods here are the thin trace-emitting Runtime surface.

use crate::errors::RuntimeError;
use crate::queue::QueueJob;

use super::Runtime;

impl Runtime {
    pub fn enqueue_job(
        &self,
        task: impl Into<String>,
        payload: serde_json::Value,
        max_retries: u64,
        budget_usd: f64,
        effect_summary: Option<String>,
        replay_key: Option<String>,
    ) -> Result<QueueJob, RuntimeError> {
        let job = self.queue.enqueue(
            task,
            payload,
            max_retries,
            budget_usd,
            effect_summary,
            replay_key,
        )?;
        self.emit_host_event(
            "std.queue.enqueue",
            serde_json::json!({
                "id": job.id,
                "task": job.task,
                "status": job.status.as_str(),
                "max_retries": job.max_retries,
                "budget_usd": job.budget_usd,
                "effect_summary": job.effect_summary,
                "replay_key": job.replay_key,
            }),
        );
        Ok(job)
    }

    pub fn cancel_job(&self, id: &str) -> Result<QueueJob, RuntimeError> {
        let job = self.queue.cancel(id)?;
        self.emit_host_event(
            "std.queue.cancel",
            serde_json::json!({
                "id": job.id,
                "task": job.task,
                "status": job.status.as_str(),
            }),
        );
        Ok(job)
    }
}
