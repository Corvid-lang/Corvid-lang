//! OAuth state-token hashing + the `auth_oauth_states` row reader.
//!
//! OAuth callbacks land back at the runtime carrying the state
//! token the client sent at authorize time. The state is hashed
//! with SHA-256 (same family as session tokens, with a different
//! prefix to keep the hash spaces distinct) so a leaked database
//! row never lets an attacker forge a future callback.
//!
//! `read_oauth_state_row` is the SQLite row → `OAuthStateRecord`
//! deserializer the impl methods on `SessionAuthRuntime` use to
//! return typed records from the `auth_oauth_states` table.

use sha2::{Digest, Sha256};

use super::OAuthStateRecord;

pub fn hash_oauth_state(raw_state: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"corvid-auth-oauth-state-v1:");
    hasher.update(raw_state.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

pub(super) fn read_oauth_state_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OAuthStateRecord> {
    Ok(OAuthStateRecord {
        id: row.get(0)?,
        provider: row.get(1)?,
        tenant_id: row.get(2)?,
        actor_id: row.get(3)?,
        state_hash: row.get(4)?,
        pkce_verifier_ref: row.get(5)?,
        nonce_fingerprint: row.get(6)?,
        expires_ms: row.get::<_, i64>(7)? as u64,
        replay_key: row.get(8)?,
        used_ms: row.get::<_, Option<i64>>(9)?.map(|value| value as u64),
        created_ms: row.get::<_, i64>(10)? as u64,
        updated_ms: row.get::<_, i64>(11)? as u64,
    })
}
