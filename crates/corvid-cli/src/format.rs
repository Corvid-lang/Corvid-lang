//! User-facing output projectors shared across CLI commands.
//!
//! Two distinct rendering layers live here:
//!
//! - **Approvals JSON projectors** turn the typed
//!   [`crate::approvals_cmd`] records into `serde_json::Value`s the
//!   `corvid approvals *` dispatch arms emit. Multiple arms reuse
//!   the same projector (`approval_summary_value` is consumed by
//!   `approve`, `deny`, `expire`, `delegate`, and the per-row
//!   rendering inside `queue` / `inspect` / `batch` / `export`).
//!
//! - **Jobs `print_*` helpers** stream a typed runtime record
//!   ([`corvid_runtime::queue::JobCheckpoint`] etc.) line by line
//!   to stdout in the `key: value` shape the `corvid jobs *`
//!   dispatch arms have emitted since slice 38I.
//!
//! Keeping every projector in one module means the dispatch
//! functions stay focused on the dispatch tree (slice 20j-A1
//! commit 11 will collapse them into `dispatch.rs`); each command
//! body is "open the runtime, call the operation, hand the result
//! to a renderer here".

use crate::approvals_cmd;
use corvid_runtime::queue::{
    JobCheckpoint, JobLoopLimits, JobLoopUsage, JobStallCheck, JobStallPolicy, QueueJob,
    QueueScheduleManifest,
};

// ---------------------------------------------------------------
// Approvals JSON projectors
// ---------------------------------------------------------------

pub(crate) fn approval_summary_value(s: &approvals_cmd::ApprovalSummary) -> serde_json::Value {
    serde_json::json!({
        "id": s.id,
        "status": s.status,
        "action": s.action,
        "target_kind": s.target_kind,
        "target_id": s.target_id,
        "required_role": s.required_role,
        "risk_level": s.risk_level,
        "max_cost_usd": s.max_cost_usd,
        "expires_at_ms": s.expires_at_ms,
        "created_at_ms": s.created_at_ms,
        "trace_id": s.trace_id,
    })
}

pub(crate) fn audit_event_value(e: &approvals_cmd::AuditEventSummary) -> serde_json::Value {
    serde_json::json!({
        "event_kind": e.event_kind,
        "status_before": e.status_before,
        "status_after": e.status_after,
        "actor_id": e.actor_id,
        "reason": e.reason,
        "created_at_ms": e.created_at_ms,
    })
}

pub(crate) fn approvals_queue_summary(out: &approvals_cmd::ApprovalsQueueOutput) -> serde_json::Value {
    serde_json::json!({
        "tenant_id": out.tenant_id,
        "approvals": out.approvals.iter().map(approval_summary_value).collect::<Vec<_>>(),
    })
}

pub(crate) fn approvals_inspect_summary(out: &approvals_cmd::ApprovalsInspectOutput) -> serde_json::Value {
    serde_json::json!({
        "approval": approval_summary_value(&out.approval),
        "audit_events": out.audit_events.iter().map(audit_event_value).collect::<Vec<_>>(),
    })
}

// ---------------------------------------------------------------
// Jobs print_* helpers
// ---------------------------------------------------------------

pub(crate) fn print_checkpoint_summary(checkpoint: &JobCheckpoint) {
    println!("checkpoint: {}", checkpoint.id);
    println!("job: {}", checkpoint.job_id);
    println!("sequence: {}", checkpoint.sequence);
    println!("kind: {}", checkpoint.kind.as_str());
    println!("label: {}", checkpoint.label);
    println!(
        "payload_fingerprint: {}",
        checkpoint.payload_fingerprint.as_deref().unwrap_or("")
    );
    println!("created_ms: {}", checkpoint.created_ms);
}

pub(crate) fn print_loop_limits(limits: &JobLoopLimits) {
    println!("job: {}", limits.job_id);
    println!(
        "max_steps: {}",
        limits
            .max_steps
            .map(|value| value.to_string())
            .unwrap_or_default()
    );
    println!(
        "max_wall_ms: {}",
        limits
            .max_wall_ms
            .map(|value| value.to_string())
            .unwrap_or_default()
    );
    println!(
        "max_spend_usd: {}",
        limits
            .max_spend_usd
            .map(|value| format!("{value:.6}"))
            .unwrap_or_default()
    );
    println!(
        "max_tool_calls: {}",
        limits
            .max_tool_calls
            .map(|value| value.to_string())
            .unwrap_or_default()
    );
    println!("limits_updated_ms: {}", limits.updated_ms);
}

