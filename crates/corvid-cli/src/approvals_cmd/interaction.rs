//! Annotation + delegation + batch subcommands: `corvid
//! approvals comment`, `delegate`, `batch`. These don't transition
//! the approval state directly (except `batch`, which is just a
//! loop over `approve`); they enrich the audit trail and let an
//! operator hand a pending approval off to a different reviewer.
//!
//! Per-id failures in `batch` are isolated rather than aborting
//! the whole call — the operator gets a clear "succeeded N,
//! failed M" summary with per-id reasons.

use anyhow::{anyhow, Result};
use corvid_runtime::approval_authorization::ApprovalActorContext;
use corvid_runtime::approval_queue::ApprovalQueueRuntime;
use std::path::PathBuf;

use super::{summarise, summarise_audit, ApprovalSummary, AuditEventSummary};

#[derive(Debug, Clone)]
pub struct ApprovalsCommentArgs {
    pub approvals_state: PathBuf,
    pub tenant_id: String,
    pub approval_id: String,
    pub actor_id: String,
    pub comment: String,
}

pub fn run_approvals_comment(args: ApprovalsCommentArgs) -> Result<AuditEventSummary> {
    let approvals = ApprovalQueueRuntime::open(&args.approvals_state)
        .map_err(|e| anyhow!("approvals runtime init failed: {e}"))?;
    let event = approvals
        .comment(
            &args.approval_id,
            &args.tenant_id,
            &args.actor_id,
            &args.comment,
        )
        .map_err(|e| anyhow!("comment: {e}"))?;
    Ok(summarise_audit(event))
}

#[derive(Debug, Clone)]
pub struct ApprovalsDelegateArgs {
    pub approvals_state: PathBuf,
    pub tenant_id: String,
    pub approval_id: String,
    pub actor_id: String,
    pub role: String,
    pub delegate_to: String,
    pub reason: Option<String>,
}

pub fn run_approvals_delegate(args: ApprovalsDelegateArgs) -> Result<ApprovalSummary> {
    let approvals = ApprovalQueueRuntime::open(&args.approvals_state)
        .map_err(|e| anyhow!("approvals runtime init failed: {e}"))?;
    let actor = ApprovalActorContext {
        actor_id: args.actor_id.clone(),
        tenant_id: args.tenant_id.clone(),
        role: args.role.clone(),
    };
    let record = approvals
        .delegate(
            &args.approval_id,
            &args.tenant_id,
            &actor,
            &args.delegate_to,
            args.reason.as_deref(),
        )
        .map_err(|e| anyhow!("delegate: {e}"))?;
    Ok(summarise(record))
}

