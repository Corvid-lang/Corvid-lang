//! `corvid jobs` CLI dispatch — slice 38 / durable jobs surface,
//! decomposed in Phase 20j-A1.
//!
//! Houses every `cmd_jobs_*` dispatch arm: lifecycle
//! (enqueue / run / inspect / retry / cancel / pause / resume /
//! drain / export-trace), durable approvals (wait / decide /
//! audit), loop limits + heartbeats + stall checks, dead-letter
//! queue, schedule manifest CRUD, per-task limit policies, and
//! checkpoint CRUD + resume.
//!
//! All arg-tree types ([`super::cli::jobs`]) and `print_*`
//! formatters ([`super::format`]) are imported from the top-level
//! crate; this module owns the dispatch behaviour only.

use anyhow::{Context, Result};
use corvid_runtime::queue::{DurableQueueRuntime, QueueScheduleManifest};
use std::path::Path;

use crate::cli::jobs::{
    ApprovalDecisionArg, CheckpointKindArg, LimitScopeArg, SchedulePolicyArg, StallActionArg,
};
use crate::format::{
    print_checkpoint_summary, print_job_summary, print_loop_limits, print_loop_usage,
    print_schedule_summary, print_stall_check, print_stall_policy,
};

pub(crate) fn cmd_jobs_enqueue(
    state: &Path,
    task: &str,
    payload: &str,
    input_schema: Option<String>,
    max_retries: u64,
    budget_usd: f64,
    effect_summary: Option<String>,
    replay_key: Option<String>,
    idempotency_key: Option<String>,
    delay_ms: u64,
) -> Result<u8> {
    if let Some(parent) = state
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create jobs state dir `{}`", parent.display()))?;
    }
    let queue = DurableQueueRuntime::open(state)?;
    let payload = serde_json::from_str(payload).context("jobs payload must be valid JSON")?;
    let next_run_ms = if delay_ms == 0 {
        None
    } else {
        Some(corvid_runtime::tracing::now_ms().saturating_add(delay_ms))
    };
    let job = queue.enqueue_typed_idempotent(
        task,
        payload,
        input_schema,
        max_retries,
        budget_usd,
        effect_summary,
        replay_key,
        idempotency_key,
        next_run_ms,
    )?;
    println!("corvid jobs enqueue");
    println!("state: {}", state.display());
    print_job_summary(&job);
    Ok(0)
}

pub(crate) fn cmd_jobs_run_one(
    state: &Path,
    output_kind: Option<String>,
    output_fingerprint: Option<String>,
    fail_kind: Option<String>,
    fail_fingerprint: Option<String>,
    retry_base_ms: u64,
) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    println!("corvid jobs run-one");
    println!("state: {}", state.display());
    let result = if let Some(kind) = fail_kind {
        queue.run_one_failed(
            kind,
            fail_fingerprint.unwrap_or_else(|| "sha256:redacted-failure".to_string()),
            retry_base_ms,
        )?
    } else {
        queue.run_one_with_output(output_kind, output_fingerprint)?
    };
    match result {
        Some(job) => {
            print_job_summary(&job);
            Ok(0)
        }
        None => {
            println!("job: none");
            Ok(0)
        }
    }
}


pub(crate) fn cmd_jobs_run(
    state: &Path,
    workers: usize,
    lease_ttl_ms: u64,
    idle_poll_ms: u64,
    max_runtime_ms: u64,
) -> Result<u8> {
    use corvid_runtime::worker_pool::WorkerPool;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    let queue = Arc::new(DurableQueueRuntime::open(state)?);
    let pool = WorkerPool::new(queue.clone(), workers)
        .with_lease_ttl_ms(lease_ttl_ms)
        .with_idle_poll_ms(idle_poll_ms);
    let drain = pool.drain_handle();
    let counters = pool.counters();
    let drain_for_signal = drain.clone();

    println!("corvid jobs run");
    println!("state: {}", state.display());
    println!("workers: {workers}");
    println!("lease_ttl_ms: {lease_ttl_ms}");
    println!("idle_poll_ms: {idle_poll_ms}");
    if max_runtime_ms > 0 {
        println!("max_runtime_ms: {max_runtime_ms}");
    } else {
        println!("max_runtime_ms: 0 (run until Ctrl-C)");
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        // Best-effort Ctrl-C handler (Unix + Windows): signal
        // drain when received. If signal::ctrl_c is unavailable
        // (e.g. running detached), we still respect
        // max_runtime_ms.
        tokio::spawn(async move {
            if (tokio::signal::ctrl_c().await).is_ok() {
                drain_for_signal.store(true, Ordering::SeqCst);
            }
        });
        let handles = pool.spawn();
        if max_runtime_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(max_runtime_ms)).await;
            drain.store(true, Ordering::SeqCst);
        }
        for h in handles {
            let _ = h.await;
        }
    });

    println!(
        "result: succeeded={} failed={} skipped={} total={}",
        counters.succeeded(),
        counters.failed(),
        counters.skipped(),
        counters.total()
    );
    Ok(0)
}

