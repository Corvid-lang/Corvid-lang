use super::{sqlite_error, ApprovalAuditCoverage, ApprovalQueueAuditEvent, ApprovalQueueRuntime};
use crate::errors::RuntimeError;
use rusqlite::params;
impl ApprovalQueueRuntime {
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

    pub fn audit_coverage(&self, id: &str) -> Result<ApprovalAuditCoverage, RuntimeError> {
        let approval = self
            .get(id)?
            .ok_or_else(|| RuntimeError::Other(format!("approval `{id}` not found")))?;
        let events = self.audit_events(id)?;
        let has_create = events.iter().any(|event| {
            event.event_kind == "created"
                && event.status_before.is_empty()
                && event.status_after == "pending"
                && event.trace_id == approval.trace_id
                && event.tenant_id == approval.tenant_id
        });
        let has_terminal_transition = match approval.status.as_str() {
            "approved" | "denied" | "expired" => events.iter().any(|event| {
                event.status_before == "pending"
                    && event.status_after == approval.status
                    && event.trace_id == approval.trace_id
                    && event.tenant_id == approval.tenant_id
            }),
            "pending" => true,
            _ => false,
        };
        let all_trace_linked = events.iter().all(|event| {
            event.approval_id == approval.id
                && event.tenant_id == approval.tenant_id
                && event.trace_id == approval.trace_id
                && !event.actor_id.trim().is_empty()
                && !event.event_kind.trim().is_empty()
        });
        Ok(ApprovalAuditCoverage {
            approval_id: approval.id,
            tenant_id: approval.tenant_id,
            trace_id: approval.trace_id,
            current_status: approval.status,
            event_count: events.len(),
            has_create,
            has_terminal_transition,
            complete: has_create && has_terminal_transition && all_trace_linked,
        })
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn insert_audit_tx(
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