pub(crate) fn print_loop_usage(usage: &JobLoopUsage) {
    println!("job: {}", usage.job_id);
    println!("steps: {}", usage.steps);
    println!("wall_ms: {}", usage.wall_ms);
    println!("spend_usd: {:.6}", usage.spend_usd);
    println!("tool_calls: {}", usage.tool_calls);
    println!("usage_updated_ms: {}", usage.updated_ms);
}

pub(crate) fn print_stall_policy(policy: &JobStallPolicy) {
    println!("job: {}", policy.job_id);
    println!("stall_after_ms: {}", policy.stall_after_ms);
    println!("action: {}", policy.action.as_str());
    println!("updated_ms: {}", policy.updated_ms);
}

pub(crate) fn print_stall_check(check: &JobStallCheck) {
    println!("job: {}", check.job_id);
    println!("stalled: {}", check.stalled);
    println!(
        "action_taken: {}",
        check.action_taken.as_deref().unwrap_or("")
    );
    println!("last_heartbeat_ms: {}", check.last_heartbeat_ms);
    println!("stall_after_ms: {}", check.stall_after_ms);
    println!("elapsed_ms: {}", check.elapsed_ms);
}

pub(crate) fn print_schedule_summary(schedule: &QueueScheduleManifest) {
    println!("schedule: {}", schedule.id);
    println!("cron: {}", schedule.cron);
    println!("zone: {}", schedule.zone);
    println!("task: {}", schedule.task);
    println!("missed_policy: {}", schedule.missed_policy.as_str());
    println!("last_checked_ms: {}", schedule.last_checked_ms);
    println!(
        "last_fire_ms: {}",
        schedule
            .last_fire_ms
            .map(|value| value.to_string())
            .unwrap_or_default()
    );
    println!("max_retries: {}", schedule.max_retries);
    println!("budget_usd: {:.4}", schedule.budget_usd);
    println!(
        "effect_summary: {}",
        schedule.effect_summary.as_deref().unwrap_or("")
    );
    println!(
        "replay_key_prefix: {}",
        schedule.replay_key_prefix.as_deref().unwrap_or("")
    );
}

pub(crate) fn print_job_summary(job: &QueueJob) {
    println!("job: {}", job.id);
    println!("task: {}", job.task);
    println!(
        "input_schema: {}",
        job.input_schema.as_deref().unwrap_or("")
    );
    println!("status: {}", job.status.as_str());
    println!("attempts: {}", job.attempts);
    println!("max_retries: {}", job.max_retries);
    println!("budget_usd: {:.4}", job.budget_usd);
    println!(
        "effect_summary: {}",
        job.effect_summary.as_deref().unwrap_or("")
    );
    println!("replay_key: {}", job.replay_key.as_deref().unwrap_or(""));
    println!(
        "idempotency_key: {}",
        job.idempotency_key.as_deref().unwrap_or("")
    );
    println!("output_kind: {}", job.output_kind.as_deref().unwrap_or(""));
    println!(
        "output_fingerprint: {}",
        job.output_fingerprint.as_deref().unwrap_or("")
    );
    println!(
        "failure_kind: {}",
        job.failure_kind.as_deref().unwrap_or("")
    );
    println!(
        "failure_fingerprint: {}",
        job.failure_fingerprint.as_deref().unwrap_or("")
    );
    println!(
        "next_run_ms: {}",
        job.next_run_ms
            .map(|value| value.to_string())
            .unwrap_or_default()
    );
    println!("lease_owner: {}", job.lease_owner.as_deref().unwrap_or(""));
    println!(
        "lease_expires_ms: {}",
        job.lease_expires_ms
            .map(|value| value.to_string())
            .unwrap_or_default()
    );
    println!("approval_id: {}", job.approval_id.as_deref().unwrap_or(""));
    println!(
        "approval_expires_ms: {}",
        job.approval_expires_ms
            .map(|value| value.to_string())
            .unwrap_or_default()
    );
    println!(
        "approval_reason: {}",
        job.approval_reason.as_deref().unwrap_or("")
    );
}