pub(crate) fn cmd_jobs_inspect(state: &Path, job_id: &str) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    let job = queue
        .get(job_id)?
        .with_context(|| format!("job `{job_id}` not found"))?;
    println!("corvid jobs inspect");
    println!("state: {}", state.display());
    print_job_summary(&job);
    if let Some(usage) = queue.loop_usage(job_id)? {
        print_loop_usage(&usage);
    }
    println!("checkpoint_count: {}", queue.list_checkpoints(job_id)?.len());
    println!("audit_event_count: {}", queue.job_audit_events(job_id)?.len());
    Ok(0)
}

pub(crate) fn cmd_jobs_retry(state: &Path, job_id: &str) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    let job = queue.retry_job(job_id)?;
    println!("corvid jobs retry");
    println!("state: {}", state.display());
    print_job_summary(&job);
    Ok(0)
}

pub(crate) fn cmd_jobs_cancel(state: &Path, job_id: &str) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    let job = queue.cancel(job_id)?;
    println!("corvid jobs cancel");
    println!("state: {}", state.display());
    print_job_summary(&job);
    Ok(0)
}

pub(crate) fn cmd_jobs_pause(state: &Path, reason: Option<&str>) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    queue.set_paused(true, reason)?;
    println!("corvid jobs pause");
    println!("state: {}", state.display());
    println!("paused: true");
    println!("reason: {}", reason.unwrap_or(""));
    Ok(0)
}

pub(crate) fn cmd_jobs_resume(state: &Path) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    queue.set_paused(false, None)?;
    println!("corvid jobs resume");
    println!("state: {}", state.display());
    println!("paused: false");
    Ok(0)
}

pub(crate) fn cmd_jobs_drain(state: &Path, reason: Option<&str>) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    let released = queue.drain_active_leases(reason)?;
    println!("corvid jobs drain");
    println!("state: {}", state.display());
    println!("paused: true");
    println!("released_leases: {released}");
    println!("reason: {}", reason.unwrap_or(""));
    Ok(0)
}

pub(crate) fn cmd_jobs_export_trace(state: &Path, job_id: &str, out: Option<&Path>) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    let job = queue
        .get(job_id)?
        .with_context(|| format!("job `{job_id}` not found"))?;
    let checkpoints = queue.list_checkpoints(job_id)?;
    let audit = queue.job_audit_events(job_id)?;
    let usage = queue.loop_usage(job_id)?;
    let trace = serde_json::json!({
        "schema": "corvid.jobs.trace.v1",
        "job": {
            "id": job.id,
            "task": job.task,
            "status": job.status.as_str(),
            "attempts": job.attempts,
            "max_retries": job.max_retries,
            "budget_usd": job.budget_usd,
            "effect_summary": job.effect_summary,
            "replay_key": job.replay_key,
            "idempotency_key": job.idempotency_key,
            "output_kind": job.output_kind,
            "output_fingerprint": job.output_fingerprint,
            "failure_kind": job.failure_kind,
            "failure_fingerprint": job.failure_fingerprint,
            "approval_id": job.approval_id,
            "approval_expires_ms": job.approval_expires_ms,
            "created_ms": job.created_ms,
            "updated_ms": job.updated_ms
        },
        "checkpoints": checkpoints.into_iter().map(|checkpoint| serde_json::json!({
            "id": checkpoint.id,
            "sequence": checkpoint.sequence,
            "kind": checkpoint.kind.as_str(),
            "label": checkpoint.label,
            "payload_fingerprint": checkpoint.payload_fingerprint,
            "created_ms": checkpoint.created_ms
        })).collect::<Vec<_>>(),
        "audit": audit.into_iter().map(|event| serde_json::json!({
            "id": event.id,
            "event_kind": event.event_kind,
            "actor": event.actor,
            "approval_id": event.approval_id,
            "status_before": event.status_before,
            "status_after": event.status_after,
            "reason": event.reason,
            "created_ms": event.created_ms
        })).collect::<Vec<_>>(),
        "loop_usage": usage.map(|usage| serde_json::json!({
            "steps": usage.steps,
            "wall_ms": usage.wall_ms,
            "spend_usd": usage.spend_usd,
            "tool_calls": usage.tool_calls,
            "updated_ms": usage.updated_ms
        }))
    });
    let rendered = serde_json::to_string_pretty(&trace)?;
    println!("corvid jobs export-trace");
    println!("state: {}", state.display());
    println!("job: {job_id}");
    if let Some(out) = out {
        std::fs::write(out, rendered)
            .with_context(|| format!("failed to write job trace `{}`", out.display()))?;
        println!("out: {}", out.display());
    } else {
        println!("{rendered}");
    }
    Ok(0)
}

