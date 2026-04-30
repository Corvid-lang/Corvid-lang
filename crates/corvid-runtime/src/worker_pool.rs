//! Multi-worker durable-queue runner — slice 38K.
//!
//! Phase 38 originally shipped only `DurableQueueRuntime::run_one_*`,
//! a single-shot path that leases one job, runs an executor
//! closure, completes/fails it, and returns. The audit flagged
//! that as "no multi-worker job runner" — there was no way to
//! scale concurrency past one without an external scheduler.
//!
//! 38K introduces `WorkerPool`: a configurable async worker pool
//! that runs N tokio tasks, each contesting the durable queue's
//! lease lock. The pool guarantees lease exclusivity (the queue's
//! existing `lease_next_at` UPDATE handles that under the hood),
//! supports graceful drain (signal → workers stop pulling new
//! work, in-flight jobs complete, then exit), and surfaces a
//! shared completion counter the test corpus consults to prove
//! "exactly one of N workers ran each job."
//!
//! The pool does NOT know how to *execute* arbitrary task logic —
//! that is application-specific. The caller supplies a
//! `JobExecutor` closure that turns a `QueueJob` into a
//! `JobOutcome` (success / failure / retry-with-delay). The
//! shipped CLI wraps a no-op executor for smoke tests; production
//! callers wire their own executor via `WorkerPool::with_executor`.

use crate::errors::RuntimeError;
use crate::queue::{DurableQueueRuntime, QueueJob};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::sleep;

/// Outcome the executor reports for a single leased job. The
/// pool decides whether to call `complete_leased` (Success),
/// `fail_leased` (Failure / RetryAfter), or release the lease
/// without persisting (Skip — used when the executor decides the
/// job is not its responsibility, e.g. wrong task name in a
/// per-task pool).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobOutcome {
    Success {
        output_kind: Option<String>,
        output_fingerprint: Option<String>,
    },
    Failure {
        failure_kind: String,
        failure_fingerprint: String,
        base_delay_ms: u64,
    },
    Skip,
}

impl JobOutcome {
    pub fn success() -> Self {
        Self::Success {
            output_kind: None,
            output_fingerprint: None,
        }
    }
}

/// Synchronous executor signature. The pool runs the executor on
/// `tokio::task::spawn_blocking` so a synchronous LLM/HTTP call
/// inside it does not block the tokio reactor. Production
/// executors that are async themselves can do the work on a
/// dedicated runtime; the pool's contract is sync-call.
pub type JobExecutor =
    Arc<dyn Fn(&QueueJob) -> Result<JobOutcome, RuntimeError> + Send + Sync>;

/// Multi-worker pool over a `DurableQueueRuntime`. Each worker is
/// a tokio task that loops:
///
///   1. Try `lease_next_at(worker_id, lease_ttl_ms, now_ms())`.
///   2. If a job came back, run the executor; finalise via
///      `complete_leased` / `fail_leased` / release.
///   3. If no job or pool is paused, sleep `idle_poll_ms` and
///      retry until the drain flag flips.
pub struct WorkerPool {
    queue: Arc<DurableQueueRuntime>,
    executor: JobExecutor,
    workers: usize,
    lease_ttl_ms: u64,
    idle_poll_ms: u64,
    drain: Arc<AtomicBool>,
    counter_succeeded: Arc<AtomicU64>,
    counter_failed: Arc<AtomicU64>,
    counter_skipped: Arc<AtomicU64>,
}

