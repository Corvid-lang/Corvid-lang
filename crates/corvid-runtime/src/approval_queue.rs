use crate::errors::RuntimeError;
use crate::tracing::now_ms;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalContractRecord {
    pub id: String,
    pub version: String,
    pub action: String,
    pub target_kind: String,
    pub target_id: String,
    pub tenant_id: String,
    pub required_role: String,
    pub max_cost_usd: f64,
    pub data_class: String,
    pub irreversible: bool,
    pub expires_ms: u64,
    pub replay_key: String,
    pub created_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalCreate {
    pub id: String,
    pub tenant_id: String,
    pub requester_actor_id: String,
    pub contract: ApprovalContractRecord,
    pub risk_level: String,
    pub trace_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalQueueRecord {
    pub id: String,
    pub tenant_id: String,
    pub requester_actor_id: String,
    pub approver_actor_id: Option<String>,
    pub delegated_to_actor_id: Option<String>,
    pub contract_id: String,
    pub contract_version: String,
    pub action: String,
    pub target_kind: String,
    pub target_id: String,
    pub status: String,
    pub required_role: String,
    pub risk_level: String,
    pub data_class: String,
    pub irreversible: bool,
    pub max_cost_usd: f64,
    pub expires_ms: u64,
    pub trace_id: String,
    pub replay_key: String,
    pub created_ms: u64,
    pub updated_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalQueueAuditEvent {
    pub id: String,
    pub approval_id: String,
    pub tenant_id: String,
    pub actor_id: String,
    pub event_kind: String,
    pub status_before: String,
    pub status_after: String,
    pub reason: Option<String>,
    pub trace_id: String,
    pub created_ms: u64,
}

pub struct ApprovalQueueRuntime {
    conn: Mutex<Connection>,
}

impl ApprovalQueueRuntime {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, RuntimeError> {
        let conn = Connection::open(path.as_ref()).map_err(sqlite_error)?;
        let runtime = Self {
            conn: Mutex::new(conn),
        };
        runtime.init()?;
        Ok(runtime)
    }

    pub fn open_in_memory() -> Result<Self, RuntimeError> {
        let conn = Connection::open_in_memory().map_err(sqlite_error)?;
        let runtime = Self {
            conn: Mutex::new(conn),
        };
        runtime.init()?;
        Ok(runtime)
    }

    pub fn create(&self, input: ApprovalCreate) -> Result<ApprovalQueueRecord, RuntimeError> {
        validate_create(&input)?;
        if input.tenant_id != input.contract.tenant_id {
            return Err(RuntimeError::Other(
                "approval tenant must match contract tenant".to_string(),
            ));
        }
        let now = now_ms();
        if input.contract.expires_ms <= now {
            return Err(RuntimeError::Other(
                "approval contract expiry must be in the future".to_string(),
            ));
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

    pub fn list_by_tenant(&self, tenant_id: &str) -> Result<Vec<ApprovalQueueRecord>, RuntimeError> {
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

    pub fn audit_events(
        &self,
        approval_id: &str,
    ) -> Result<Vec<ApprovalQueueAuditEvent>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select id, approval_id, tenant_id, actor_id, event_kind, status_before, status_after, reason, trace_id, created_ms
                 from approval_queue_audit where approval_id = ?1 order by rowid",
            )
            .map_err(sqlite_error)?;
        let rows = stmt
            .query_map(params![approval_id], read_audit_row)
            .map_err(sqlite_error)?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row.map_err(sqlite_error)?);
        }
        Ok(events)
    }

    pub fn approve(
        &self,
        id: &str,
        tenant_id: &str,
        actor_id: &str,
        reason: Option<&str>,
    ) -> Result<ApprovalQueueRecord, RuntimeError> {
        self.transition_status(id, tenant_id, actor_id, "approved", "approved", reason)
    }

    pub fn deny(
        &self,
        id: &str,
        tenant_id: &str,
        actor_id: &str,
        reason: Option<&str>,
    ) -> Result<ApprovalQueueRecord, RuntimeError> {
        self.transition_status(id, tenant_id, actor_id, "denied", "denied", reason)
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
        self.transition_status_at(id, tenant_id, actor_id, "expired", "expired", reason, at_ms)
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
        actor_id: &str,
        delegated_to_actor_id: &str,
        reason: Option<&str>,
    ) -> Result<ApprovalQueueRecord, RuntimeError> {
        validate_value("delegated actor id", delegated_to_actor_id)?;
        let existing = self.require_pending(id, tenant_id)?;
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
            actor_id,
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

    fn init(&self) -> Result<(), RuntimeError> {
        self.conn
            .lock()
            .unwrap()
            .execute_batch(
                "create table if not exists approval_contracts (
                    id text not null,
                    version text not null,
                    action text not null,
                    target_kind text not null,
                    target_id text not null,
                    tenant_id text not null,
                    required_role text not null,
                    max_cost_usd real not null,
                    data_class text not null,
                    irreversible integer not null,
                    expires_ms integer not null,
                    replay_key text not null,
                    created_ms integer not null,
                    primary key (id, version)
                );
                create index if not exists approval_contracts_tenant on approval_contracts(tenant_id);
                create table if not exists approval_queue (
                    id text primary key,
                    tenant_id text not null,
                    requester_actor_id text not null,
                    approver_actor_id text,
                    delegated_to_actor_id text,
                    contract_id text not null,
                    contract_version text not null,
                    action text not null,
                    target_kind text not null,
                    target_id text not null,
                    status text not null,
                    required_role text not null,
                    risk_level text not null,
                    data_class text not null,
                    irreversible integer not null,
                    max_cost_usd real not null,
                    expires_ms integer not null,
                    trace_id text not null,
                    replay_key text not null,
                    created_ms integer not null,
                    updated_ms integer not null
                );
                create index if not exists approval_queue_tenant_status on approval_queue(tenant_id, status);
                create index if not exists approval_queue_target on approval_queue(tenant_id, target_kind, target_id);
                create table if not exists approval_queue_audit (
                    id text primary key,
                    approval_id text not null,
                    tenant_id text not null,
                    actor_id text not null,
                    event_kind text not null,
                    status_before text not null,
                    status_after text not null,
                    reason text,
                    trace_id text not null,
                    created_ms integer not null
                );
                create index if not exists approval_queue_audit_approval on approval_queue_audit(approval_id);",
            )
            .map_err(sqlite_error)
    }

    fn transition_status(
        &self,
        id: &str,
        tenant_id: &str,
        actor_id: &str,
        event_kind: &str,
        status_after: &str,
        reason: Option<&str>,
    ) -> Result<ApprovalQueueRecord, RuntimeError> {
        self.transition_status_at(
            id,
            tenant_id,
            actor_id,
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
        actor_id: &str,
        event_kind: &str,
        status_after: &str,
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
             set status = ?4, approver_actor_id = ?3, updated_ms = ?5
             where id = ?1 and tenant_id = ?2 and status = 'pending'",
            params![id, tenant_id, actor_id, status_after, at_ms as i64],
        )
        .map_err(sqlite_error)?;
        insert_audit_tx(
            &tx,
            id,
            tenant_id,
            actor_id,
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
            return Err(RuntimeError::Other(
                "approval tenant mismatch".to_string(),
            ));
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

#[allow(clippy::too_many_arguments)]
fn insert_audit_tx(
    tx: &rusqlite::Transaction<'_>,
    approval_id: &str,
    tenant_id: &str,
    actor_id: &str,
    event_kind: &str,
    status_before: &str,
    status_after: &str,
    reason: Option<&str>,
    trace_id: &str,
    created_ms: u64,
) -> Result<(), RuntimeError> {
    let id = format!("{approval_id}:audit:{created_ms}:{event_kind}");
    tx.execute(
        "insert into approval_queue_audit
         (id, approval_id, tenant_id, actor_id, event_kind, status_before, status_after, reason, trace_id, created_ms)
         values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            id,
            approval_id,
            tenant_id,
            actor_id,
            event_kind,
            status_before,
            status_after,
            reason,
            trace_id,
            created_ms as i64,
        ],
    )
    .map_err(sqlite_error)?;
    Ok(())
}

fn read_approval_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ApprovalQueueRecord> {
    Ok(ApprovalQueueRecord {
        id: row.get(0)?,
        tenant_id: row.get(1)?,
        requester_actor_id: row.get(2)?,
        approver_actor_id: row.get(3)?,
        delegated_to_actor_id: row.get(4)?,
        contract_id: row.get(5)?,
        contract_version: row.get(6)?,
        action: row.get(7)?,
        target_kind: row.get(8)?,
        target_id: row.get(9)?,
        status: row.get(10)?,
        required_role: row.get(11)?,
        risk_level: row.get(12)?,
        data_class: row.get(13)?,
        irreversible: row.get::<_, i64>(14)? != 0,
        max_cost_usd: row.get(15)?,
        expires_ms: row.get::<_, i64>(16)? as u64,
        trace_id: row.get(17)?,
        replay_key: row.get(18)?,
        created_ms: row.get::<_, i64>(19)? as u64,
        updated_ms: row.get::<_, i64>(20)? as u64,
    })
}

fn read_audit_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ApprovalQueueAuditEvent> {
    Ok(ApprovalQueueAuditEvent {
        id: row.get(0)?,
        approval_id: row.get(1)?,
        tenant_id: row.get(2)?,
        actor_id: row.get(3)?,
        event_kind: row.get(4)?,
        status_before: row.get(5)?,
        status_after: row.get(6)?,
        reason: row.get(7)?,
        trace_id: row.get(8)?,
        created_ms: row.get::<_, i64>(9)? as u64,
    })
}

fn sqlite_error(err: rusqlite::Error) -> RuntimeError {
    RuntimeError::Other(format!("approval queue sqlite error: {err}"))
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
        assert!(err.to_string().contains("expiry"));
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
            .comment(&approval.id, "org-1", "reviewer-1", "looks safe after redaction")
            .unwrap();
        assert_eq!(comment.event_kind, "commented");
        assert_eq!(comment.status_before, "pending");
        assert_eq!(comment.status_after, "pending");

        let delegated = queue
            .delegate(
                &approval.id,
                "org-1",
                "reviewer-1",
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
            .approve(&approval.id, "org-1", "reviewer-2", Some("approved"))
            .unwrap();
        assert_eq!(approved.status, "approved");
        assert_eq!(approved.approver_actor_id.as_deref(), Some("reviewer-2"));
        assert!(queue
            .deny(&approval.id, "org-1", "reviewer-3", Some("too late"))
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
            .deny(&denial.id, "org-1", "reviewer-1", Some("insufficient context"))
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
            .approve(&expiring.id, "org-2", "reviewer-1", None)
            .unwrap_err()
            .to_string()
            .contains("tenant"));
    }
}
