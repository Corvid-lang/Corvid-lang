//! State-transition subcommands: `corvid approvals approve`,
//! `deny`, `expire`. Each writes an audit event and flips the
//! approval's status.
//!
//! Authorisation is enforced by the runtime: an actor's role
//! must satisfy the contract's `required_role` for `approve` /
//! `deny`; `expire` is a system-driven transition that does not
//! gate on role but still records the actor for the audit
//! trail.

use anyhow::{anyhow, Result};
use corvid_runtime::approval_authorization::ApprovalActorContext;
use corvid_runtime::approval_queue::ApprovalQueueRuntime;
use std::path::PathBuf;

use super::{summarise, ApprovalSummary};

#[derive(Debug, Clone)]
pub struct ApprovalsTransitionArgs {
    pub approvals_state: PathBuf,
    pub tenant_id: String,
    pub approval_id: String,
    pub actor_id: String,
    pub role: String,
    pub reason: Option<String>,
}

pub fn run_approvals_approve(args: ApprovalsTransitionArgs) -> Result<ApprovalSummary> {
    let approvals = ApprovalQueueRuntime::open(&args.approvals_state)
        .map_err(|e| anyhow!("approvals runtime init failed: {e}"))?;
    let actor = ApprovalActorContext {
        actor_id: args.actor_id.clone(),
        tenant_id: args.tenant_id.clone(),
        role: args.role.clone(),
    };
    let record = approvals
        .approve(&args.approval_id, &args.tenant_id, &actor, args.reason.as_deref())
        .map_err(|e| anyhow!("approve: {e}"))?;
    Ok(summarise(record))
}

pub fn run_approvals_deny(args: ApprovalsTransitionArgs) -> Result<ApprovalSummary> {
    let approvals = ApprovalQueueRuntime::open(&args.approvals_state)
        .map_err(|e| anyhow!("approvals runtime init failed: {e}"))?;
    let actor = ApprovalActorContext {
        actor_id: args.actor_id.clone(),
        tenant_id: args.tenant_id.clone(),
        role: args.role.clone(),
    };
    let record = approvals
        .deny(&args.approval_id, &args.tenant_id, &actor, args.reason.as_deref())
        .map_err(|e| anyhow!("deny: {e}"))?;
    Ok(summarise(record))
}

#[derive(Debug, Clone)]
pub struct ApprovalsExpireArgs {
    pub approvals_state: PathBuf,
    pub tenant_id: String,
    pub approval_id: String,
    pub actor_id: String,
    pub reason: Option<String>,
}

pub fn run_approvals_expire(args: ApprovalsExpireArgs) -> Result<ApprovalSummary> {
    let approvals = ApprovalQueueRuntime::open(&args.approvals_state)
        .map_err(|e| anyhow!("approvals runtime init failed: {e}"))?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let record = approvals
        .expire(
            &args.approval_id,
            &args.tenant_id,
            &args.actor_id,
            args.reason.as_deref(),
            now_ms,
        )
        .map_err(|e| anyhow!("expire: {e}"))?;
    Ok(summarise(record))
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

    /// Slice 39L: `corvid approvals approve` walks the pending →
    /// approved transition and writes audit.
    #[test]
    fn approvals_approve_transitions_to_approved() {
        let (_dir, approvals_state) = temp_paths();
        seed_pending_approval(&approvals_state, "ap-1", "tenant-1", "Admin");
        let summary = run_approvals_approve(ApprovalsTransitionArgs {
            approvals_state: approvals_state.clone(),
            tenant_id: "tenant-1".to_string(),
            approval_id: "ap-1".to_string(),
            actor_id: "actor-admin".to_string(),
            role: "Admin".to_string(),
            reason: Some("looks good".to_string()),
        })
        .expect("approve");
        assert_eq!(summary.status, "approved");
        // Inspect to confirm audit trail.
        let inspect = run_approvals_inspect(ApprovalsInspectArgs {
            approvals_state,
            tenant_id: "tenant-1".to_string(),
            approval_id: "ap-1".to_string(),
        })
        .expect("inspect");
        assert!(inspect
            .audit_events
            .iter()
            .any(|e| e.event_kind == "approved"));
    }

    /// Slice 39L: `corvid approvals deny` walks the pending →
    /// denied transition.
    #[test]
    fn approvals_deny_transitions_to_denied() {
        let (_dir, approvals_state) = temp_paths();
        seed_pending_approval(&approvals_state, "ap-1", "tenant-1", "Admin");
        let summary = run_approvals_deny(ApprovalsTransitionArgs {
            approvals_state,
            tenant_id: "tenant-1".to_string(),
            approval_id: "ap-1".to_string(),
            actor_id: "actor-admin".to_string(),
            role: "Admin".to_string(),
            reason: Some("policy violation".to_string()),
        })
        .expect("deny");
        assert_eq!(summary.status, "denied");
    }

    /// Slice 39L adversarial: an actor with the wrong role cannot
    /// approve. Ensures the role check propagates from runtime.
    #[test]
    fn approvals_approve_rejects_wrong_role() {
        let (_dir, approvals_state) = temp_paths();
        seed_pending_approval(&approvals_state, "ap-1", "tenant-1", "Admin");
        let err = run_approvals_approve(ApprovalsTransitionArgs {
            approvals_state,
            tenant_id: "tenant-1".to_string(),
            approval_id: "ap-1".to_string(),
            actor_id: "actor-member".to_string(),
            role: "Member".to_string(),
            reason: None,
        })
        .unwrap_err();
        assert!(err.to_string().contains("role"), "{err}");
    }
}
