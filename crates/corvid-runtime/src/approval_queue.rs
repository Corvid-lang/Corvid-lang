use crate::approval_authorization::{
    authorize_approval_delegate_target, authorize_approval_transition, ApprovalActorContext,
    ApprovalTransitionKind,
};
use crate::approval_policy::validate_approval_contract_policy_at;
use crate::errors::RuntimeError;
use crate::tracing::now_ms;
use rusqlite::Connection;
use rusqlite::{params, OptionalExtension};
use std::sync::Mutex;

mod audit;
mod records;
mod sqlite;

pub use records::{
    ApprovalAuditCoverage, ApprovalContractRecord, ApprovalCreate, ApprovalQueueAuditEvent,
    ApprovalQueueRecord,
};

use audit::insert_audit_tx;
use sqlite::{read_approval_row, sqlite_error};

pub struct ApprovalQueueRuntime {
    conn: Mutex<Connection>,
}

impl ApprovalQueueRuntime {
    pub fn create(&self, input: ApprovalCreate) -> Result<ApprovalQueueRecord, RuntimeError> {
        validate_create(&input)?;
        if input.tenant_id != input.contract.tenant_id {
            return Err(RuntimeError::Other(
                "approval tenant must match contract tenant".to_string(),
            ));
        }
        let now = now_ms();
        let policy = validate_approval_contract_policy_at(&input.contract, now);
        if !policy.valid {
            return Err(RuntimeError::Other(format!(
                "approval contract policy violation: {}",
                policy.violations.join(",")
            )));
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(sqlite_error)?;
        tx.execute(
            "insert into approval_contracts
             (id, version, action, target_kind, target_id, tenant_id, required_role, max_cost_usd, data_class, irreversible, expires_ms, replay_key, created_ms)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             on conflict(id, version) do update set
               action = excluded.action,
               target_kind = excluded.target_kind,
               target_id = excluded.target_id,
               tenant_id = excluded.tenant_id,
               required_role = excluded.required_role,
               max_cost_usd = excluded.max_cost_usd,
               data_class = excluded.data_class,
               irreversible = excluded.irreversible,
               expires_ms = excluded.expires_ms,
               replay_key = excluded.replay_key",
            params![
                input.contract.id,
                input.contract.version,
                input.contract.action,
                input.contract.target_kind,
                input.contract.target_id,
                input.contract.tenant_id,
                input.contract.required_role,
                input.contract.max_cost_usd,
                input.contract.data_class,
                input.contract.irreversible as i64,
                input.contract.expires_ms as i64,
                input.contract.replay_key,
                now as i64,
            ],
        )
        .map_err(sqlite_error)?;
        tx.execute(
            "insert into approval_queue
             (id, tenant_id, requester_actor_id, approver_actor_id, delegated_to_actor_id, contract_id, contract_version, action, target_kind, target_id, status, required_role, risk_level, data_class, irreversible, max_cost_usd, expires_ms, trace_id, replay_key, created_ms, updated_ms)
             values (?1, ?2, ?3, null, null, ?4, ?5, ?6, ?7, ?8, 'pending', ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?17)",
            params![
                input.id,
                input.tenant_id,
                input.requester_actor_id,
                input.contract.id,
                input.contract.version,
                input.contract.action,
                input.contract.target_kind,
                input.contract.target_id,
                input.contract.required_role,
                input.risk_level,
                input.contract.data_class,
                input.contract.irreversible as i64,
                input.contract.max_cost_usd,
                input.contract.expires_ms as i64,
                input.trace_id,
                input.contract.replay_key,
                now as i64,
            ],
        )
        .map_err(sqlite_error)?;
        insert_audit_tx(
            &tx,
            &input.id,
            &input.tenant_id,
            &input.requester_actor_id,
            "created",
            "",
            "pending",
            Some("approval created"),
            &input.trace_id,
            now,
        )?;
        tx.commit().map_err(sqlite_error)?;
        drop(conn);
        self.get(&input.id)?
            .ok_or_else(|| RuntimeError::Other(format!("approval `{}` not found", input.id)))
    }

    pub fn get(&self, id: &str) -> Result<Option<ApprovalQueueRecord>, RuntimeError> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "select id, tenant_id, requester_actor_id, approver_actor_id, delegated_to_actor_id, contract_id, contract_version, action, target_kind, target_id, status, required_role, risk_level, data_class, irreversible, max_cost_usd, expires_ms, trace_id, replay_key, created_ms, updated_ms
                 from approval_queue where id = ?1",
                params![id],
                read_approval_row,
            )
            .optional()
            .map_err(sqlite_error)
    }

    pub fn list_by_tenant(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<ApprovalQueueRecord>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select id, tenant_id, requester_actor_id, approver_actor_id, delegated_to_actor_id, contract_id, contract_version, action, target_kind, target_id, status, required_role, risk_level, data_class, irreversible, max_cost_usd, expires_ms, trace_id, replay_key, created_ms, updated_ms
                 from approval_queue where tenant_id = ?1 order by created_ms, id",
            )
            .map_err(sqlite_error)?;
        let rows = stmt
            .query_map(params![tenant_id], read_approval_row)
            .map_err(sqlite_error)?;
        let mut approvals = Vec::new();
        for row in rows {
            approvals.push(row.map_err(sqlite_error)?);
        }
        Ok(approvals)
    }

    pub fn approve(
        &self,
        id: &str,
        tenant_id: &str,
        actor: &ApprovalActorContext,
        reason: Option<&str>,
    ) -> Result<ApprovalQueueRecord, RuntimeError> {
        self.approve_at(id, tenant_id, actor, reason, now_ms())
    }

    pub fn approve_at(
        &self,
        id: &str,
        tenant_id: &str,
        actor: &ApprovalActorContext,
        reason: Option<&str>,
        at_ms: u64,
    ) -> Result<ApprovalQueueRecord, RuntimeError> {
        self.transition_status_at(
            id,
            tenant_id,
            actor,
            ApprovalTransitionKind::Approve,
            "approved",
            "approved",
            reason,
            at_ms,
        )
    }

    pub fn deny(
        &self,
        id: &str,
        tenant_id: &str,
        actor: &ApprovalActorContext,
        reason: Option<&str>,
    ) -> Result<ApprovalQueueRecord, RuntimeError> {
        self.transition_status(
            id,
            tenant_id,
            actor,
            ApprovalTransitionKind::Deny,
            "denied",
            "denied",
            reason,
        )
    }

    pub fn expire(
        &self,
        id: &str,
        tenant_id: &str,
        actor_id: &str,
        reason: Option<&str>,
        at_ms: u64,
    ) -> Result<ApprovalQueueRecord, RuntimeError> {
        let existing = self.require_pending(id, tenant_id)?;
        if at_ms < existing.expires_ms {
            return Err(RuntimeError::Other(
                "approval cannot expire before contract expiry".to_string(),
            ));
        }
        self.transition_expired_at(id, tenant_id, actor_id, reason, at_ms)
    }

    pub fn comment(
        &self,
        id: &str,
        tenant_id: &str,
        actor_id: &str,
        comment: &str,
    ) -> Result<ApprovalQueueAuditEvent, RuntimeError> {
        validate_value("comment", comment)?;
        let existing = self.require_tenant(id, tenant_id)?;
        let now = now_ms();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(sqlite_error)?;
        insert_audit_tx(
            &tx,
            id,
            tenant_id,
            actor_id,
            "commented",
            &existing.status,
            &existing.status,
            Some(comment),
            &existing.trace_id,
            now,
        )?;
        tx.commit().map_err(sqlite_error)?;
        drop(conn);
        self.audit_events(id)?
            .into_iter()
            .rev()
            .find(|event| event.event_kind == "commented")
            .ok_or_else(|| RuntimeError::Other(format!("approval `{id}` comment audit not found")))
    }

    pub fn delegate(
        &self,
        id: &str,
        tenant_id: &str,
        actor: &ApprovalActorContext,
        delegated_to_actor_id: &str,
        reason: Option<&str>,
    ) -> Result<ApprovalQueueRecord, RuntimeError> {
        validate_value("delegated actor id", delegated_to_actor_id)?;
        let existing = self.require_pending(id, tenant_id)?;
        authorize_approval_transition(
            &existing,
            actor,
            ApprovalTransitionKind::Delegate,
            now_ms(),
        )?;
        authorize_approval_delegate_target(&existing, delegated_to_actor_id)?;
        let now = now_ms();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(sqlite_error)?;
        tx.execute(
            "update approval_queue
             set delegated_to_actor_id = ?3, updated_ms = ?4
             where id = ?1 and tenant_id = ?2 and status = 'pending'",
            params![id, tenant_id, delegated_to_actor_id, now as i64],
        )
        .map_err(sqlite_error)?;
        insert_audit_tx(
            &tx,
            id,
            tenant_id,
            &actor.actor_id,
            "delegated",
            &existing.status,
            &existing.status,
            reason,
            &existing.trace_id,
            now,
        )?;
        tx.commit().map_err(sqlite_error)?;
        drop(conn);
        self.get(id)?
            .ok_or_else(|| RuntimeError::Other(format!("approval `{id}` not found")))
    }

    fn transition_status(
        &self,
        id: &str,
        tenant_id: &str,
        actor: &ApprovalActorContext,
        transition: ApprovalTransitionKind,
        event_kind: &str,
        status_after: &str,
        reason: Option<&str>,
    ) -> Result<ApprovalQueueRecord, RuntimeError> {
        self.transition_status_at(
            id,
            tenant_id,
            actor,
            transition,
            event_kind,
            status_after,
            reason,
            now_ms(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn transition_status_at(
        &self,
        id: &str,
        tenant_id: &str,
        actor: &ApprovalActorContext,
        transition: ApprovalTransitionKind,
        event_kind: &str,
        status_after: &str,
        reason: Option<&str>,
        at_ms: u64,
    ) -> Result<ApprovalQueueRecord, RuntimeError> {
        validate_value("approval id", id)?;
        validate_value("tenant id", tenant_id)?;
        let existing = self.require_pending(id, tenant_id)?;
        authorize_approval_transition(&existing, actor, transition, at_ms)?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(sqlite_error)?;
        tx.execute(
            "update approval_queue
             set status = ?4, approver_actor_id = ?3, updated_ms = ?5
             where id = ?1 and tenant_id = ?2 and status = 'pending'",
            params![id, tenant_id, actor.actor_id, status_after, at_ms as i64],
        )
        .map_err(sqlite_error)?;
        insert_audit_tx(
            &tx,
            id,
            tenant_id,
            &actor.actor_id,
            event_kind,
            &existing.status,
            status_after,
            reason,
            &existing.trace_id,
            at_ms,
        )?;
        tx.commit().map_err(sqlite_error)?;
        drop(conn);
        self.get(id)?
            .ok_or_else(|| RuntimeError::Other(format!("approval `{id}` not found")))
    }

    fn transition_expired_at(
        &self,
        id: &str,
        tenant_id: &str,
        actor_id: &str,
        reason: Option<&str>,
        at_ms: u64,
    ) -> Result<ApprovalQueueRecord, RuntimeError> {
        validate_value("approval id", id)?;
        validate_value("tenant id", tenant_id)?;
        validate_value("actor id", actor_id)?;
        let existing = self.require_pending(id, tenant_id)?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(sqlite_error)?;
        tx.execute(
            "update approval_queue
             set status = 'expired', approver_actor_id = ?3, updated_ms = ?4
             where id = ?1 and tenant_id = ?2 and status = 'pending'",
            params![id, tenant_id, actor_id, at_ms as i64],
        )
        .map_err(sqlite_error)?;
        insert_audit_tx(
            &tx,
            id,
            tenant_id,
            actor_id,
            "expired",
            &existing.status,
            "expired",
            reason,
            &existing.trace_id,
            at_ms,
        )?;
        tx.commit().map_err(sqlite_error)?;
        drop(conn);
        self.get(id)?
            .ok_or_else(|| RuntimeError::Other(format!("approval `{id}` not found")))
    }

    fn require_tenant(
        &self,
        id: &str,
        tenant_id: &str,
    ) -> Result<ApprovalQueueRecord, RuntimeError> {
        validate_value("approval id", id)?;
        validate_value("tenant id", tenant_id)?;
        let approval = self
            .get(id)?
            .ok_or_else(|| RuntimeError::Other(format!("approval `{id}` not found")))?;
        if approval.tenant_id != tenant_id {
            return Err(RuntimeError::Other("approval tenant mismatch".to_string()));
        }
        Ok(approval)
    }

    fn require_pending(
        &self,
        id: &str,
        tenant_id: &str,
    ) -> Result<ApprovalQueueRecord, RuntimeError> {
        let approval = self.require_tenant(id, tenant_id)?;
        if approval.status != "pending" {
            return Err(RuntimeError::Other(format!(
                "approval `{id}` is not pending"
            )));
        }
        Ok(approval)
    }
}

fn validate_create(input: &ApprovalCreate) -> Result<(), RuntimeError> {
    for (label, value) in [
        ("approval id", input.id.as_str()),
        ("tenant id", input.tenant_id.as_str()),
        ("requester actor id", input.requester_actor_id.as_str()),
        ("contract id", input.contract.id.as_str()),
        ("contract version", input.contract.version.as_str()),
        ("action", input.contract.action.as_str()),
        ("target kind", input.contract.target_kind.as_str()),
        ("target id", input.contract.target_id.as_str()),
        ("required role", input.contract.required_role.as_str()),
        ("risk level", input.risk_level.as_str()),
        ("trace id", input.trace_id.as_str()),
        ("replay key", input.contract.replay_key.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(RuntimeError::Other(format!("{label} must not be empty")));
        }
    }
    if !input.contract.max_cost_usd.is_finite() || input.contract.max_cost_usd < 0.0 {
        return Err(RuntimeError::Other(
            "approval max cost must be finite and non-negative".to_string(),
        ));
    }
    Ok(())
}

fn validate_value(label: &str, value: &str) -> Result<(), RuntimeError> {
    if value.trim().is_empty() {
        Err(RuntimeError::Other(format!("{label} must not be empty")))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract(expires_ms: u64) -> ApprovalContractRecord {
        ApprovalContractRecord {
            id: "contract-1".to_string(),
            version: "v1".to_string(),
            action: "SendExecutiveFollowUp".to_string(),
            target_kind: "email_thread".to_string(),
            target_id: "thread-1".to_string(),
            tenant_id: "org-1".to_string(),
            required_role: "Reviewer".to_string(),
            max_cost_usd: 0.25,
            data_class: "private".to_string(),
            irreversible: true,
            expires_ms,
            replay_key: "replay-approval-1".to_string(),
            created_ms: 0,
        }
    }

    fn reviewer(id: &str) -> ApprovalActorContext {
        ApprovalActorContext {
            actor_id: id.to_string(),
            tenant_id: "org-1".to_string(),
            role: "Reviewer".to_string(),
        }
    }

    fn actor(id: &str, tenant_id: &str, role: &str) -> ApprovalActorContext {
        ApprovalActorContext {
            actor_id: id.to_string(),
            tenant_id: tenant_id.to_string(),
            role: role.to_string(),
        }
    }

    fn queued_approval(queue: &ApprovalQueueRuntime, id: &str) -> ApprovalQueueRecord {
        queue
            .create(ApprovalCreate {
                id: id.to_string(),
                tenant_id: "org-1".to_string(),
                requester_actor_id: "user-1".to_string(),
                contract: contract(now_ms().saturating_add(60_000)),
                risk_level: "external_side_effect".to_string(),
                trace_id: format!("trace-{id}"),
            })
            .unwrap()
    }

    #[test]
    fn approval_store_persists_contract_queue_item_and_create_audit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("approvals.sqlite");
        let approval_id = {
            let queue = ApprovalQueueRuntime::open(&path).unwrap();
            let approval = queue
                .create(ApprovalCreate {
                    id: "approval-1".to_string(),
                    tenant_id: "org-1".to_string(),
                    requester_actor_id: "user-1".to_string(),
                    contract: contract(now_ms().saturating_add(60_000)),
                    risk_level: "external_side_effect".to_string(),
                    trace_id: "trace-1".to_string(),
                })
                .unwrap();
            assert_eq!(approval.status, "pending");
            assert_eq!(approval.required_role, "Reviewer");
            assert!(approval.irreversible);
            approval.id
        };

        let queue = ApprovalQueueRuntime::open(&path).unwrap();
        let approval = queue.get(&approval_id).unwrap().expect("approval");
        assert_eq!(approval.action, "SendExecutiveFollowUp");
        assert_eq!(approval.tenant_id, "org-1");
        let approvals = queue.list_by_tenant("org-1").unwrap();
        assert_eq!(approvals.len(), 1);
        let audit = queue.audit_events(&approval_id).unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].event_kind, "created");
        assert_eq!(audit[0].status_after, "pending");
        assert_eq!(audit[0].trace_id, "trace-1");
    }

    #[test]
    fn approval_store_rejects_cross_tenant_or_expired_contracts() {
        let queue = ApprovalQueueRuntime::open_in_memory().unwrap();
        let mut cross = contract(now_ms().saturating_add(60_000));
        cross.tenant_id = "org-2".to_string();
        let err = queue
            .create(ApprovalCreate {
                id: "approval-cross".to_string(),
                tenant_id: "org-1".to_string(),
                requester_actor_id: "user-1".to_string(),
                contract: cross,
                risk_level: "high".to_string(),
                trace_id: "trace-1".to_string(),
            })
            .unwrap_err();
        assert!(err.to_string().contains("tenant"));

        let err = queue
            .create(ApprovalCreate {
                id: "approval-expired".to_string(),
                tenant_id: "org-1".to_string(),
                requester_actor_id: "user-1".to_string(),
                contract: contract(1),
                risk_level: "high".to_string(),
                trace_id: "trace-1".to_string(),
            })
            .unwrap_err();
        assert!(err.to_string().contains("expired_contract"));
    }

    #[test]
    fn approval_store_enforces_contract_policy_before_queueing() {
        let queue = ApprovalQueueRuntime::open_in_memory().unwrap();
        let mut weak = contract(now_ms().saturating_add(60_000));
        weak.required_role = "Member".to_string();
        weak.data_class = "unknown".to_string();
        let err = queue
            .create(ApprovalCreate {
                id: "approval-weak".to_string(),
                tenant_id: "org-1".to_string(),
                requester_actor_id: "user-1".to_string(),
                contract: weak,
                risk_level: "external_side_effect".to_string(),
                trace_id: "trace-weak".to_string(),
            })
            .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("irreversible_requires_elevated_role"));
        assert!(message.contains("irreversible_requires_known_data_class"));
        assert_eq!(queue.list_by_tenant("org-1").unwrap().len(), 0);
    }

    #[test]
    fn approval_api_transitions_comment_and_delegate_work_transactionally() {
        let queue = ApprovalQueueRuntime::open_in_memory().unwrap();
        let approval = queue
            .create(ApprovalCreate {
                id: "approval-1".to_string(),
                tenant_id: "org-1".to_string(),
                requester_actor_id: "user-1".to_string(),
                contract: contract(now_ms().saturating_add(60_000)),
                risk_level: "external_side_effect".to_string(),
                trace_id: "trace-1".to_string(),
            })
            .unwrap();

        let comment = queue
            .comment(
                &approval.id,
                "org-1",
                "reviewer-1",
                "looks safe after redaction",
            )
            .unwrap();
        assert_eq!(comment.event_kind, "commented");
        assert_eq!(comment.status_before, "pending");
        assert_eq!(comment.status_after, "pending");

        let delegated = queue
            .delegate(
                &approval.id,
                "org-1",
                &reviewer("reviewer-1"),
                "reviewer-2",
                Some("handoff to owner"),
            )
            .unwrap();
        assert_eq!(delegated.status, "pending");
        assert_eq!(
            delegated.delegated_to_actor_id.as_deref(),
            Some("reviewer-2")
        );

        let approved = queue
            .approve(
                &approval.id,
                "org-1",
                &reviewer("reviewer-2"),
                Some("approved"),
            )
            .unwrap();
        assert_eq!(approved.status, "approved");
        assert_eq!(approved.approver_actor_id.as_deref(), Some("reviewer-2"));
        assert!(queue
            .deny(
                &approval.id,
                "org-1",
                &reviewer("reviewer-3"),
                Some("too late"),
            )
            .unwrap_err()
            .to_string()
            .contains("not pending"));

        let audit = queue.audit_events(&approval.id).unwrap();
        assert_eq!(
            audit
                .iter()
                .map(|event| event.event_kind.as_str())
                .collect::<Vec<_>>(),
            vec!["created", "commented", "delegated", "approved"]
        );
    }

    #[test]
    fn approval_api_denies_and_expires_fail_closed() {
        let queue = ApprovalQueueRuntime::open_in_memory().unwrap();
        let denial = queue
            .create(ApprovalCreate {
                id: "approval-deny".to_string(),
                tenant_id: "org-1".to_string(),
                requester_actor_id: "user-1".to_string(),
                contract: contract(now_ms().saturating_add(60_000)),
                risk_level: "external_side_effect".to_string(),
                trace_id: "trace-deny".to_string(),
            })
            .unwrap();
        let denied = queue
            .deny(
                &denial.id,
                "org-1",
                &reviewer("reviewer-1"),
                Some("insufficient context"),
            )
            .unwrap();
        assert_eq!(denied.status, "denied");

        let expires_ms = now_ms().saturating_add(1_000);
        let expiring = queue
            .create(ApprovalCreate {
                id: "approval-expire".to_string(),
                tenant_id: "org-1".to_string(),
                requester_actor_id: "user-1".to_string(),
                contract: contract(expires_ms),
                risk_level: "external_side_effect".to_string(),
                trace_id: "trace-expire".to_string(),
            })
            .unwrap();
        assert!(queue
            .expire(
                &expiring.id,
                "org-1",
                "system",
                Some("too early"),
                expires_ms - 1,
            )
            .unwrap_err()
            .to_string()
            .contains("before contract expiry"));
        let expired = queue
            .expire(
                &expiring.id,
                "org-1",
                "system",
                Some("deadline passed"),
                expires_ms,
            )
            .unwrap();
        assert_eq!(expired.status, "expired");

        assert!(queue
            .approve(&expiring.id, "org-2", &reviewer("reviewer-1"), None)
            .unwrap_err()
            .to_string()
            .contains("tenant"));
    }

    #[test]
    fn approval_audit_coverage_proves_trace_linked_transition_evidence() {
        let queue = ApprovalQueueRuntime::open_in_memory().unwrap();
        let approval = queue
            .create(ApprovalCreate {
                id: "approval-audit".to_string(),
                tenant_id: "org-1".to_string(),
                requester_actor_id: "user-1".to_string(),
                contract: contract(now_ms().saturating_add(60_000)),
                risk_level: "external_side_effect".to_string(),
                trace_id: "trace-audit".to_string(),
            })
            .unwrap();
        let pending = queue.audit_coverage(&approval.id).unwrap();
        assert!(pending.complete);
        assert_eq!(pending.current_status, "pending");
        assert_eq!(pending.event_count, 1);

        queue
            .comment(&approval.id, "org-1", "reviewer-1", "needs owner review")
            .unwrap();
        queue
            .delegate(
                &approval.id,
                "org-1",
                &reviewer("reviewer-1"),
                "reviewer-2",
                Some("owner review"),
            )
            .unwrap();
        queue
            .deny(
                &approval.id,
                "org-1",
                &reviewer("reviewer-2"),
                Some("unsafe target"),
            )
            .unwrap();

        let coverage = queue.audit_coverage(&approval.id).unwrap();
        assert!(coverage.complete);
        assert!(coverage.has_create);
        assert!(coverage.has_terminal_transition);
        assert_eq!(coverage.current_status, "denied");
        assert_eq!(coverage.trace_id, "trace-audit");
        assert_eq!(coverage.event_count, 4);
        let events = queue.audit_events(&approval.id).unwrap();
        assert!(events.iter().all(|event| event.trace_id == "trace-audit"));
        assert!(events.iter().all(|event| event.tenant_id == "org-1"));
        assert!(events.iter().all(|event| event.approval_id == approval.id));
    }

    #[test]
    fn approval_bypass_rejects_confused_deputy_self_approval() {
        let queue = ApprovalQueueRuntime::open_in_memory().unwrap();
        let approval = queued_approval(&queue, "approval-self");
        let requester_with_role = actor("user-1", "org-1", "Reviewer");
        let err = queue
            .approve(
                &approval.id,
                "org-1",
                &requester_with_role,
                Some("self approve"),
            )
            .unwrap_err();
        assert!(err.to_string().contains("requester"));
        assert_eq!(queue.get(&approval.id).unwrap().unwrap().status, "pending");
    }

    #[test]
    fn approval_bypass_rejects_tenant_crossing_actor() {
        let queue = ApprovalQueueRuntime::open_in_memory().unwrap();
        let approval = queued_approval(&queue, "approval-tenant");
        let cross_tenant_reviewer = actor("reviewer-1", "org-2", "Reviewer");
        let err = queue
            .approve(
                &approval.id,
                "org-1",
                &cross_tenant_reviewer,
                Some("wrong tenant"),
            )
            .unwrap_err();
        assert!(err.to_string().contains("tenant mismatch"));
        assert_eq!(queue.get(&approval.id).unwrap().unwrap().status, "pending");
    }

    #[test]
    fn approval_bypass_rejects_stale_approval_replay() {
        let queue = ApprovalQueueRuntime::open_in_memory().unwrap();
        let expires_ms = now_ms().saturating_add(60_000);
        let approval = queue
            .create(ApprovalCreate {
                id: "approval-stale".to_string(),
                tenant_id: "org-1".to_string(),
                requester_actor_id: "user-1".to_string(),
                contract: contract(expires_ms),
                risk_level: "external_side_effect".to_string(),
                trace_id: "trace-stale".to_string(),
            })
            .unwrap();
        let err = queue
            .approve_at(
                &approval.id,
                "org-1",
                &reviewer("reviewer-1"),
                Some("replayed approval"),
                expires_ms,
            )
            .unwrap_err();
        assert!(err.to_string().contains("expired"));
        assert_eq!(queue.get(&approval.id).unwrap().unwrap().status, "pending");
    }

    #[test]
    fn approval_bypass_rejects_privilege_escalation() {
        let queue = ApprovalQueueRuntime::open_in_memory().unwrap();
        let approval = queued_approval(&queue, "approval-role");
        let member = actor("member-1", "org-1", "Member");
        let err = queue
            .approve(&approval.id, "org-1", &member, Some("role escalation"))
            .unwrap_err();
        assert!(err.to_string().contains("required role"));
        assert_eq!(queue.get(&approval.id).unwrap().unwrap().status, "pending");
    }
}
