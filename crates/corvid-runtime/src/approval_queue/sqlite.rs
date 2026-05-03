use super::{ApprovalQueueRecord, ApprovalQueueRuntime};
use crate::errors::RuntimeError;
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

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
}

pub(super) fn read_approval_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ApprovalQueueRecord> {
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

pub(super) fn sqlite_error(err: rusqlite::Error) -> RuntimeError {
    RuntimeError::Other(format!("approval queue sqlite error: {err}"))
}