pub(crate) fn cmd_jobs_wait_approval(
    state: &Path,
    worker_id: &str,
    lease_ttl_ms: u64,
    approval_id: &str,
    approval_expires_ms: u64,
    approval_reason: &str,
) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    println!("corvid jobs wait-approval");
    println!("state: {}", state.display());
    let Some(job) = queue.lease_next(worker_id, lease_ttl_ms)? else {
        println!("job: none");
        return Ok(0);
    };
    let waiting = queue.enter_approval_wait(
        &job.id,
        worker_id,
        approval_id,
        approval_expires_ms,
        approval_reason,
    )?;
    print_job_summary(&waiting);
    Ok(0)
}

pub(crate) fn cmd_jobs_approvals(state: &Path) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    let jobs = queue.approval_waiting()?;
    println!("corvid jobs approvals");
    println!("state: {}", state.display());
    println!("approval_wait_count: {}", jobs.len());
    for job in jobs {
        print_job_summary(&job);
    }
    Ok(0)
}

pub(crate) fn cmd_jobs_approval_decide(
    state: &Path,
    job_id: &str,
    approval_id: &str,
    decision: ApprovalDecisionArg,
    actor: &str,
    reason: Option<String>,
) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    let job = queue.decide_approval_wait(job_id, approval_id, decision.into(), actor, reason)?;
    println!("corvid jobs approval decide");
    println!("state: {}", state.display());
    println!("decision: {}", decision.as_str());
    print_job_summary(&job);
    Ok(0)
}

pub(crate) fn cmd_jobs_approval_audit(state: &Path, job_id: &str) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    let events = queue.job_audit_events(job_id)?;
    println!("corvid jobs approval audit");
    println!("state: {}", state.display());
    println!("job: {job_id}");
    println!("audit_event_count: {}", events.len());
    for event in events {
        println!("audit: {}", event.id);
        println!("event_kind: {}", event.event_kind);
        println!("actor: {}", event.actor);
        println!(
            "approval_id: {}",
            event.approval_id.as_deref().unwrap_or("")
        );
        println!("status_before: {}", event.status_before);
        println!("status_after: {}", event.status_after);
        println!("reason: {}", event.reason.as_deref().unwrap_or(""));
        println!("created_ms: {}", event.created_ms);
    }
    Ok(0)
}

pub(crate) fn cmd_jobs_loop_limits(
    state: &Path,
    job_id: &str,
    max_steps: Option<u64>,
    max_wall_ms: Option<u64>,
    max_spend_usd: Option<f64>,
    max_tool_calls: Option<u64>,
) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    let limits = queue.set_loop_limits(
        job_id,
        max_steps,
        max_wall_ms,
        max_spend_usd,
        max_tool_calls,
    )?;
    println!("corvid jobs loop limits");
    println!("state: {}", state.display());
    print_loop_limits(&limits);
    Ok(0)
}

pub(crate) fn cmd_jobs_loop_record(
    state: &Path,
    job_id: &str,
    steps: u64,
    wall_ms: u64,
    spend_usd: f64,
    tool_calls: u64,
    actor: &str,
) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    let report = queue.record_loop_usage(job_id, steps, wall_ms, spend_usd, tool_calls, actor)?;
    println!("corvid jobs loop record");
    println!("state: {}", state.display());
    print_loop_usage(&report.usage);
    println!("violated_bound_count: {}", report.violated_bounds.len());
    for bound in report.violated_bounds {
        println!("violated_bound: {bound}");
    }
    if let Some(job) = queue.get(job_id)? {
        println!("status: {}", job.status.as_str());
        println!(
            "failure_kind: {}",
            job.failure_kind.as_deref().unwrap_or("")
        );
        println!(
            "failure_fingerprint: {}",
            job.failure_fingerprint.as_deref().unwrap_or("")
        );
    }
    Ok(0)
}

