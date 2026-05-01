//! Session token hashing + the SQLite row readers for the
//! `auth_actors` and `auth_sessions` tables.
//!
//! Session tokens use a SHA-256 prefix-keyed hash rather than the
//! Argon2id used for API keys. Sessions live minutes-to-hours and
//! are validated on every request, so the cheaper hash keeps the
//! per-resolve latency low; the brute-force window is bounded by
//! the session's short TTL anyway.
//!
//! `read_actor_row` and `read_session_row` are the typed-record
//! deserializers the impl methods on `SessionAuthRuntime` use to
//! return `AuthActor` and `SessionRecord` from query rows.

use sha2::{Digest, Sha256};

use super::{AuthActor, SessionRecord};

pub fn hash_session_secret(raw_token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"corvid-auth-session-v1:");
    hasher.update(raw_token.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

pub(super) fn read_actor_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuthActor> {
    Ok(AuthActor {
        id: row.get(0)?,
        tenant_id: row.get(1)?,
        display_name: row.get(2)?,
        actor_kind: row.get(3)?,
        auth_method: row.get(4)?,
        assurance_level: row.get(5)?,
        role_fingerprint: row.get(6)?,
        permission_fingerprint: row.get(7)?,
        created_ms: row.get::<_, i64>(8)? as u64,
        updated_ms: row.get::<_, i64>(9)? as u64,
    })
}

pub(super) fn read_session_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    Ok(SessionRecord {
        id: row.get(0)?,
        actor_id: row.get(1)?,
        tenant_id: row.get(2)?,
        token_hash: row.get(3)?,
        issued_ms: row.get::<_, i64>(4)? as u64,
        expires_ms: row.get::<_, i64>(5)? as u64,
        rotation_counter: row.get::<_, i64>(6)? as u64,
        csrf_binding_id: row.get(7)?,
        revoked_ms: row.get::<_, Option<i64>>(8)?.map(|value| value as u64),
        created_ms: row.get::<_, i64>(9)? as u64,
        updated_ms: row.get::<_, i64>(10)? as u64,
    })
}
