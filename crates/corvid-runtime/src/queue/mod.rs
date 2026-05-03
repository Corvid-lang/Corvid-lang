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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

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
    fn personal_executive_agent_jobs_survive_restart_without_duplicate_ai_spend() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("executive-agent-jobs.sqlite");
        let start = Utc
            .with_ymd_and_hms(2026, 4, 29, 9, 50, 0)
            .single()
            .unwrap()
            .timestamp_millis() as u64;
        let now = Utc
            .with_ymd_and_hms(2026, 4, 29, 12, 16, 0)
            .single()
            .unwrap()
            .timestamp_millis() as u64;
        let expected_tasks = [
            "daily_brief_job",
            "meeting_prep_job",
            "email_triage_job",
            "follow_up_job",
        ];
        let job_ids = {
            let queue = DurableQueueRuntime::open(&path).unwrap();
            for (id, cron, task, payload, effects, approval) in [
                (
                    "daily_brief",
                    "0 7 * * 1-5",
                    "daily_brief_job",
                    serde_json::json!({"user_id": "active_users", "day": "business_day"}),
                    "inbox_read,calendar_read,executive_llm",
                    false,
                ),
                (
                    "meeting_prep",
                    "0 6 * * 1-5",
                    "meeting_prep_job",
                    serde_json::json!({"user_id": "active_users", "day": "business_day"}),
                    "inbox_read,calendar_read,executive_llm",
                    false,
                ),
                (
                    "email_triage",
                    "*/10 8-18 * * 1-5",
                    "email_triage_job",
                    serde_json::json!({"user_id": "active_users", "inbox_window": "workday_window"}),
                    "inbox_read,executive_llm,task_write",
                    false,
                ),
                (
                    "follow_up",
                    "*/15 8-18 * * 1-5",
                    "follow_up_job",
                    serde_json::json!({"user_id": "active_users", "thread_id": "open_threads"}),
                    "inbox_read,executive_llm,send_email,task_write,approval:SendExecutiveFollowUp",
                    true,
                ),
            ] {
                queue
                    .upsert_schedule(QueueScheduleManifest {
                        id: id.to_string(),
                        cron: cron.to_string(),
                        zone: "America/New_York".to_string(),
                        task: task.to_string(),
                        payload: serde_json::json!({
                            "contract": {
                                "queue": "personal_executive_agent",
                                "idempotency_key": format!("{id}:active_users:business_window"),
                                "replay_key": format!("executive:{id}:active_users:business_window"),
                                "approval_required": approval
                            },
                            "input": payload
                        }),
                        max_retries: 5,
                        budget_usd: 0.75,
                        effect_summary: Some(effects.to_string()),
                        replay_key_prefix: Some(format!("schedule:personal_executive_agent:{id}")),
                        missed_policy: ScheduleMissedPolicy::FireOnceOnRecovery,
                        last_checked_ms: start,
                        last_fire_ms: None,
                        created_ms: start,
                        updated_ms: start,
                    })
                    .unwrap();
            }

            let report = queue.recover_schedules_at(now, 16).unwrap();
            assert_eq!(report.scanned, 4);
            assert_eq!(report.enqueued, 4);
            assert_eq!(report.skipped, 0);

            let mut jobs = queue.list().unwrap();
            jobs.sort_by(|left, right| left.task.cmp(&right.task));
            assert_eq!(jobs.len(), 4);
            for task in expected_tasks {
                assert!(jobs.iter().any(|job| job.task == task), "missing {task}");
            }
            for job in &jobs {
                assert_eq!(job.budget_usd, 0.75);
                assert_eq!(job.max_retries, 5);
                assert!(job
                    .replay_key
                    .as_deref()
                    .unwrap_or_default()
                    .starts_with("schedule:personal_executive_agent:"));
            }
            jobs.into_iter().map(|job| job.id).collect::<Vec<_>>()
        };

        let mut follow_up_id = String::new();
        {
            let queue = DurableQueueRuntime::open(&path).unwrap();
            for offset in 0..job_ids.len() {
                let leased = queue
                    .lease_next_at("executive-worker", 60_000, now + offset as u64)
                    .unwrap()
                    .expect("lease recovered executive job");
                queue
                    .set_loop_limits(&leased.id, Some(8), Some(120_000), Some(0.75), Some(5))
                    .unwrap();
                queue
                    .record_checkpoint(
                        &leased.id,
                        JobCheckpointKind::AgentStep,
                        "contract.loaded",
                        serde_json::json!({"task": leased.task, "budget_usd": leased.budget_usd}),
                        Some(format!("sha256:{}:contract", leased.task)),
                    )
                    .unwrap();
                queue
                    .record_checkpoint(
                        &leased.id,
                        JobCheckpointKind::ToolResult,
                        "workspace.context",
                        serde_json::json!({"redacted": true, "source_count": 2}),
                        Some(format!("sha256:{}:context", leased.task)),
                    )
                    .unwrap();
                queue
                    .record_checkpoint(
                        &leased.id,
                        JobCheckpointKind::ToolResult,
                        "llm.complete",
                        serde_json::json!({"model": "executive-safe", "tokens": 640, "cost_usd": 0.18}),
                        Some(format!("sha256:{}:llm", leased.task)),
                    )
                    .unwrap();
                queue
                    .record_checkpoint(
                        &leased.id,
                        JobCheckpointKind::PartialOutput,
                        "redacted.output",
                        serde_json::json!({"fingerprint": format!("sha256:{}:output", leased.task)}),
                        Some(format!("sha256:{}:output", leased.task)),
                    )
                    .unwrap();
                let usage = queue
                    .record_loop_usage_at(
                        &leased.id,
                        3,
                        30_000,
                        0.18,
                        2,
                        "executive-worker",
                        now + 1_000 + offset as u64,
                    )
                    .unwrap();
                assert!(usage.violated_bounds.is_empty());

                if leased.task == "follow_up_job" {
                    follow_up_id = leased.id.clone();
                    let waiting = queue
                        .enter_approval_wait(
                            &leased.id,
                            "executive-worker",
                            "approval:SendExecutiveFollowUp:open_threads",
                            now + 3_600_000,
                            "send executive follow-up email",
                        )
                        .unwrap();
                    assert_eq!(waiting.status, QueueJobStatus::ApprovalWait);
                } else {
                    let completed = queue
                        .complete_leased(
                            &leased.id,
                            "executive-worker",
                            Some(format!("{}Output", leased.task)),
                            Some(format!("sha256:{}:output", leased.task)),
                        )
                        .unwrap();
                    assert_eq!(completed.status, QueueJobStatus::Succeeded);
                }
            }
        }
        assert!(
            !follow_up_id.is_empty(),
            "follow-up job should require approval"
        );

        let queue = DurableQueueRuntime::open(&path).unwrap();
        let duplicate_recovery = queue.recover_schedules_at(now, 16).unwrap();
        assert_eq!(duplicate_recovery.enqueued, 0);
        assert_eq!(queue.list().unwrap().len(), 4);

        for job_id in &job_ids {
            let resume = queue.resume_state(job_id).unwrap();
            assert_eq!(resume.checkpoints.len(), 4);
            assert_eq!(resume.next_sequence, 5);
            let llm_spend_count = resume
                .checkpoints
                .iter()
                .filter(|checkpoint| checkpoint.label == "llm.complete")
                .count();
            assert_eq!(llm_spend_count, 1, "job {job_id} re-spent LLM work");
        }

        let waiting = queue.resume_state(&follow_up_id).unwrap();
        assert_eq!(waiting.job.status, QueueJobStatus::ApprovalWait);
        assert_eq!(
            waiting.job.approval_id.as_deref(),
            Some("approval:SendExecutiveFollowUp:open_threads")
        );

        let approved = queue
            .decide_approval_wait_at(
                &follow_up_id,
                "approval:SendExecutiveFollowUp:open_threads",
                JobApprovalDecision::Approve,
                "human:executive",
                Some("approved after restart".to_string()),
                now + 10_000,
            )
            .unwrap();
        assert_eq!(approved.status, QueueJobStatus::Pending);
        let leased = queue
            .lease_next_at("executive-worker-2", 60_000, now + 10_001)
            .unwrap()
            .expect("approved follow-up should resume");
        assert_eq!(leased.id, follow_up_id);
        let completed = queue
            .complete_leased(
                &follow_up_id,
                "executive-worker-2",
                Some("follow_up_jobOutput".to_string()),
                Some("sha256:follow_up_job:output".to_string()),
            )
            .unwrap();
        assert_eq!(completed.status, QueueJobStatus::Succeeded);

        let final_resume = queue.resume_state(&follow_up_id).unwrap();
        assert_eq!(final_resume.checkpoints.len(), 4);
        assert_eq!(
            final_resume
                .checkpoints
                .iter()
                .filter(|checkpoint| checkpoint.label == "llm.complete")
                .count(),
            1
        );
        let audit = queue.job_audit_events(&follow_up_id).unwrap();
        assert!(audit
            .iter()
            .any(|event| event.event_kind == "approval_approve"));
    }
}