pub(crate) fn cmd_jobs_loop_usage(state: &Path, job_id: &str) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    println!("corvid jobs loop usage");
    println!("state: {}", state.display());
    if let Some(limits) = queue.loop_limits(job_id)? {
        print_loop_limits(&limits);
    } else {
        println!("limits: none");
    }
    if let Some(usage) = queue.loop_usage(job_id)? {
        print_loop_usage(&usage);
    } else {
        println!("usage: none");
    }
    Ok(0)
}

pub(crate) fn cmd_jobs_loop_heartbeat(
    state: &Path,
    job_id: &str,
    actor: &str,
    message: Option<String>,
) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    let heartbeat = queue.record_loop_heartbeat(job_id, actor, message)?;
    println!("corvid jobs loop heartbeat");
    println!("state: {}", state.display());
    println!("job: {}", heartbeat.job_id);
    println!("actor: {}", heartbeat.actor);
    println!("message: {}", heartbeat.message.as_deref().unwrap_or(""));
    println!("last_heartbeat_ms: {}", heartbeat.last_heartbeat_ms);
    println!("updated_ms: {}", heartbeat.updated_ms);
    Ok(0)
}

pub(crate) fn cmd_jobs_loop_stall_policy(
    state: &Path,
    job_id: &str,
    stall_after_ms: u64,
    action: StallActionArg,
) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    let policy = queue.set_stall_policy(job_id, stall_after_ms, action.into())?;
    println!("corvid jobs loop stall-policy");
    println!("state: {}", state.display());
    print_stall_policy(&policy);
    Ok(0)
}

pub(crate) fn cmd_jobs_loop_check_stall(state: &Path, job_id: &str, actor: &str) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    let check = queue.check_stall(job_id, actor)?;
    println!("corvid jobs loop check-stall");
    println!("state: {}", state.display());
    print_stall_check(&check);
    if let Some(job) = queue.get(job_id)? {
        println!("status: {}", job.status.as_str());
        println!(
            "failure_kind: {}",
            job.failure_kind.as_deref().unwrap_or("")
        );
        println!(
            "failure_fingerprint: {}",
            job.failure_fingerprint.as_deref().unwrap_or("")
        );
    }
    Ok(0)
}

pub(crate) fn cmd_jobs_dlq(state: &Path) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    let jobs = queue.dead_lettered()?;
    println!("corvid jobs dlq");
    println!("state: {}", state.display());
    println!("dead_lettered_count: {}", jobs.len());
    for job in jobs {
        println!(
            "dead_lettered: {} task:{} attempts:{} failure_kind:{} failure_fingerprint:{} replay_key:{}",
            job.id,
            job.task,
            job.attempts,
            job.failure_kind.as_deref().unwrap_or(""),
            job.failure_fingerprint.as_deref().unwrap_or(""),
            job.replay_key.as_deref().unwrap_or("")
        );
    }
    Ok(0)
}

pub(crate) fn cmd_jobs_schedule_add(
    state: &Path,
    id: &str,
    cron: &str,
    zone: &str,
    task: &str,
    payload: &str,
    max_retries: u64,
    budget_usd: f64,
    effect_summary: Option<String>,
    replay_key_prefix: Option<String>,
    missed_policy: SchedulePolicyArg,
) -> Result<u8> {
    if let Some(parent) = state
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create jobs state dir `{}`", parent.display()))?;
    }
    let queue = DurableQueueRuntime::open(state)?;
    let payload = serde_json::from_str(payload).context("schedule payload must be valid JSON")?;
    let now = corvid_runtime::tracing::now_ms();
    let schedule = queue.upsert_schedule(QueueScheduleManifest {
        id: id.to_string(),
        cron: cron.to_string(),
        zone: zone.to_string(),
        task: task.to_string(),
        payload,
        max_retries,
        budget_usd,
        effect_summary,
        replay_key_prefix,
        missed_policy: missed_policy.into(),
        last_checked_ms: now,
        last_fire_ms: None,
        created_ms: now,
        updated_ms: now,
    })?;
    println!("corvid jobs schedule add");
    println!("state: {}", state.display());
    print_schedule_summary(&schedule);
    Ok(0)
}

