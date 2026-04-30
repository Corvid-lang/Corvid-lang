//! `corvid approvals` CLI surface — slice 39L, decomposed in
//! Phase 20j-S1.
//!
//! The runtime functions (`ApprovalQueueRuntime::approve`,
//! `::deny`, `::expire`, `::comment`, `::delegate`, `::list_by_tenant`)
//! are unchanged; this module contributes the clap-shaped CLI
//! surface plus JSON-rendering of the runtime's typed records.
//!
//! `--approvals-state` defaults to `target/approvals.db`; the
//! file is a SQLite database initialised by
//! [`crate::auth_cmd::run_auth_migrate`] on first deploy.
//!
//! The module is split per CLI use-case:
//!
//! - [`queue`] — read views: `queue` (list by tenant), `inspect`
//!   (single record + audit trail), `export` (auditable transcript).
//! - [`transition`] — state transitions: `approve`, `deny`, `expire`.
//!   Lands in slice 20j-S1 commit 3.
//! - [`interaction`] — annotations + delegation + batch:
//!   `comment`, `delegate`, `batch`. Lands in slice 20j-S1 commit 4.
//!
//! Shared typed records ([`ApprovalSummary`], [`AuditEventSummary`])
//! and the runtime → CLI summary helpers (`summarise`,
//! `summarise_audit`) live in this file because every sub-module
//! consumes them.

pub mod queue;
pub mod transition;
#[allow(unused_imports)]
pub use queue::*;
#[allow(unused_imports)]
pub use transition::*;

use corvid_runtime::approval_queue::{
    ApprovalQueueAuditEvent, ApprovalQueueRecord,
};

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalSummary {
    pub id: String,
    pub status: String,
    pub action: String,
    pub target_kind: String,
    pub target_id: String,
    pub required_role: String,
    pub risk_level: String,
    pub max_cost_usd: f64,
    pub expires_at_ms: u64,
    pub created_at_ms: u64,
    pub trace_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuditEventSummary {
    pub event_kind: String,
    pub status_before: String,
    pub status_after: String,
    pub actor_id: String,
    pub reason: Option<String>,
    pub created_at_ms: u64,
}

/// Runtime → CLI summary projector. Crate-visible because every
/// sub-module of `approvals_cmd` (and the residual transition /
/// interaction code still in `auth_cmd` mid-refactor) calls it.
pub(crate) fn summarise(record: ApprovalQueueRecord) -> ApprovalSummary {
    ApprovalSummary {
        id: record.id,
        status: record.status,
        action: record.action,
        target_kind: record.target_kind,
        target_id: record.target_id,
        required_role: record.required_role,
        risk_level: record.risk_level,
        max_cost_usd: record.max_cost_usd,
        expires_at_ms: record.expires_ms,
        created_at_ms: record.created_ms,
        trace_id: record.trace_id,
    }
}

/// Runtime → CLI audit-event projector. Same crate-visibility
/// rationale as `summarise`.
pub(crate) fn summarise_audit(event: ApprovalQueueAuditEvent) -> AuditEventSummary {
    AuditEventSummary {
        event_kind: event.event_kind,
        status_before: event.status_before,
        status_after: event.status_after,
        actor_id: event.actor_id,
        reason: event.reason,
        created_at_ms: event.created_ms,
    }
}
