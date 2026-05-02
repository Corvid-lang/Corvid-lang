//! Session domain — token hashing, row decoding, and the
//! `SessionAuthRuntime` impl block carrying create / get /
//! resolve / rotate / revoke for `auth_sessions`.
//!
//! Session tokens use a SHA-256 prefix-keyed hash rather than the
//! Argon2id used for API keys. Sessions live minutes-to-hours and
//! are validated on every request, so the cheaper hash keeps the
//! per-resolve latency low; the brute-force window is bounded by
//! the session's short TTL anyway.
//!
//! `resolve_session` is the per-request validation: token →
//! session record → freshness / tenancy / actor checks → audit
//! `allowed`/`denied` → `SessionResolution`. `get_session_by_hash`
//! is the private lookup `resolve_session` builds on; mod.rs no
//! longer touches it directly.

use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::errors::RuntimeError;
use crate::tracing::now_ms;

use super::{
    validate_non_empty, AuthActor, AuthTraceContext, SessionAuthRuntime, SessionCreate,
    SessionRecord, SessionResolution,
};

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

impl SessionAuthRuntime {
    pub fn create_session(&self, input: SessionCreate) -> Result<SessionRecord, RuntimeError> {
        validate_non_empty("session id", &input.id)?;
        validate_non_empty("actor id", &input.actor_id)?;
        validate_non_empty("tenant id", &input.tenant_id)?;
        validate_non_empty("session token", &input.raw_token)?;
        if input.expires_ms <= input.issued_ms {
            return Err(RuntimeError::Other(
                "session expiry must be after issue time".to_string(),
            ));
        }
        let actor = self
            .get_actor(&input.actor_id)?
            .ok_or_else(|| RuntimeError::Other(format!("auth actor `{}` not found", input.actor_id)))?;
        if actor.tenant_id != input.tenant_id {
            return Err(RuntimeError::Other(
                "session actor tenant mismatch".to_string(),
            ));
        }
        let token_hash = hash_session_secret(&input.raw_token);
        let now = now_ms();
        self.conn
            .lock()
            .unwrap()
            .execute(
                "insert into auth_sessions
                 (id, actor_id, tenant_id, token_hash, issued_ms, expires_ms, rotation_counter, csrf_binding_id, revoked_ms, created_ms, updated_ms)
                 values (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, null, ?8, ?8)",
                params![
                    input.id,
                    input.actor_id,
                    input.tenant_id,
                    token_hash,
                    input.issued_ms as i64,
                    input.expires_ms as i64,
                    input.csrf_binding_id,
                    now as i64,
                ],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to create session: {err}")))?;
        self.get_session(&input.id)?
            .ok_or_else(|| RuntimeError::Other(format!("auth session `{}` not found", input.id)))
    }

    pub fn get_session(&self, id: &str) -> Result<Option<SessionRecord>, RuntimeError> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "select id, actor_id, tenant_id, token_hash, issued_ms, expires_ms, rotation_counter, csrf_binding_id, revoked_ms, created_ms, updated_ms
                 from auth_sessions where id = ?1",
                params![id],
                read_session_row,
            )
            .optional()
            .map_err(|err| RuntimeError::Other(format!("failed to read session: {err}")))
    }

    pub fn resolve_session(
        &self,
        raw_token: &str,
        expected_tenant_id: &str,
        trace_id: &str,
        replay_key: &str,
        at_ms: u64,
    ) -> Result<SessionResolution, RuntimeError> {
        validate_non_empty("session token", raw_token)?;
        validate_non_empty("tenant id", expected_tenant_id)?;
        validate_non_empty("trace id", trace_id)?;
        let token_hash = hash_session_secret(raw_token);
        let session = match self.get_session_by_hash(&token_hash)? {
            Some(session) => session,
            None => {
                self.insert_audit(
                    "session.resolve",
                    None,
                    Some(expected_tenant_id),
                    None,
                    None,
                    Some(trace_id),
                    "denied",
                    "session token not found",
                )?;
                return Err(RuntimeError::Other(
                    "session resolve denied: token not found".to_string(),
                ));
            }
        };
        if session.revoked_ms.is_some() {
            self.audit_session_denied(&session, trace_id, "session revoked")?;
            return Err(RuntimeError::Other(
                "session resolve denied: session revoked".to_string(),
            ));
        }
        if at_ms >= session.expires_ms {
            self.audit_session_denied(&session, trace_id, "session expired")?;
            return Err(RuntimeError::Other(
                "session resolve denied: session expired".to_string(),
            ));
        }
        if session.tenant_id != expected_tenant_id {
            self.audit_session_denied(&session, trace_id, "tenant mismatch")?;
            return Err(RuntimeError::Other(
                "session resolve denied: tenant mismatch".to_string(),
            ));
        }
        let actor = self
            .get_actor(&session.actor_id)?
            .ok_or_else(|| RuntimeError::Other("session actor not found".to_string()))?;
        if actor.tenant_id != session.tenant_id {
            self.audit_session_denied(&session, trace_id, "actor tenant mismatch")?;
            return Err(RuntimeError::Other(
                "session resolve denied: actor tenant mismatch".to_string(),
            ));
        }
        let trace = AuthTraceContext {
            trace_id: trace_id.to_string(),
            tenant_id: session.tenant_id.clone(),
            actor_id: actor.id.clone(),
            auth_method: actor.auth_method.clone(),
            session_id: session.id.clone(),
            api_key_id: String::new(),
            permission_fingerprint: actor.permission_fingerprint.clone(),
            replay_key: replay_key.to_string(),
        };
        self.insert_audit(
            "session.resolve",
            Some(&actor.id),
            Some(&session.tenant_id),
            Some(&session.id),
            None,
            Some(trace_id),
            "allowed",
            "session resolved",
        )?;
        Ok(SessionResolution {
            actor,
            session,
            trace,
        })
    }

    pub fn rotate_session(
        &self,
        session_id: &str,
        new_raw_token: &str,
        new_expires_ms: u64,
    ) -> Result<SessionRecord, RuntimeError> {
        validate_non_empty("session id", session_id)?;
        validate_non_empty("session token", new_raw_token)?;
        let session = self
            .get_session(session_id)?
            .ok_or_else(|| RuntimeError::Other(format!("auth session `{session_id}` not found")))?;
        if new_expires_ms <= session.issued_ms {
            return Err(RuntimeError::Other(
                "rotated session expiry must be after issue time".to_string(),
            ));
        }
        let now = now_ms();
        let token_hash = hash_session_secret(new_raw_token);
        self.conn
            .lock()
            .unwrap()
            .execute(
                "update auth_sessions
                 set token_hash = ?2, expires_ms = ?3, rotation_counter = rotation_counter + 1, revoked_ms = null, updated_ms = ?4
                 where id = ?1",
                params![session_id, token_hash, new_expires_ms as i64, now as i64],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to rotate session: {err}")))?;
        self.get_session(session_id)?
            .ok_or_else(|| RuntimeError::Other(format!("auth session `{session_id}` not found")))
    }

    pub fn revoke_session(&self, session_id: &str, at_ms: u64) -> Result<SessionRecord, RuntimeError> {
        validate_non_empty("session id", session_id)?;
        self.conn
            .lock()
            .unwrap()
            .execute(
                "update auth_sessions set revoked_ms = ?2, updated_ms = ?2 where id = ?1",
                params![session_id, at_ms as i64],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to revoke session: {err}")))?;
        self.get_session(session_id)?
            .ok_or_else(|| RuntimeError::Other(format!("auth session `{session_id}` not found")))
    }

    pub(super) fn get_session_by_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<SessionRecord>, RuntimeError> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "select id, actor_id, tenant_id, token_hash, issued_ms, expires_ms, rotation_counter, csrf_binding_id, revoked_ms, created_ms, updated_ms
                 from auth_sessions where token_hash = ?1",
                params![token_hash],
                read_session_row,
            )
            .optional()
            .map_err(|err| RuntimeError::Other(format!("failed to read session by token: {err}")))
    }
}
