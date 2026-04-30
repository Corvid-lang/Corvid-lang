//! Public typed records for the durable queue subsystem — slice
//! 38 / persistent jobs surface, decomposed in Phase 20j-A2.
//!
//! This file holds only the public data types the queue surface
//! exposes: status enums, job + schedule + checkpoint records,
//! approval/loop-limit/stall side records, and the typed audit
//! event. The persistence + dispatch logic lives in sibling
//! modules under `crate::queue::*`.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueJobStatus {
    Pending,
    Leased,
    ApprovalWait,
    ApprovalDenied,
    ApprovalExpired,
    LoopBudgetExceeded,
    LoopStallEscalated,
    LoopStallTerminated,
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
            Self::LoopStallEscalated => "loop_stall_escalated",
            Self::LoopStallTerminated => "loop_stall_terminated",
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStallAction {
    Escalate,
    Terminate,
}

impl JobStallAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Escalate => "escalate",
            Self::Terminate => "terminate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobStallPolicy {
    pub job_id: String,
    pub stall_after_ms: u64,
    pub action: JobStallAction,
    pub updated_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobLoopHeartbeat {
    pub job_id: String,
    pub actor: String,
    pub message: Option<String>,
    pub last_heartbeat_ms: u64,
    pub updated_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobStallCheck {
    pub job_id: String,
    pub stalled: bool,
    pub action_taken: Option<String>,
    pub last_heartbeat_ms: u64,
    pub stall_after_ms: u64,
    pub elapsed_ms: u64,
}
