//! Row-reading + string-parsing helpers shared across the queue
//! subsystem — slice 38 / persistent jobs surface, decomposed in
//! Phase 20j-A2.
//!
//! The 12 helpers here all share one shape: they translate
//! between the SQLite row representation (or a string token) and
//! a typed [`super::model`] record. Per-topic dispatch modules
//! call into this layer to read rows and parse status tokens
//! without duplicating the column-index plumbing.

use serde_json::Value;

use super::model::{
    JobAuditEvent, JobCheckpoint, JobCheckpointKind, JobLoopHeartbeat, JobLoopLimits,
    JobLoopUsage, JobStallAction, JobStallPolicy, QueueJob, QueueJobStatus,
    QueueScheduleManifest, ScheduleMissedPolicy,
};

pub(super) fn read_job_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueueJob> {
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

pub(super) fn read_schedule_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueueScheduleManifest> {
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

pub(super) fn read_checkpoint_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobCheckpoint> {
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

pub(super) fn read_job_audit_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobAuditEvent> {
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

pub(super) fn read_loop_limits_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobLoopLimits> {
    Ok(JobLoopLimits {
        job_id: row.get(0)?,
        max_steps: row.get::<_, Option<i64>>(1)?.map(|value| value as u64),
        max_wall_ms: row.get::<_, Option<i64>>(2)?.map(|value| value as u64),
        max_spend_usd: row.get(3)?,
        max_tool_calls: row.get::<_, Option<i64>>(4)?.map(|value| value as u64),
        updated_ms: row.get::<_, i64>(5)? as u64,
    })
}

pub(super) fn read_loop_usage_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobLoopUsage> {
    Ok(JobLoopUsage {
        job_id: row.get(0)?,
        steps: row.get::<_, i64>(1)? as u64,
        wall_ms: row.get::<_, i64>(2)? as u64,
        spend_usd: row.get(3)?,
        tool_calls: row.get::<_, i64>(4)? as u64,
        updated_ms: row.get::<_, i64>(5)? as u64,
    })
}

pub(super) fn read_stall_policy_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobStallPolicy> {
    let action: String = row.get(2)?;
    Ok(JobStallPolicy {
        job_id: row.get(0)?,
        stall_after_ms: row.get::<_, i64>(1)? as u64,
        action: parse_stall_action(&action),
        updated_ms: row.get::<_, i64>(3)? as u64,
    })
}

pub(super) fn read_loop_heartbeat_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobLoopHeartbeat> {
    Ok(JobLoopHeartbeat {
        job_id: row.get(0)?,
        actor: row.get(1)?,
        message: row.get(2)?,
        last_heartbeat_ms: row.get::<_, i64>(3)? as u64,
        updated_ms: row.get::<_, i64>(4)? as u64,
    })
}

pub(super) fn parse_status(status: &str) -> QueueJobStatus {
    match status {
        "leased" => QueueJobStatus::Leased,
        "approval_wait" => QueueJobStatus::ApprovalWait,
        "approval_denied" => QueueJobStatus::ApprovalDenied,
        "approval_expired" => QueueJobStatus::ApprovalExpired,
        "loop_budget_exceeded" => QueueJobStatus::LoopBudgetExceeded,
        "loop_stall_escalated" => QueueJobStatus::LoopStallEscalated,
        "loop_stall_terminated" => QueueJobStatus::LoopStallTerminated,
        "retry_wait" => QueueJobStatus::RetryWait,
        "running" => QueueJobStatus::Running,
        "succeeded" => QueueJobStatus::Succeeded,
        "failed" => QueueJobStatus::Failed,
        "dead_lettered" => QueueJobStatus::DeadLettered,
        "canceled" => QueueJobStatus::Canceled,
        _ => QueueJobStatus::Pending,
    }
}

pub(super) fn parse_stall_action(action: &str) -> JobStallAction {
    match action {
        "terminate" => JobStallAction::Terminate,
        _ => JobStallAction::Escalate,
    }
}

pub(super) fn parse_missed_policy(policy: &str) -> ScheduleMissedPolicy {
    match policy {
        "skip_missed" => ScheduleMissedPolicy::SkipMissed,
        "enqueue_all_bounded" => ScheduleMissedPolicy::EnqueueAllBounded,
        _ => ScheduleMissedPolicy::FireOnceOnRecovery,
    }
}

pub(super) fn parse_checkpoint_kind(kind: &str) -> JobCheckpointKind {
    match kind {
        "tool_result" => JobCheckpointKind::ToolResult,
        "partial_output" => JobCheckpointKind::PartialOutput,
        _ => JobCheckpointKind::AgentStep,
    }
}