#[derive(Debug, Clone)]
pub struct ApprovalsBatchArgs {
    pub approvals_state: PathBuf,
    pub tenant_id: String,
    pub actor_id: String,
    pub role: String,
    pub approval_ids: Vec<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalsBatchOutput {
    pub approved: Vec<ApprovalSummary>,
    pub failed: Vec<BatchFailure>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BatchFailure {
    pub approval_id: String,
    pub reason: String,
}

/// Approve a batch of approval ids in one operation. Per-approval
/// failures (wrong role, wrong tenant, already-resolved) are
/// reported individually rather than aborting the whole batch —
/// the operator gets a clear "succeeded N, failed M" summary.
pub fn run_approvals_batch(args: ApprovalsBatchArgs) -> Result<ApprovalsBatchOutput> {
    let approvals = ApprovalQueueRuntime::open(&args.approvals_state)
        .map_err(|e| anyhow!("approvals runtime init failed: {e}"))?;
    let actor = ApprovalActorContext {
        actor_id: args.actor_id.clone(),
        tenant_id: args.tenant_id.clone(),
        role: args.role.clone(),
    };
    let mut approved = Vec::new();
    let mut failed = Vec::new();
    for id in &args.approval_ids {
        match approvals.approve(id, &args.tenant_id, &actor, args.reason.as_deref()) {
            Ok(record) => approved.push(summarise(record)),
            Err(e) => failed.push(BatchFailure {
                approval_id: id.clone(),
                reason: format!("{e}"),
            }),
        }
    }
    Ok(ApprovalsBatchOutput { approved, failed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approvals_cmd::queue::{run_approvals_inspect, ApprovalsInspectArgs};
    use corvid_runtime::approval_queue::{
        ApprovalContractRecord, ApprovalCreate, ApprovalQueueRuntime,
    };
    use tempfile::tempdir;

    fn temp_paths() -> (tempfile::TempDir, PathBuf) {
        let dir = tempdir().unwrap();
        let approvals = dir.path().join("approvals.db");
        (dir, approvals)
    }

    fn seed_pending_approval(approvals_state: &PathBuf, id: &str, tenant: &str, role: &str) {
        let approvals = ApprovalQueueRuntime::open(approvals_state).unwrap();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let contract = ApprovalContractRecord {
            id: format!("{id}-contract"),
            version: "v1".to_string(),
            action: "issue_refund".to_string(),
            target_kind: "order".to_string(),
            target_id: "ord_42".to_string(),
            tenant_id: tenant.to_string(),
            required_role: role.to_string(),
            max_cost_usd: 100.0,
            data_class: "financial".to_string(),
            irreversible: true,
            expires_ms: now_ms + 60_000,
            replay_key: format!("rk-{id}"),
            created_ms: now_ms,
        };
        approvals
            .create(ApprovalCreate {
                id: id.to_string(),
                tenant_id: tenant.to_string(),
                requester_actor_id: "requester-1".to_string(),
                contract,
                risk_level: "high".to_string(),
                trace_id: format!("trace-{id}"),
            })
            .unwrap();
    }

    /// Slice 39L: `corvid approvals comment` records an audit
    /// event without changing status.
    #[test]
    fn approvals_comment_writes_audit_without_status_change() {
        let (_dir, approvals_state) = temp_paths();
        seed_pending_approval(&approvals_state, "ap-1", "tenant-1", "Admin");
        let event = run_approvals_comment(ApprovalsCommentArgs {
            approvals_state: approvals_state.clone(),
            tenant_id: "tenant-1".to_string(),
            approval_id: "ap-1".to_string(),
            actor_id: "actor-x".to_string(),
            comment: "needs more context".to_string(),
        })
        .expect("comment");
        assert_eq!(event.event_kind, "commented");
        let inspect = run_approvals_inspect(ApprovalsInspectArgs {
            approvals_state,
            tenant_id: "tenant-1".to_string(),
            approval_id: "ap-1".to_string(),
        })
        .expect("inspect");
        assert_eq!(inspect.approval.status, "pending");
    }

    /// Slice 39L: `corvid approvals batch` approves multiple in
    /// one invocation; per-id failures are isolated.
    #[test]
    fn approvals_batch_approves_succeeded_isolates_failures() {
        let (_dir, approvals_state) = temp_paths();
        seed_pending_approval(&approvals_state, "ap-1", "tenant-1", "Admin");
        seed_pending_approval(&approvals_state, "ap-2", "tenant-1", "Reviewer");
        let out = run_approvals_batch(ApprovalsBatchArgs {
            approvals_state,
            tenant_id: "tenant-1".to_string(),
            actor_id: "actor-admin".to_string(),
            role: "Admin".to_string(),
            approval_ids: vec!["ap-1".to_string(), "ap-2".to_string(), "ap-missing".to_string()],
            reason: Some("batch approve".to_string()),
        })
        .expect("batch");
        assert_eq!(out.approved.len(), 1);
        assert_eq!(out.approved[0].id, "ap-1");
        assert_eq!(out.failed.len(), 2);
        let failed_ids: Vec<&str> = out.failed.iter().map(|f| f.approval_id.as_str()).collect();
        assert!(failed_ids.contains(&"ap-2"));
        assert!(failed_ids.contains(&"ap-missing"));
    }
}
