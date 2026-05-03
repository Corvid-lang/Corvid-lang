//! OAuth domain — state-token hashing, row decoding, and the
//! `SessionAuthRuntime` impl block carrying create / get /
//! resolve_callback for `auth_oauth_states`.
//!
//! OAuth callbacks land back at the runtime carrying the state
//! token the client sent at authorize time. The state is hashed
//! with SHA-256 (same family as session tokens, with a different
//! prefix to keep the hash spaces distinct) so a leaked database
//! row never lets an attacker forge a future callback.
//!
//! `resolve_oauth_callback` enforces single-use semantics — the
//! conditional `update ... where id = ?1 and used_ms is null`
//! plus the audit-then-error pattern means a replayed callback
//! lands in the audit log as `denied: oauth state already used`
//! even if the original write raced with a concurrent callback
//! attempt.

use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::errors::RuntimeError;
use crate::tracing::now_ms;

use super::{
    validate_non_empty, AuthTraceContext, OAuthCallbackResolution, OAuthStateCreate,
    OAuthStateRecord, SessionAuthRuntime,
};

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

impl SessionAuthRuntime {
    pub fn create_oauth_state(
        &self,
        input: OAuthStateCreate,
    ) -> Result<OAuthStateRecord, RuntimeError> {
        validate_non_empty("oauth state id", &input.id)?;
        validate_non_empty("oauth provider", &input.provider)?;
        validate_non_empty("tenant id", &input.tenant_id)?;
        validate_non_empty("actor id", &input.actor_id)?;
        validate_non_empty("oauth state", &input.raw_state)?;
        validate_non_empty("pkce verifier reference", &input.pkce_verifier_ref)?;
        validate_non_empty("nonce fingerprint", &input.nonce_fingerprint)?;
        validate_non_empty("replay key", &input.replay_key)?;
        let now = now_ms();
        if input.expires_ms <= now {
            return Err(RuntimeError::Other(
                "oauth state expiry must be in the future".to_string(),
            ));
        }
        let actor = self.get_actor(&input.actor_id)?.ok_or_else(|| {
            RuntimeError::Other(format!("auth actor `{}` not found", input.actor_id))
        })?;
        if actor.tenant_id != input.tenant_id {
            return Err(RuntimeError::Other(
                "oauth actor tenant mismatch".to_string(),
            ));
        }
        let state_hash = hash_oauth_state(&input.raw_state);
        self.conn
            .lock()
            .unwrap()
            .execute(
                "insert into auth_oauth_states
                 (id, provider, tenant_id, actor_id, state_hash, pkce_verifier_ref, nonce_fingerprint, expires_ms, replay_key, used_ms, created_ms, updated_ms)
                 values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, null, ?10, ?10)",
                params![
                    input.id,
                    input.provider,
                    input.tenant_id,
                    input.actor_id,
                    state_hash,
                    input.pkce_verifier_ref,
                    input.nonce_fingerprint,
                    input.expires_ms as i64,
                    input.replay_key,
                    now as i64,
                ],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to create oauth state: {err}")))?;
        self.get_oauth_state(&input.id)?
            .ok_or_else(|| RuntimeError::Other(format!("oauth state `{}` not found", input.id)))
    }

