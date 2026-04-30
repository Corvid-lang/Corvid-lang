//! Read-only views over the approval queue: `corvid approvals
//! queue` (list by tenant), `corvid approvals inspect` (single
//! record + chronological audit trail), `corvid approvals export`
//! (full auditable transcript scoped to a tenant + since-window).
//!
//! Each operation opens [`ApprovalQueueRuntime`] read-only — none
//! of these subcommands transition state or write audit events.

use anyhow::{anyhow, Result};
use corvid_runtime::approval_queue::{ApprovalQueueRecord, ApprovalQueueRuntime};
use std::path::PathBuf;

use super::{summarise, summarise_audit, ApprovalSummary, AuditEventSummary};

#[derive(Debug, Clone)]
pub struct ApprovalsQueueArgs {
    pub approvals_state: PathBuf,
    pub tenant_id: String,
    /// Optional status filter: `pending`, `approved`, `denied`, `expired`.
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalsQueueOutput {
    pub tenant_id: String,
    pub approvals: Vec<ApprovalSummary>,
}

/// List approvals for a tenant, optionally filtered by status.
pub fn run_approvals_queue(args: ApprovalsQueueArgs) -> Result<ApprovalsQueueOutput> {
    let approvals = ApprovalQueueRuntime::open(&args.approvals_state)
        .map_err(|e| anyhow!("approvals runtime init failed: {e}"))?;
    let records = approvals
        .list_by_tenant(&args.tenant_id)
        .map_err(|e| anyhow!("list approvals: {e}"))?;
    let filtered: Vec<ApprovalQueueRecord> = if let Some(status) = args.status {
        records.into_iter().filter(|r| r.status == status).collect()
    } else {
        records
    };
    Ok(ApprovalsQueueOutput {
        tenant_id: args.tenant_id,
        approvals: filtered.into_iter().map(summarise).collect(),
    })
}

#[derive(Debug, Clone)]
pub struct ApprovalsInspectArgs {
    pub approvals_state: PathBuf,
    pub tenant_id: String,
    pub approval_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalsInspectOutput {
    pub approval: ApprovalSummary,
    pub audit_events: Vec<AuditEventSummary>,
}

/// Inspect a single approval — returns the record + every audit
/// event in chronological order.
pub fn run_approvals_inspect(args: ApprovalsInspectArgs) -> Result<ApprovalsInspectOutput> {
    let approvals = ApprovalQueueRuntime::open(&args.approvals_state)
        .map_err(|e| anyhow!("approvals runtime init failed: {e}"))?;
    let record = approvals
        .get(&args.approval_id)
        .map_err(|e| anyhow!("get approval: {e}"))?
        .ok_or_else(|| anyhow!("approval `{}` not found", args.approval_id))?;
    if record.tenant_id != args.tenant_id {
        return Err(anyhow!(
            "approval `{}` belongs to tenant `{}`, not `{}`",
            args.approval_id,
            record.tenant_id,
            args.tenant_id
        ));
    }
    let events = approvals
        .audit_events(&args.approval_id)
        .map_err(|e| anyhow!("approval audit: {e}"))?;
    Ok(ApprovalsInspectOutput {
        approval: summarise(record),
        audit_events: events.into_iter().map(summarise_audit).collect(),
    })
}

#[derive(Debug, Clone)]
pub struct ApprovalsExportArgs {
    pub approvals_state: PathBuf,
    pub tenant_id: String,
    pub since_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalsExportOutput {
    pub tenant_id: String,
    pub approvals: Vec<ApprovalExportEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalExportEntry {
    pub approval: ApprovalSummary,
    pub audit_events: Vec<AuditEventSummary>,
}

/// Export every approval (with full audit trail) for a tenant
/// since the supplied timestamp. The output is the auditable
/// transcript a compliance review consumes.
pub fn run_approvals_export(args: ApprovalsExportArgs) -> Result<ApprovalsExportOutput> {
    let approvals = ApprovalQueueRuntime::open(&args.approvals_state)
        .map_err(|e| anyhow!("approvals runtime init failed: {e}"))?;
    let records = approvals
        .list_by_tenant(&args.tenant_id)
        .map_err(|e| anyhow!("list approvals: {e}"))?;
    let mut entries = Vec::new();
    for record in records {
        if let Some(since) = args.since_ms {
            if record.created_ms < since {
                continue;
            }
        }
        let events = approvals
            .audit_events(&record.id)
            .map_err(|e| anyhow!("audit events: {e}"))?;
        entries.push(ApprovalExportEntry {
            approval: summarise(record),
            audit_events: events.into_iter().map(summarise_audit).collect(),
        });
    }
    Ok(ApprovalsExportOutput {
        tenant_id: args.tenant_id,
        approvals: entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth_cmd::{run_approvals_approve, ApprovalsTransitionArgs};
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

    /// Slice 39L: `corvid approvals queue` lists by tenant, filters by status.
    #[test]
    fn approvals_queue_filters_by_status() {
        let (_dir, approvals_state) = temp_paths();
        seed_pending_approval(&approvals_state, "ap-1", "tenant-1", "Admin");
        let out = run_approvals_queue(ApprovalsQueueArgs {
            approvals_state: approvals_state.clone(),
            tenant_id: "tenant-1".to_string(),
            status: None,
        })
        .expect("queue");
        assert_eq!(out.approvals.len(), 1);
        assert_eq!(out.approvals[0].status, "pending");
        let filtered = run_approvals_queue(ApprovalsQueueArgs {
            approvals_state,
            tenant_id: "tenant-1".to_string(),
            status: Some("approved".to_string()),
        })
        .expect("queue filtered");
        assert!(filtered.approvals.is_empty());
    }

    /// Slice 39L adversarial: `inspect` for an approval that
    /// belongs to a different tenant fails with a clear message
    /// (no cross-tenant disclosure).
    #[test]
    fn approvals_inspect_rejects_wrong_tenant() {
        let (_dir, approvals_state) = temp_paths();
        seed_pending_approval(&approvals_state, "ap-1", "tenant-1", "Admin");
        let err = run_approvals_inspect(ApprovalsInspectArgs {
            approvals_state,
            tenant_id: "tenant-2".to_string(),
            approval_id: "ap-1".to_string(),
        })
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("tenant"), "{msg}");
    }

    /// Slice 39L: `corvid approvals export` emits the typed
    /// approval + audit transcript a compliance review consumes.
    #[test]
    fn approvals_export_emits_record_with_audit_trail() {
        let (_dir, approvals_state) = temp_paths();
        seed_pending_approval(&approvals_state, "ap-1", "tenant-1", "Admin");
        run_approvals_approve(ApprovalsTransitionArgs {
            approvals_state: approvals_state.clone(),
            tenant_id: "tenant-1".to_string(),
            approval_id: "ap-1".to_string(),
            actor_id: "actor-admin".to_string(),
            role: "Admin".to_string(),
            reason: None,
        })
        .unwrap();
        let out = run_approvals_export(ApprovalsExportArgs {
            approvals_state,
            tenant_id: "tenant-1".to_string(),
            since_ms: None,
        })
        .expect("export");
        assert_eq!(out.approvals.len(), 1);
        assert_eq!(out.approvals[0].approval.status, "approved");
        assert!(out.approvals[0]
            .audit_events
            .iter()
            .any(|e| e.event_kind == "created"));
        assert!(out.approvals[0]
            .audit_events
            .iter()
            .any(|e| e.event_kind == "approved"));
    }
}
