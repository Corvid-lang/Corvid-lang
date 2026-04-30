//! `corvid jobs` clap argument tree — slice 38I, decomposed in
//! Phase 20j-A1.
//!
//! Owns every clap `Subcommand` / `ValueEnum` enum the
//! `corvid jobs *` surface uses, plus the `From` conversions that
//! project the CLI-shape enums onto the
//! [`corvid_runtime::queue`] runtime types. Keeps the argument
//! tree together so adding a new `corvid jobs *` subcommand only
//! touches this file (plus its dispatch arm in `main.rs`).

use clap::{Subcommand, ValueEnum};
use corvid_runtime::queue::{
    JobApprovalDecision, JobCheckpointKind, JobStallAction, ScheduleMissedPolicy,
};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum JobsCommand {
    /// Persist a new local background job.
    Enqueue {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        /// Job kind or task name.
        #[arg(long)]
        task: String,
        /// Redacted JSON input payload for the job.
        #[arg(long, default_value = "{}")]
        payload: String,
        /// Typed input schema name carried with the persisted job.
        #[arg(long)]
        input_schema: Option<String>,
        /// Maximum retry count available to later retry policies.
        #[arg(long, default_value = "3")]
        max_retries: u64,
        /// Budget carried with the job metadata.
        #[arg(long, default_value = "0")]
        budget_usd: f64,
        /// Human-readable effect summary, for audit output.
        #[arg(long)]
        effect_summary: Option<String>,
        /// Replay key linking the job to trace/replay metadata.
        #[arg(long)]
        replay_key: Option<String>,
        /// Idempotency key; duplicate enqueues return the existing job.
        #[arg(long)]
        idempotency_key: Option<String>,
        /// Persist the job for a future run after this many milliseconds.
        #[arg(long, default_value = "0")]
        delay_ms: u64,
    },
    /// Execute the first pending local job and persist the result metadata.
    RunOne {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        /// Typed output kind recorded after the job completes.
        #[arg(long)]
        output_kind: Option<String>,
        /// Redacted output fingerprint recorded after the job completes.
        #[arg(long)]
        output_fingerprint: Option<String>,
        /// Record this run as a failed attempt with the given redacted kind.
        #[arg(long)]
        fail_kind: Option<String>,
        /// Redacted failure fingerprint for failed attempts.
        #[arg(long)]
        fail_fingerprint: Option<String>,
        /// Base backoff in milliseconds for failed attempts.
        #[arg(long, default_value = "1000")]
        retry_base_ms: u64,
    },
    /// Run an N-worker async pool over the local queue. Each
    /// worker independently leases the next ready job, runs the
    /// shipped no-op executor (production callers wire their
    /// own), and finalises via complete_leased / fail_leased.
    /// Slice 38K's audit-correction surface — the multi-worker
    /// runner the audit found absent. Default lease TTL = 60s,
    /// default idle poll = 100ms. Press Ctrl-C to drain.
    Run {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        /// Number of concurrent workers.
        #[arg(long, default_value = "1")]
        workers: usize,
        /// Lease TTL in milliseconds.
        #[arg(long, default_value = "60000")]
        lease_ttl_ms: u64,
        /// Idle poll interval when the queue is empty.
        #[arg(long, default_value = "100")]
        idle_poll_ms: u64,
        /// Run for at most this many milliseconds, then drain.
        /// 0 = run until Ctrl-C / external drain. Tests use a
        /// finite duration so they don't hang.
        #[arg(long, default_value = "0")]
        max_runtime_ms: u64,
    },
    /// Inspect one job and its operational metadata.
    Inspect {
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        #[arg(long)]
        job: String,
    },
    /// Requeue a terminal or delayed job immediately.
    Retry {
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        #[arg(long)]
        job: String,
    },
    /// Cancel one job.
    Cancel {
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        #[arg(long)]
        job: String,
    },
    /// Pause leasing new work from the local queue.
    Pause {
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Resume leasing work from the local queue.
    Resume {
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
    },
    /// Pause the queue and release active leases back to pending.
    Drain {
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Export a redacted JSON trace for one job.
    ExportTrace {
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        #[arg(long)]
        job: String,
        #[arg(long, value_name = "PATH")]
        out: Option<PathBuf>,
    },
    /// Lease the next runnable job and pause it on a human approval boundary.
    WaitApproval {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        /// Worker identity that owns the short lease before the approval wait starts.
        #[arg(long, default_value = "corvid-approval-wait")]
        worker_id: String,
        /// Lease TTL while atomically moving the job into approval-wait.
        #[arg(long, default_value = "300000")]
        lease_ttl_ms: u64,
        /// Stable approval request id supplied by the approval product surface.
        #[arg(long)]
        approval_id: String,
        /// Absolute Unix epoch milliseconds when the approval request expires.
        #[arg(long)]
        approval_expires_ms: u64,
        /// Human-readable reason preserved with the durable job state.
        #[arg(long)]
        approval_reason: String,
    },
    /// List jobs paused on approval.
    Approvals {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
    },
    /// Decide or audit a paused approval-wait job.
    Approval {
        #[command(subcommand)]
        command: JobsApprovalCommand,
    },
    /// Configure and record bounded agent-loop usage.
    Loop {
        #[command(subcommand)]
        command: JobsLoopCommand,
    },
    /// Manage durable cron schedules and restart recovery.
    Schedule {
        #[command(subcommand)]
        command: JobsScheduleCommand,
    },
    /// Configure queue and task concurrency limits.
    Limit {
        #[command(subcommand)]
        command: JobsLimitCommand,
    },
    /// Record and inspect durable job checkpoints.
    Checkpoint {
        #[command(subcommand)]
        command: JobsCheckpointCommand,
    },
    /// Inspect terminally failed local jobs.
    Dlq {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum CheckpointKindArg {
    AgentStep,
    ToolResult,
    PartialOutput,
}

impl From<CheckpointKindArg> for JobCheckpointKind {
    fn from(value: CheckpointKindArg) -> Self {
        match value {
            CheckpointKindArg::AgentStep => JobCheckpointKind::AgentStep,
            CheckpointKindArg::ToolResult => JobCheckpointKind::ToolResult,
            CheckpointKindArg::PartialOutput => JobCheckpointKind::PartialOutput,
        }
    }
}

#[derive(Subcommand)]
pub enum JobsApprovalCommand {
    /// Approve, deny, or expire a job waiting on a durable approval.
    Decide {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        /// Job id currently in approval-wait.
        #[arg(long)]
        job: String,
        /// Approval request id that must match the waiting job.
        #[arg(long)]
        approval_id: String,
        /// Decision to apply.
        #[arg(long, value_enum)]
        decision: ApprovalDecisionArg,
        /// Actor id for the durable audit event.
        #[arg(long)]
        actor: String,
        /// Redacted decision reason recorded in the audit event.
        #[arg(long)]
        reason: Option<String>,
    },
    /// List durable approval audit events for one job.
    Audit {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        /// Job id to inspect.
        #[arg(long)]
        job: String,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ApprovalDecisionArg {
    Approve,
    Deny,
    Expire,
}

impl ApprovalDecisionArg {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Deny => "deny",
            Self::Expire => "expire",
        }
    }
}

impl From<ApprovalDecisionArg> for JobApprovalDecision {
    fn from(value: ApprovalDecisionArg) -> Self {
        match value {
            ApprovalDecisionArg::Approve => JobApprovalDecision::Approve,
            ApprovalDecisionArg::Deny => JobApprovalDecision::Deny,
            ApprovalDecisionArg::Expire => JobApprovalDecision::Expire,
        }
    }
}

#[derive(Subcommand)]
pub enum JobsLoopCommand {
    /// Set max steps, wall time, spend, and tool-call limits for a job.
    Limits {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        /// Job id to constrain.
        #[arg(long)]
        job: String,
        /// Maximum agent-loop steps.
        #[arg(long)]
        max_steps: Option<u64>,
        /// Maximum cumulative wall time in milliseconds.
        #[arg(long)]
        max_wall_ms: Option<u64>,
        /// Maximum cumulative spend in USD.
        #[arg(long)]
        max_spend_usd: Option<f64>,
        /// Maximum cumulative tool calls.
        #[arg(long)]
        max_tool_calls: Option<u64>,
    },
    /// Add usage deltas after one bounded agent-loop step.
    Record {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        /// Job id to update.
        #[arg(long)]
        job: String,
        /// Step delta to add.
        #[arg(long, default_value = "0")]
        steps: u64,
        /// Wall-time delta in milliseconds.
        #[arg(long, default_value = "0")]
        wall_ms: u64,
        /// Spend delta in USD.
        #[arg(long, default_value = "0")]
        spend_usd: f64,
        /// Tool-call delta to add.
        #[arg(long, default_value = "0")]
        tool_calls: u64,
        /// Worker or runtime actor recording the usage.
        #[arg(long, default_value = "corvid-loop-runtime")]
        actor: String,
    },
    /// Inspect current limits and cumulative usage for one job.
    Usage {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        /// Job id to inspect.
        #[arg(long)]
        job: String,
    },
    /// Record a worker heartbeat for stall detection.
    Heartbeat {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        /// Job id to heartbeat.
        #[arg(long)]
        job: String,
        /// Worker or runtime actor recording the heartbeat.
        #[arg(long, default_value = "corvid-loop-runtime")]
        actor: String,
        /// Redacted progress message.
        #[arg(long)]
        message: Option<String>,
    },
    /// Configure stall escalation or termination for a job loop.
    StallPolicy {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        /// Job id to constrain.
        #[arg(long)]
        job: String,
        /// Milliseconds without heartbeat before the loop is stalled.
        #[arg(long)]
        stall_after_ms: u64,
        /// Action to apply after stall detection.
        #[arg(long, value_enum)]
        action: StallActionArg,
    },
    /// Check whether a job loop has stalled and apply the configured action.
    CheckStall {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        /// Job id to inspect.
        #[arg(long)]
        job: String,
        /// Operator/runtime actor performing the check.
        #[arg(long, default_value = "corvid-stall-watchdog")]
        actor: String,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum StallActionArg {
    Escalate,
    Terminate,
}

impl From<StallActionArg> for JobStallAction {
    fn from(value: StallActionArg) -> Self {
        match value {
            StallActionArg::Escalate => JobStallAction::Escalate,
            StallActionArg::Terminate => JobStallAction::Terminate,
        }
    }
}

#[derive(Subcommand)]
pub enum JobsCheckpointCommand {
    /// Record a durable checkpoint for a job.
    Add {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        /// Job id.
        #[arg(long)]
        job: String,
        /// Checkpoint kind.
        #[arg(long, value_enum)]
        kind: CheckpointKindArg,
        /// Human-readable step/tool/output label.
        #[arg(long)]
        label: String,
        /// Redacted JSON payload for the checkpoint.
        #[arg(long, default_value = "{}")]
        payload: String,
        /// Redacted payload fingerprint.
        #[arg(long)]
        payload_fingerprint: Option<String>,
    },
    /// List durable checkpoints for a job.
    List {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        /// Job id.
        #[arg(long)]
        job: String,
    },
    /// Show the restart resume point for a job.
    Resume {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        /// Job id.
        #[arg(long)]
        job: String,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum LimitScopeArg {
    Global,
    Task,
}

#[derive(Subcommand)]
pub enum JobsLimitCommand {
    /// Set a durable concurrency limit.
    Set {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        /// Limit scope.
        #[arg(long, value_enum)]
        scope: LimitScopeArg,
        /// Task name when --scope task.
        #[arg(long)]
        task: Option<String>,
        /// Maximum active leases allowed for this scope.
        #[arg(long)]
        max_leased: u64,
    },
    /// List durable concurrency limits.
    List {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum SchedulePolicyArg {
    SkipMissed,
    FireOnceOnRecovery,
    EnqueueAllBounded,
}

impl From<SchedulePolicyArg> for ScheduleMissedPolicy {
    fn from(value: SchedulePolicyArg) -> Self {
        match value {
            SchedulePolicyArg::SkipMissed => ScheduleMissedPolicy::SkipMissed,
            SchedulePolicyArg::FireOnceOnRecovery => ScheduleMissedPolicy::FireOnceOnRecovery,
            SchedulePolicyArg::EnqueueAllBounded => ScheduleMissedPolicy::EnqueueAllBounded,
        }
    }
}

#[derive(Subcommand)]
pub enum JobsScheduleCommand {
    /// Add or update a durable cron schedule.
    Add {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        /// Stable schedule id.
        #[arg(long)]
        id: String,
        /// Cron expression. Five-field expressions are accepted and normalized to second=0.
        #[arg(long)]
        cron: String,
        /// IANA timezone, such as UTC or America/New_York.
        #[arg(long, default_value = "UTC")]
        zone: String,
        /// Job kind or task name to enqueue when the schedule fires.
        #[arg(long)]
        task: String,
        /// Redacted JSON payload embedded into each recovered job.
        #[arg(long, default_value = "{}")]
        payload: String,
        /// Maximum retry count for jobs created by this schedule.
        #[arg(long, default_value = "3")]
        max_retries: u64,
        /// Budget carried into jobs created by this schedule.
        #[arg(long, default_value = "0")]
        budget_usd: f64,
        /// Human-readable effect summary for audit and operations output.
        #[arg(long)]
        effect_summary: Option<String>,
        /// Prefix used to create deterministic replay keys per scheduled fire.
        #[arg(long)]
        replay_key_prefix: Option<String>,
        /// Missed-fire policy applied after restart.
        #[arg(long, value_enum, default_value = "fire-once-on-recovery")]
        missed_policy: SchedulePolicyArg,
    },
    /// List durable cron schedules.
    List {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
    },
    /// Recover missed schedule fires after restart.
    Recover {
        /// SQLite state file used by the durable local queue.
        #[arg(long, value_name = "PATH", default_value = "target/corvid-jobs.sqlite")]
        state: PathBuf,
        /// Maximum missed fires to inspect per schedule.
        #[arg(long, default_value = "16")]
        max_missed_per_schedule: usize,
    },
}
