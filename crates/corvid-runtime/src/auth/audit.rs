//! Auth audit-event ID synthesis + the `auth_audit_events` row
//! reader.
//!
//! `stable_suffix` produces a deterministic 16-hex digest from
//! the event kind plus the session and trace ids the event
//! pertains to — used by `insert_audit` to mint stable
//! `auth_audit_<ts>_<digest>` ids without per-process counters.
//! `read_audit_row` is the SQLite row → `AuthAuditEvent`
//! deserializer the audit listing query consumes.

use sha2::{Digest, Sha256};

use super::AuthAuditEvent;

pub(super) fn stable_suffix(
    event_kind: &str,
    session_id: Option<&str>,
    trace_id: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(event_kind.as_bytes());
    hasher.update(b":");
    hasher.update(session_id.unwrap_or("").as_bytes());
    hasher.update(b":");
    hasher.update(trace_id.unwrap_or("").as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    digest[..16].to_string()
}

pub(super) fn read_audit_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuthAuditEvent> {
    Ok(AuthAuditEvent {
        id: row.get(0)?,
        event_kind: row.get(1)?,
        actor_id: row.get(2)?,
        tenant_id: row.get(3)?,
        session_id: row.get(4)?,
        api_key_id: row.get(5)?,
        trace_id: row.get(6)?,
        status: row.get(7)?,
        reason: row.get(8)?,
        created_ms: row.get::<_, i64>(9)? as u64,
    })
}