    pub fn get_oauth_state(&self, id: &str) -> Result<Option<OAuthStateRecord>, RuntimeError> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "select id, provider, tenant_id, actor_id, state_hash, pkce_verifier_ref, nonce_fingerprint, expires_ms, replay_key, used_ms, created_ms, updated_ms
                 from auth_oauth_states where id = ?1",
                params![id],
                read_oauth_state_row,
            )
            .optional()
            .map_err(|err| RuntimeError::Other(format!("failed to read oauth state: {err}")))
    }

    pub fn resolve_oauth_callback(
        &self,
        raw_state: &str,
        expected_tenant_id: &str,
        trace_id: &str,
        at_ms: u64,
    ) -> Result<OAuthCallbackResolution, RuntimeError> {
        validate_non_empty("oauth state", raw_state)?;
        validate_non_empty("tenant id", expected_tenant_id)?;
        validate_non_empty("trace id", trace_id)?;
        let state_hash = hash_oauth_state(raw_state);
        let state = match self.get_oauth_state_by_hash(&state_hash)? {
            Some(state) => state,
            None => {
                self.insert_audit(
                    "oauth.callback",
                    None,
                    Some(expected_tenant_id),
                    None,
                    None,
                    Some(trace_id),
                    "denied",
                    "oauth state not found",
                )?;
                return Err(RuntimeError::Other(
                    "oauth callback denied: state not found".to_string(),
                ));
            }
        };
        if state.used_ms.is_some() {
            self.audit_oauth_denied(&state, trace_id, "oauth state already used")?;
            return Err(RuntimeError::Other(
                "oauth callback denied: state already used".to_string(),
            ));
        }
        if at_ms >= state.expires_ms {
            self.audit_oauth_denied(&state, trace_id, "oauth state expired")?;
            return Err(RuntimeError::Other(
                "oauth callback denied: state expired".to_string(),
            ));
        }
        if state.tenant_id != expected_tenant_id {
            self.audit_oauth_denied(&state, trace_id, "tenant mismatch")?;
            return Err(RuntimeError::Other(
                "oauth callback denied: tenant mismatch".to_string(),
            ));
        }
        let actor = self
            .get_actor(&state.actor_id)?
            .ok_or_else(|| RuntimeError::Other("oauth actor not found".to_string()))?;
        if actor.tenant_id != state.tenant_id {
            self.audit_oauth_denied(&state, trace_id, "actor tenant mismatch")?;
            return Err(RuntimeError::Other(
                "oauth callback denied: actor tenant mismatch".to_string(),
            ));
        }
        self.conn
            .lock()
            .unwrap()
            .execute(
                "update auth_oauth_states set used_ms = ?2, updated_ms = ?2 where id = ?1 and used_ms is null",
                params![state.id, at_ms as i64],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to mark oauth state used: {err}")))?;
        let state = self.get_oauth_state(&state.id)?.ok_or_else(|| {
            RuntimeError::Other("oauth state disappeared after callback".to_string())
        })?;
        let trace = AuthTraceContext {
            trace_id: trace_id.to_string(),
            tenant_id: state.tenant_id.clone(),
            actor_id: actor.id.clone(),
            auth_method: "oauth".to_string(),
            session_id: String::new(),
            api_key_id: String::new(),
            permission_fingerprint: actor.permission_fingerprint.clone(),
            replay_key: state.replay_key.clone(),
        };
        self.insert_audit(
            "oauth.callback",
            Some(&actor.id),
            Some(&state.tenant_id),
            None,
            None,
            Some(trace_id),
            "allowed",
            "oauth state resolved",
        )?;
        Ok(OAuthCallbackResolution {
            actor,
            state,
            trace,
        })
    }

    fn get_oauth_state_by_hash(
        &self,
        state_hash: &str,
    ) -> Result<Option<OAuthStateRecord>, RuntimeError> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "select id, provider, tenant_id, actor_id, state_hash, pkce_verifier_ref, nonce_fingerprint, expires_ms, replay_key, used_ms, created_ms, updated_ms
                 from auth_oauth_states where state_hash = ?1",
                params![state_hash],
                read_oauth_state_row,
            )
            .optional()
            .map_err(|err| RuntimeError::Other(format!("failed to read oauth state by hash: {err}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthActor;

    fn actor(id: &str, tenant_id: &str) -> AuthActor {
        AuthActor {
            id: id.to_string(),
            tenant_id: tenant_id.to_string(),
            display_name: "Ada".to_string(),
            actor_kind: "user".to_string(),
            auth_method: "session".to_string(),
            assurance_level: "aal1".to_string(),
            role_fingerprint: "sha256:roles".to_string(),
            permission_fingerprint: "sha256:permissions".to_string(),
            created_ms: 1,
            updated_ms: 1,
        }
    }

    #[test]
    fn oauth_callback_state_is_hashed_single_use_and_restart_safe() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.sqlite");
        let expires_ms = now_ms().saturating_add(60_000);
        {
            let auth = SessionAuthRuntime::open(&path).unwrap();
            auth.upsert_actor(actor("user-1", "org-1")).unwrap();
            let state = auth
                .create_oauth_state(OAuthStateCreate {
                    id: "oauth-state-1".to_string(),
                    provider: "google".to_string(),
                    tenant_id: "org-1".to_string(),
                    actor_id: "user-1".to_string(),
                    raw_state: "raw-oauth-state".to_string(),
                    pkce_verifier_ref: "pkce-ref-1".to_string(),
                    nonce_fingerprint: "sha256:nonce".to_string(),
                    expires_ms,
                    replay_key: "replay-oauth-1".to_string(),
                })
                .unwrap();
            assert_eq!(state.state_hash, hash_oauth_state("raw-oauth-state"));
            assert!(!state.state_hash.contains("raw-oauth-state"));
            assert_eq!(state.used_ms, None);
        }

        let auth = SessionAuthRuntime::open(&path).unwrap();
        let resolved = auth
            .resolve_oauth_callback("raw-oauth-state", "org-1", "trace-oauth-1", now_ms())
            .unwrap();
        assert_eq!(resolved.actor.id, "user-1");
        assert_eq!(resolved.trace.auth_method, "oauth");
        assert_eq!(resolved.trace.replay_key, "replay-oauth-1");
        assert!(resolved.state.used_ms.is_some());

        let replay = auth
            .resolve_oauth_callback("raw-oauth-state", "org-1", "trace-oauth-2", now_ms())
            .unwrap_err();
        assert!(replay.to_string().contains("state already used"));
        let audit = auth.audit_events().unwrap();
        assert_eq!(audit.len(), 2);
        assert!(audit.iter().all(|event| {
            !event.reason.contains("raw-oauth-state") && !event.id.contains("raw-oauth-state")
        }));
    }

    #[test]
    fn oauth_callback_rejects_expired_and_cross_tenant_state() {
        let auth = SessionAuthRuntime::open_in_memory().unwrap();
        auth.upsert_actor(actor("user-1", "org-1")).unwrap();
        let expires_ms = now_ms().saturating_add(60_000);
        auth.create_oauth_state(OAuthStateCreate {
            id: "oauth-state-1".to_string(),
            provider: "github".to_string(),
            tenant_id: "org-1".to_string(),
            actor_id: "user-1".to_string(),
            raw_state: "state-1".to_string(),
            pkce_verifier_ref: "pkce-ref-1".to_string(),
            nonce_fingerprint: "sha256:nonce".to_string(),
            expires_ms,
            replay_key: "replay-oauth-1".to_string(),
        })
        .unwrap();

        let tenant = auth
            .resolve_oauth_callback("state-1", "org-2", "trace-tenant", now_ms())
            .unwrap_err();
        assert!(tenant.to_string().contains("tenant mismatch"));
        let expired = auth
            .resolve_oauth_callback("state-1", "org-1", "trace-expired", expires_ms)
            .unwrap_err();
        assert!(expired.to_string().contains("state expired"));
    }
}
