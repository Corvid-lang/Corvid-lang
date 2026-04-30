//! `corvid auth` + `corvid approvals` CLI surface — slice 39L.
//!
//! Wires the Phase 39 auth + approval runtime into the top-level
//! `corvid` CLI so an operator can manage sessions / API keys /
//! OAuth tokens / approval queues from the shell rather than only
//! from Rust callers. The runtime functions
//! (`SessionAuthRuntime::create_api_key`,
//! `ApprovalQueueRuntime::approve`, etc.) are unchanged; this
//! slice contributes only the clap surface + JSON-rendering of
//! the runtime's typed records.
//!
//! `--auth-state` and `--approvals-state` default to
//! `target/auth.db` and `target/approvals.db` respectively. Both
//! file paths are SQLite databases initialised on first open;
//! `corvid auth migrate` is the explicit "open both, init both,
//! report success" operation an operator runs once at deploy.
//!
//! The module is split per CLI surface (Phase 20j-S1):
//!
//! - [`keys`] — `corvid auth keys issue/revoke/rotate` lifecycle.
//! - The `corvid auth migrate` initialiser stays in this file
//!   because it is small and tightly coupled to the deploy story.
//!
//! The `corvid approvals *` surface lives in the sibling
//! [`crate::approvals_cmd`] module so the auth and approval
//! lanes evolve independently.

pub mod keys;
#[allow(unused_imports)]
pub use keys::*;

use anyhow::{anyhow, Context, Result};
use corvid_runtime::approval_authorization::ApprovalActorContext;
use corvid_runtime::approval_queue::{
    ApprovalQueueAuditEvent, ApprovalQueueRecord, ApprovalQueueRuntime,
};
use corvid_runtime::auth::SessionAuthRuntime;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct AuthMigrateArgs {
    pub auth_state: PathBuf,
    pub approvals_state: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuthMigrateOutput {
    pub auth_state: PathBuf,
    pub approvals_state: PathBuf,
    pub auth_initialised: bool,
    pub approvals_initialised: bool,
}

/// Open both stores at the supplied paths; both runtimes' `open`
/// constructors invoke `init()` to create tables idempotently. The
/// command is safe to run any number of times.
pub fn run_auth_migrate(args: AuthMigrateArgs) -> Result<AuthMigrateOutput> {
    if let Some(parent) = args.auth_state.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("creating auth state parent `{}`", parent.display())
            })?;
        }
    }
    if let Some(parent) = args.approvals_state.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("creating approvals state parent `{}`", parent.display())
            })?;
        }
    }
    let _auth = SessionAuthRuntime::open(&args.auth_state)
        .map_err(|e| anyhow!("auth runtime init failed: {e}"))?;
    let _approvals = ApprovalQueueRuntime::open(&args.approvals_state)
        .map_err(|e| anyhow!("approvals runtime init failed: {e}"))?;
    Ok(AuthMigrateOutput {
        auth_state: args.auth_state,
        approvals_state: args.approvals_state,
        auth_initialised: true,
        approvals_initialised: true,
    })
}

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

fn summarise(record: ApprovalQueueRecord) -> ApprovalSummary {
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

#[derive(Debug, Clone, PartialEq)]
pub struct AuditEventSummary {
    pub event_kind: String,
    pub status_before: String,
    pub status_after: String,
    pub actor_id: String,
    pub reason: Option<String>,
    pub created_at_ms: u64,
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

fn summarise_audit(event: ApprovalQueueAuditEvent) -> AuditEventSummary {
    AuditEventSummary {
        event_kind: event.event_kind,
        status_before: event.status_before,
        status_after: event.status_after,
        actor_id: event.actor_id,
        reason: event.reason,
        created_at_ms: event.created_ms,
    }
}

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
    use corvid_runtime::approval_queue::{ApprovalContractRecord, ApprovalCreate};
    use tempfile::tempdir;

    fn temp_paths() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempdir().unwrap();
        let auth = dir.path().join("auth.db");
        let approvals = dir.path().join("approvals.db");
        (dir, auth, approvals)
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

    /// Slice 39L: `corvid auth migrate` opens both stores
    /// idempotently. Re-running is a no-op.
    #[test]
    fn migrate_creates_state_files_idempotently() {
        let (_dir, auth, approvals) = temp_paths();
        let out = run_auth_migrate(AuthMigrateArgs {
            auth_state: auth.clone(),
            approvals_state: approvals.clone(),
        })
        .expect("migrate");
        assert!(out.auth_initialised);
        assert!(out.approvals_initialised);
        assert!(auth.exists());
        assert!(approvals.exists());
        // Re-run is a no-op.
        let out2 = run_auth_migrate(AuthMigrateArgs {
            auth_state: auth,
            approvals_state: approvals,
        })
        .expect("re-migrate");
        assert!(out2.auth_initialised);
    }

    /// Slice 39L: `corvid approvals queue` lists by tenant, filters by status.
    #[test]
    fn approvals_queue_filters_by_status() {
        let (_dir, _auth, approvals_state) = temp_paths();
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

    /// Slice 39L: `corvid approvals approve` walks the pending →
    /// approved transition and writes audit.
    #[test]
    fn approvals_approve_transitions_to_approved() {
        let (_dir, _auth, approvals_state) = temp_paths();
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
        let (_dir, _auth, approvals_state) = temp_paths();
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
        let (_dir, _auth, approvals_state) = temp_paths();
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

    /// Slice 39L adversarial: `inspect` for an approval that
    /// belongs to a different tenant fails with a clear message
    /// (no cross-tenant disclosure).
    #[test]
    fn approvals_inspect_rejects_wrong_tenant() {
        let (_dir, _auth, approvals_state) = temp_paths();
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

    /// Slice 39L: `corvid approvals comment` records an audit
    /// event without changing status.
    #[test]
    fn approvals_comment_writes_audit_without_status_change() {
        let (_dir, _auth, approvals_state) = temp_paths();
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
        let (_dir, _auth, approvals_state) = temp_paths();
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
        // ap-2 fails because actor's role is Admin, not Reviewer.
        // ap-missing fails because the id does not exist.
        let failed_ids: Vec<&str> = out.failed.iter().map(|f| f.approval_id.as_str()).collect();
        assert!(failed_ids.contains(&"ap-2"));
        assert!(failed_ids.contains(&"ap-missing"));
    }

    /// Slice 39L: `corvid approvals export` emits the typed
    /// approval + audit transcript a compliance review consumes.
    #[test]
    fn approvals_export_emits_record_with_audit_trail() {
        let (_dir, _auth, approvals_state) = temp_paths();
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