pub(crate) fn cmd_jobs_schedule_list(state: &Path) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    let schedules = queue.list_schedules()?;
    println!("corvid jobs schedule list");
    println!("state: {}", state.display());
    println!("schedule_count: {}", schedules.len());
    for schedule in schedules {
        print_schedule_summary(&schedule);
    }
    Ok(0)
}

pub(crate) fn cmd_jobs_schedule_recover(state: &Path, max_missed_per_schedule: usize) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    let report = queue.recover_schedules(max_missed_per_schedule)?;
    println!("corvid jobs schedule recover");
    println!("state: {}", state.display());
    println!("scanned: {}", report.scanned);
    println!("enqueued: {}", report.enqueued);
    println!("skipped: {}", report.skipped);
    for recovery in report.recoveries {
        println!(
            "recovery: schedule:{} task:{} fire_ms:{} action:{} job:{} policy:{}",
            recovery.schedule_id,
            recovery.task,
            recovery.fire_ms,
            recovery.action,
            recovery.job_id.as_deref().unwrap_or(""),
            recovery.policy.as_str()
        );
    }
    Ok(0)
}

pub(crate) fn cmd_jobs_limit_set(
    state: &Path,
    scope: LimitScopeArg,
    task: Option<&str>,
    max_leased: u64,
) -> Result<u8> {
    if let Some(parent) = state
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create jobs state dir `{}`", parent.display()))?;
    }
    let queue = DurableQueueRuntime::open(state)?;
    let limit = match scope {
        LimitScopeArg::Global => queue.set_global_concurrency_limit(max_leased)?,
        LimitScopeArg::Task => {
            let task = task.context("--task is required when --scope task")?;
            queue.set_task_concurrency_limit(task, max_leased)?
        }
    };
    println!("corvid jobs limit set");
    println!("state: {}", state.display());
    println!("scope: {}", limit.scope);
    println!("max_leased: {}", limit.limit);
    println!("updated_ms: {}", limit.updated_ms);
    Ok(0)
}

pub(crate) fn cmd_jobs_limit_list(state: &Path) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    let limits = queue.list_concurrency_limits()?;
    println!("corvid jobs limit list");
    println!("state: {}", state.display());
    println!("limit_count: {}", limits.len());
    for limit in limits {
        println!(
            "limit: scope:{} max_leased:{} updated_ms:{}",
            limit.scope, limit.limit, limit.updated_ms
        );
    }
    Ok(0)
}

pub(crate) fn cmd_jobs_checkpoint_add(
    state: &Path,
    job_id: &str,
    kind: CheckpointKindArg,
    label: &str,
    payload: &str,
    payload_fingerprint: Option<String>,
) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    let payload = serde_json::from_str(payload).context("checkpoint payload must be valid JSON")?;
    let checkpoint =
        queue.record_checkpoint(job_id, kind.into(), label, payload, payload_fingerprint)?;
    println!("corvid jobs checkpoint add");
    println!("state: {}", state.display());
    print_checkpoint_summary(&checkpoint);
    Ok(0)
}

pub(crate) fn cmd_jobs_checkpoint_list(state: &Path, job_id: &str) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    let checkpoints = queue.list_checkpoints(job_id)?;
    println!("corvid jobs checkpoint list");
    println!("state: {}", state.display());
    println!("job: {job_id}");
    println!("checkpoint_count: {}", checkpoints.len());
    for checkpoint in checkpoints {
        print_checkpoint_summary(&checkpoint);
    }
    Ok(0)
}

pub(crate) fn cmd_jobs_checkpoint_resume(state: &Path, job_id: &str) -> Result<u8> {
    let queue = DurableQueueRuntime::open(state)?;
    let resume = queue.resume_state(job_id)?;
    println!("corvid jobs checkpoint resume");
    println!("state: {}", state.display());
    println!("job: {}", resume.job.id);
    println!("status: {}", resume.job.status.as_str());
    println!("checkpoint_count: {}", resume.checkpoints.len());
    println!("next_sequence: {}", resume.next_sequence);
    if let Some(checkpoint) = resume.last_checkpoint {
        println!("last_checkpoint: {}", checkpoint.id);
        println!("last_kind: {}", checkpoint.kind.as_str());
        println!("last_label: {}", checkpoint.label);
        println!("last_sequence: {}", checkpoint.sequence);
    } else {
        println!("last_checkpoint: ");
    }
    Ok(0)
}