impl WorkerPool {
    /// Construct a pool with `workers` async tasks contesting the
    /// queue. Default lease TTL = 60s; default idle poll = 100ms.
    pub fn new(queue: Arc<DurableQueueRuntime>, workers: usize) -> Self {
        // The default executor is a no-op that succeeds every job
        // — useful for smoke testing the pool semantics without
        // wiring a real LLM/tool layer. Production callers
        // override via `with_executor`.
        let default_executor: JobExecutor =
            Arc::new(|_job| Ok(JobOutcome::success()));
        Self {
            queue,
            executor: default_executor,
            workers: workers.max(1),
            lease_ttl_ms: 60_000,
            idle_poll_ms: 100,
            drain: Arc::new(AtomicBool::new(false)),
            counter_succeeded: Arc::new(AtomicU64::new(0)),
            counter_failed: Arc::new(AtomicU64::new(0)),
            counter_skipped: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn with_executor(mut self, executor: JobExecutor) -> Self {
        self.executor = executor;
        self
    }

    pub fn with_lease_ttl_ms(mut self, lease_ttl_ms: u64) -> Self {
        self.lease_ttl_ms = lease_ttl_ms;
        self
    }

    pub fn with_idle_poll_ms(mut self, idle_poll_ms: u64) -> Self {
        self.idle_poll_ms = idle_poll_ms;
        self
    }

    /// Pre-attached drain handle for tests + CLI. Tests can flip
    /// this to true to signal graceful shutdown without owning
    /// the pool itself.
    pub fn drain_handle(&self) -> Arc<AtomicBool> {
        self.drain.clone()
    }

    /// Counters the test corpus + the CLI consult to assert
    /// per-job outcomes after `join_all` returns. The values are
    /// stable post-drain.
    pub fn counters(&self) -> WorkerPoolCounters {
        WorkerPoolCounters {
            succeeded: self.counter_succeeded.clone(),
            failed: self.counter_failed.clone(),
            skipped: self.counter_skipped.clone(),
        }
    }

    /// Spawn the worker tasks and return their handles. The
    /// caller `join_all`s them after flipping the drain flag.
    pub fn spawn(&self) -> Vec<JoinHandle<()>> {
        let mut handles = Vec::with_capacity(self.workers);
        for worker_index in 0..self.workers {
            let worker_id = format!("worker-{worker_index}");
            let queue = self.queue.clone();
            let executor = self.executor.clone();
            let drain = self.drain.clone();
            let lease_ttl_ms = self.lease_ttl_ms;
            let idle_poll_ms = self.idle_poll_ms;
            let succeeded = self.counter_succeeded.clone();
            let failed = self.counter_failed.clone();
            let skipped = self.counter_skipped.clone();
            handles.push(tokio::spawn(async move {
                worker_loop(
                    worker_id,
                    queue,
                    executor,
                    drain,
                    lease_ttl_ms,
                    idle_poll_ms,
                    succeeded,
                    failed,
                    skipped,
                )
                .await;
            }));
        }
        handles
    }

    /// Convenience: spawn workers, wait for drain, return when
    /// every worker has stopped. Tests call this after seeding
    /// the queue + flipping drain on the handle.
    pub async fn run_until_drained(self) {
        let handles = self.spawn();
        for handle in handles {
            let _ = handle.await;
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkerPoolCounters {
    pub succeeded: Arc<AtomicU64>,
    pub failed: Arc<AtomicU64>,
    pub skipped: Arc<AtomicU64>,
}

impl WorkerPoolCounters {
    pub fn succeeded(&self) -> u64 {
        self.succeeded.load(Ordering::SeqCst)
    }
    pub fn failed(&self) -> u64 {
        self.failed.load(Ordering::SeqCst)
    }
    pub fn skipped(&self) -> u64 {
        self.skipped.load(Ordering::SeqCst)
    }
    pub fn total(&self) -> u64 {
        self.succeeded() + self.failed() + self.skipped()
    }
}

#[allow(clippy::too_many_arguments)]
async fn worker_loop(
    worker_id: String,
    queue: Arc<DurableQueueRuntime>,
    executor: JobExecutor,
    drain: Arc<AtomicBool>,
    lease_ttl_ms: u64,
    idle_poll_ms: u64,
    succeeded: Arc<AtomicU64>,
    failed: Arc<AtomicU64>,
    skipped: Arc<AtomicU64>,
) {
    loop {
        if drain.load(Ordering::SeqCst) {
            return;
        }
        let lease = {
            let queue = queue.clone();
            let worker_id = worker_id.clone();
            tokio::task::spawn_blocking(move || queue.lease_next(&worker_id, lease_ttl_ms))
                .await
                .unwrap_or_else(|_| {
                    Err(RuntimeError::Other(
                        "lease_next worker task panicked".to_string(),
                    ))
                })
        };
        let job = match lease {
            Ok(Some(job)) => job,
            Ok(None) => {
                // No work; sleep and retry.
                if drain.load(Ordering::SeqCst) {
                    return;
                }
                sleep(Duration::from_millis(idle_poll_ms)).await;
                continue;
            }
            Err(_) => {
                // Transient error — back off and try again.
                sleep(Duration::from_millis(idle_poll_ms.max(50))).await;
                continue;
            }
        };

        // Run the executor on a blocking thread so a synchronous
        // long-running tool/LLM call does not block the reactor.
        let exec = executor.clone();
        let job_for_exec = job.clone();
        let outcome = tokio::task::spawn_blocking(move || (exec)(&job_for_exec))
            .await
            .unwrap_or_else(|_| {
                Err(RuntimeError::Other(format!(
                    "executor panicked on job {}",
                    job.id
                )))
            });

        let queue_for_finalise = queue.clone();
        let worker_id_for_finalise = worker_id.clone();
        let job_id = job.id.clone();
        let finalise = tokio::task::spawn_blocking(move || match outcome {
            Ok(JobOutcome::Success {
                output_kind,
                output_fingerprint,
            }) => {
                queue_for_finalise
                    .complete_leased(&job_id, &worker_id_for_finalise, output_kind, output_fingerprint)
                    .map(|_| WorkerOutcome::Succeeded)
            }
            Ok(JobOutcome::Failure {
                failure_kind,
                failure_fingerprint,
                base_delay_ms,
            }) => queue_for_finalise
                .fail_leased(
                    &job_id,
                    &worker_id_for_finalise,
                    failure_kind,
                    failure_fingerprint,
                    base_delay_ms,
                )
                .map(|_| WorkerOutcome::Failed),
            Ok(JobOutcome::Skip) => Ok(WorkerOutcome::Skipped),
            Err(err) => Err(err),
        })
        .await
        .unwrap_or_else(|_| {
            Err(RuntimeError::Other(
                "finalise task panicked".to_string(),
            ))
        });
        match finalise {
            Ok(WorkerOutcome::Succeeded) => {
                succeeded.fetch_add(1, Ordering::SeqCst);
            }
            Ok(WorkerOutcome::Failed) => {
                failed.fetch_add(1, Ordering::SeqCst);
            }
            Ok(WorkerOutcome::Skipped) => {
                skipped.fetch_add(1, Ordering::SeqCst);
            }
            Err(_) => {
                // Lease will expire on its own; the next worker
                // re-leases. Counted as failed for visibility.
                failed.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum WorkerOutcome {
    Succeeded,
    Failed,
    Skipped,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::DurableQueueRuntime;
    use serde_json::json;
    use std::collections::BTreeSet;
    use std::sync::Mutex;

    fn open_queue(path: &std::path::Path) -> Arc<DurableQueueRuntime> {
        Arc::new(DurableQueueRuntime::open(path).unwrap())
    }

    /// Slice 38K positive: a 4-worker pool over a 16-job queue
    /// drains every job exactly once. The shared `seen_ids`
    /// dictionary captures each (worker, job_id) pair the
    /// executor touched; we assert no job id appears twice.
    #[tokio::test]
    async fn t38k_pool_runs_each_job_exactly_once() {
        let dir = tempfile::tempdir().unwrap();
        let queue = open_queue(&dir.path().join("queue.db"));
        // Seed 16 jobs.
        let mut expected_ids = BTreeSet::new();
        for i in 0..16 {
            let job = queue
                .enqueue_typed(
                    "noop",
                    json!({"i": i}),
                    None,
                    1,
                    0.0,
                    Some("test".to_string()),
                    Some(format!("rk:{i}")),
                )
                .unwrap();
            expected_ids.insert(job.id);
        }

        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_ref = seen.clone();
        let executor: JobExecutor = Arc::new(move |job| {
            seen_ref.lock().unwrap().push(job.id.clone());
            Ok(JobOutcome::success())
        });

        let pool = WorkerPool::new(queue.clone(), 4)
            .with_executor(executor)
            .with_lease_ttl_ms(5_000)
            .with_idle_poll_ms(10);
        let drain = pool.drain_handle();
        let counters = pool.counters();

        let handles = pool.spawn();
        // Wait until all 16 succeed, then drain.
        let timeout_ms = 5_000;
        let start = std::time::Instant::now();
        loop {
            if counters.succeeded() >= 16 {
                break;
            }
            if start.elapsed().as_millis() as u64 > timeout_ms {
                panic!(
                    "timed out waiting for 16 successes (succeeded={}, failed={}, skipped={})",
                    counters.succeeded(),
                    counters.failed(),
                    counters.skipped()
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        drain.store(true, Ordering::SeqCst);
        for h in handles {
            let _ = h.await;
        }

        // Each enqueued job ran exactly once.
        let observed: Vec<String> = seen.lock().unwrap().clone();
        let observed_set: BTreeSet<&String> = observed.iter().collect();
        assert_eq!(
            observed.len(),
            observed_set.len(),
            "no job ran twice: {observed:?}"
        );
        assert_eq!(observed.len(), 16);
        let observed_ids: BTreeSet<String> = observed.into_iter().collect();
        assert_eq!(observed_ids, expected_ids);
        assert_eq!(counters.succeeded(), 16);
        assert_eq!(counters.failed(), 0);
    }

    /// Slice 38K positive: graceful drain — signal the drain
    /// flag mid-flight, in-flight jobs complete, no new work is
    /// claimed afterward. We seed the queue with more jobs than
    /// can run before the drain signal fires; the surplus must
    /// remain pending.
    #[tokio::test]
    async fn t38k_pool_drains_gracefully_without_claiming_new_work() {
        let dir = tempfile::tempdir().unwrap();
        let queue = open_queue(&dir.path().join("queue.db"));
        // Seed 8 jobs; only the first batch should be claimed
        // before drain fires.
        for i in 0..8 {
            queue
                .enqueue_typed(
                    "noop",
                    json!({"i": i}),
                    None,
                    1,
                    0.0,
                    Some("test".to_string()),
                    Some(format!("rk:{i}")),
                )
                .unwrap();
        }
        // Slow executor: each call sleeps so the drain signal can
        // fire mid-flight.
        let executor: JobExecutor = Arc::new(|_| {
            std::thread::sleep(Duration::from_millis(100));
            Ok(JobOutcome::success())
        });
        let pool = WorkerPool::new(queue.clone(), 2)
            .with_executor(executor)
            .with_lease_ttl_ms(5_000)
            .with_idle_poll_ms(20);
        let drain = pool.drain_handle();
        let counters = pool.counters();
        let handles = pool.spawn();

        // Let workers grab some jobs, then drain.
        tokio::time::sleep(Duration::from_millis(50)).await;
        drain.store(true, Ordering::SeqCst);
        for h in handles {
            let _ = h.await;
        }

        // At least one job ran; not all 8 (drain fired before the
        // queue was empty).
        let succeeded = counters.succeeded();
        assert!(succeeded >= 1, "at least one job ran");
        assert!(
            succeeded < 8,
            "drain stopped further claims: succeeded={succeeded}"
        );

        // Pending jobs survive the drain (status is `pending` or
        // `retry_wait`, not `succeeded`).
        let post_drain_jobs = queue.list().unwrap();
        let pending_after: Vec<&str> = post_drain_jobs
            .iter()
            .filter(|j| {
                matches!(
                    j.status,
                    crate::queue::QueueJobStatus::Pending
                        | crate::queue::QueueJobStatus::RetryWait
                )
            })
            .map(|j| j.id.as_str())
            .collect();
        assert!(
            !pending_after.is_empty(),
            "drain leaves work for the next pool: {post_drain_jobs:?}"
        );
    }

    /// Slice 38K adversarial (T1 lease exclusivity, runtime
    /// edition): two workers contesting the same queue cannot
    /// both lease the same job. We enqueue a single job, run two
    /// workers for a short window, and assert only ONE worker
    /// ever held the job lease.
    #[tokio::test]
    async fn t38k_two_workers_cannot_both_lease_same_job() {
        let dir = tempfile::tempdir().unwrap();
        let queue = open_queue(&dir.path().join("queue.db"));
        queue
            .enqueue_typed(
                "noop",
                json!({"once": true}),
                None,
                1,
                0.0,
                Some("test".to_string()),
                Some("rk:once".to_string()),
            )
            .unwrap();

        let observed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let observed_ref = observed.clone();
        let executor: JobExecutor = Arc::new(move |job| {
            observed_ref.lock().unwrap().push(job.id.clone());
            Ok(JobOutcome::success())
        });

        let pool = WorkerPool::new(queue, 2)
            .with_executor(executor)
            .with_lease_ttl_ms(5_000)
            .with_idle_poll_ms(5);
        let drain = pool.drain_handle();
        let counters = pool.counters();
        let handles = pool.spawn();
        // Wait for the one job to complete, then drain.
        let timeout_ms = 2_000;
        let start = std::time::Instant::now();
        loop {
            if counters.succeeded() >= 1 {
                break;
            }
            if start.elapsed().as_millis() as u64 > timeout_ms {
                panic!("timed out waiting for the one job");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        drain.store(true, Ordering::SeqCst);
        for h in handles {
            let _ = h.await;
        }
        // The observed list contains exactly one entry — the
        // single job ran exactly once across both workers.
        let observed: Vec<String> = observed.lock().unwrap().clone();
        assert_eq!(observed.len(), 1, "exactly one execution: {observed:?}");
        assert_eq!(counters.succeeded(), 1);
    }
}
