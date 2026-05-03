//! API-key domain — Argon2id hashing, row decoding, and the
//! `SessionAuthRuntime` impl block carrying create / get /
//! resolve / revoke for `auth_api_keys`.
//!
//! API keys store an Argon2id-encoded hash of the raw secret —
//! never the secret itself. `hash_api_key_secret` produces the
//! encoded hash at issuance; `verify_api_key_secret` validates a
//! presented secret against the stored hash on every resolve.
//! Argon2id was chosen over the cheaper SHA-based session-token
//! hash because API keys live longer (months vs. minutes), so
//! the per-resolve cost is amortized and the brute-force defense
//! matters more.
//!
//! `resolve_api_key` walks every active candidate in the tenant
//! and verifies the presented secret against each — there's no
//! key-id index because the raw key is the only identifier the
//! caller presents. `resolve_api_key_record` is the post-match
//! validation (revocation / expiry / tenancy / actor-kind)
//! before issuing a `last_used_ms` write and the audit row.

use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rand_core::OsRng;
use rusqlite::{params, OptionalExtension};

use crate::errors::RuntimeError;
use crate::tracing::now_ms;

use super::{
    validate_non_empty, ApiKeyCreate, ApiKeyRecord, ApiKeyResolution, AuthTraceContext,
    SessionAuthRuntime,
};

pub fn hash_api_key_secret(raw_key: &str) -> Result<String, RuntimeError> {
    validate_non_empty("api key secret", raw_key)?;
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(raw_key.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|err| RuntimeError::Other(format!("failed to hash api key: {err}")))
}

pub fn verify_api_key_secret(raw_key: &str, encoded_hash: &str) -> Result<bool, RuntimeError> {
    validate_non_empty("api key secret", raw_key)?;
    let parsed = PasswordHash::new(encoded_hash)
        .map_err(|err| RuntimeError::Other(format!("invalid stored api key hash: {err}")))?;
    Ok(Argon2::default()
        .verify_password(raw_key.as_bytes(), &parsed)
        .is_ok())
}

pub(super) fn read_api_key_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ApiKeyRecord> {
    Ok(ApiKeyRecord {
        id: row.get(0)?,
        service_actor_id: row.get(1)?,
        tenant_id: row.get(2)?,
        key_hash: row.get(3)?,
        scope_fingerprint: row.get(4)?,
        expires_ms: row.get::<_, i64>(5)? as u64,
        last_used_ms: row.get::<_, Option<i64>>(6)?.map(|value| value as u64),
        revoked_ms: row.get::<_, Option<i64>>(7)?.map(|value| value as u64),
        created_ms: row.get::<_, i64>(8)? as u64,
        updated_ms: row.get::<_, i64>(9)? as u64,
    })
}

impl SessionAuthRuntime {
    pub fn create_api_key(&self, input: ApiKeyCreate) -> Result<ApiKeyRecord, RuntimeError> {
        validate_non_empty("api key id", &input.id)?;
        validate_non_empty("service actor id", &input.service_actor_id)?;
        validate_non_empty("tenant id", &input.tenant_id)?;
        validate_non_empty("api key secret", &input.raw_key)?;
        validate_non_empty("scope fingerprint", &input.scope_fingerprint)?;
        let now = now_ms();
        if input.expires_ms <= now {
            return Err(RuntimeError::Other(
                "api key expiry must be in the future".to_string(),
            ));
        }
        let actor = self.get_actor(&input.service_actor_id)?.ok_or_else(|| {
            RuntimeError::Other(format!("auth actor `{}` not found", input.service_actor_id))
        })?;
        if actor.tenant_id != input.tenant_id {
            return Err(RuntimeError::Other(
                "api key actor tenant mismatch".to_string(),
            ));
        }
        if actor.actor_kind != "service" {
            return Err(RuntimeError::Other(
                "api keys must resolve to service actors".to_string(),
            ));
        }
        let key_hash = hash_api_key_secret(&input.raw_key)?;
        self.conn
            .lock()
            .unwrap()
            .execute(
                "insert into auth_api_keys
                 (id, service_actor_id, tenant_id, key_hash, scope_fingerprint, expires_ms, last_used_ms, revoked_ms, created_ms, updated_ms)
                 values (?1, ?2, ?3, ?4, ?5, ?6, null, null, ?7, ?7)",
                params![
                    input.id,
                    input.service_actor_id,
                    input.tenant_id,
                    key_hash,
                    input.scope_fingerprint,
                    input.expires_ms as i64,
                    now as i64,
                ],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to create api key: {err}")))?;
        self.get_api_key(&input.id)?
            .ok_or_else(|| RuntimeError::Other(format!("auth api key `{}` not found", input.id)))
    }

    pub fn get_api_key(&self, id: &str) -> Result<Option<ApiKeyRecord>, RuntimeError> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "select id, service_actor_id, tenant_id, key_hash, scope_fingerprint, expires_ms, last_used_ms, revoked_ms, created_ms, updated_ms
                 from auth_api_keys where id = ?1",
                params![id],
                read_api_key_row,
            )
            .optional()
            .map_err(|err| RuntimeError::Other(format!("failed to read api key: {err}")))
    }

    pub fn resolve_api_key(
        &self,
        raw_key: &str,
        expected_tenant_id: &str,
        trace_id: &str,
        replay_key: &str,
        at_ms: u64,
    ) -> Result<ApiKeyResolution, RuntimeError> {
        validate_non_empty("api key secret", raw_key)?;
        validate_non_empty("tenant id", expected_tenant_id)?;
        validate_non_empty("trace id", trace_id)?;
        let keys = self.list_active_api_key_candidates(expected_tenant_id)?;
        for key in keys {
            if verify_api_key_secret(raw_key, &key.key_hash)? {
                return self.resolve_api_key_record(
                    key,
                    expected_tenant_id,
                    trace_id,
                    replay_key,
                    at_ms,
                );
            }
        }
        self.insert_audit(
            "api_key.resolve",
            None,
            Some(expected_tenant_id),
            None,
            None,
            Some(trace_id),
            "denied",
            "api key not found",
        )?;
        Err(RuntimeError::Other(
            "api key resolve denied: key not found".to_string(),
        ))
    }

    pub fn revoke_api_key(&self, key_id: &str, at_ms: u64) -> Result<ApiKeyRecord, RuntimeError> {
        validate_non_empty("api key id", key_id)?;
        self.conn
            .lock()
            .unwrap()
            .execute(
                "update auth_api_keys set revoked_ms = ?2, updated_ms = ?2 where id = ?1",
                params![key_id, at_ms as i64],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to revoke api key: {err}")))?;
        self.get_api_key(key_id)?
            .ok_or_else(|| RuntimeError::Other(format!("auth api key `{key_id}` not found")))
    }

    fn list_active_api_key_candidates(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<ApiKeyRecord>, RuntimeError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "select id, service_actor_id, tenant_id, key_hash, scope_fingerprint, expires_ms, last_used_ms, revoked_ms, created_ms, updated_ms
                 from auth_api_keys where tenant_id = ?1 and revoked_ms is null",
            )
            .map_err(|err| {
                RuntimeError::Other(format!("failed to prepare api key candidates: {err}"))
            })?;
        let rows = stmt
            .query_map(params![tenant_id], read_api_key_row)
            .map_err(|err| {
                RuntimeError::Other(format!("failed to list api key candidates: {err}"))
            })?;
        let mut keys = Vec::new();
        for row in rows {
            keys.push(row.map_err(|err| {
                RuntimeError::Other(format!("failed to decode api key candidate: {err}"))
            })?);
        }
        Ok(keys)
    }

    fn resolve_api_key_record(
        &self,
        key: ApiKeyRecord,
        expected_tenant_id: &str,
        trace_id: &str,
        replay_key: &str,
        at_ms: u64,
    ) -> Result<ApiKeyResolution, RuntimeError> {
        if key.revoked_ms.is_some() {
            self.audit_api_key_denied(&key, trace_id, "api key revoked")?;
            return Err(RuntimeError::Other(
                "api key resolve denied: key revoked".to_string(),
            ));
        }
        if at_ms >= key.expires_ms {
            self.audit_api_key_denied(&key, trace_id, "api key expired")?;
            return Err(RuntimeError::Other(
                "api key resolve denied: key expired".to_string(),
            ));
        }
        if key.tenant_id != expected_tenant_id {
            self.audit_api_key_denied(&key, trace_id, "tenant mismatch")?;
            return Err(RuntimeError::Other(
                "api key resolve denied: tenant mismatch".to_string(),
            ));
        }
        let actor = self
            .get_actor(&key.service_actor_id)?
            .ok_or_else(|| RuntimeError::Other("api key actor not found".to_string()))?;
        if actor.tenant_id != key.tenant_id {
            self.audit_api_key_denied(&key, trace_id, "actor tenant mismatch")?;
            return Err(RuntimeError::Other(
                "api key resolve denied: actor tenant mismatch".to_string(),
            ));
        }
        if actor.actor_kind != "service" {
            self.audit_api_key_denied(&key, trace_id, "actor is not service")?;
            return Err(RuntimeError::Other(
                "api key resolve denied: actor is not service".to_string(),
            ));
        }
        self.conn
            .lock()
            .unwrap()
            .execute(
                "update auth_api_keys set last_used_ms = ?2, updated_ms = ?2 where id = ?1",
                params![key.id, at_ms as i64],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to update api key use: {err}")))?;
        let key = self
            .get_api_key(&key.id)?
            .ok_or_else(|| RuntimeError::Other("api key disappeared after resolve".to_string()))?;
        let trace = AuthTraceContext {
            trace_id: trace_id.to_string(),
            tenant_id: key.tenant_id.clone(),
            actor_id: actor.id.clone(),
            auth_method: "api_key".to_string(),
            session_id: String::new(),
            api_key_id: key.id.clone(),
            permission_fingerprint: actor.permission_fingerprint.clone(),
            replay_key: replay_key.to_string(),
        };
        self.insert_audit(
            "api_key.resolve",
            Some(&actor.id),
            Some(&key.tenant_id),
            None,
            Some(&key.id),
            Some(trace_id),
            "allowed",
            "api key resolved",
        )?;
        Ok(ApiKeyResolution {
            actor,
            api_key: key,
            trace,
        })
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

    fn service_actor(id: &str, tenant_id: &str) -> AuthActor {
        AuthActor {
            actor_kind: "service".to_string(),
            auth_method: "api_key".to_string(),
            display_name: "CI".to_string(),
            permission_fingerprint: "sha256:service-scopes".to_string(),
            ..actor(id, tenant_id)
        }
    }

    #[test]
    fn api_key_runtime_resolves_service_actor_with_argon2_hash_and_redacted_audit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.sqlite");
        {
            let auth = SessionAuthRuntime::open(&path).unwrap();
            auth.upsert_actor(service_actor("svc-1", "org-1")).unwrap();
            let key = auth
                .create_api_key(ApiKeyCreate {
                    id: "key-1".to_string(),
                    service_actor_id: "svc-1".to_string(),
                    tenant_id: "org-1".to_string(),
                    raw_key: "raw-api-key-secret".to_string(),
                    scope_fingerprint: "sha256:service-scopes".to_string(),
                    expires_ms: now_ms().saturating_add(60_000),
                })
                .unwrap();
            assert!(key.key_hash.starts_with("$argon2"));
            assert!(!key.key_hash.contains("raw-api-key-secret"));
            assert!(verify_api_key_secret("raw-api-key-secret", &key.key_hash).unwrap());
        }

        let auth = SessionAuthRuntime::open(&path).unwrap();
        let resolved = auth
            .resolve_api_key(
                "raw-api-key-secret",
                "org-1",
                "trace-key-1",
                "replay-key-1",
                now_ms(),
            )
            .unwrap();
        assert_eq!(resolved.actor.id, "svc-1");
        assert_eq!(resolved.actor.actor_kind, "service");
        assert_eq!(resolved.trace.api_key_id, "key-1");
        assert_eq!(resolved.trace.session_id, "");
        assert_eq!(
            resolved.api_key.last_used_ms,
            Some(resolved.api_key.updated_ms)
        );
        let audit = auth.audit_events().unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].event_kind, "api_key.resolve");
        assert_eq!(audit[0].api_key_id.as_deref(), Some("key-1"));
        assert_eq!(audit[0].status, "allowed");
    }

    #[test]
    fn api_key_runtime_rejects_wrong_tenant_revoked_expired_and_user_actors() {
        let auth = SessionAuthRuntime::open_in_memory().unwrap();
        auth.upsert_actor(service_actor("svc-1", "org-1")).unwrap();
        auth.upsert_actor(actor("user-1", "org-1")).unwrap();
        let expires_ms = now_ms().saturating_add(60_000);
        auth.create_api_key(ApiKeyCreate {
            id: "key-1".to_string(),
            service_actor_id: "svc-1".to_string(),
            tenant_id: "org-1".to_string(),
            raw_key: "secret-1".to_string(),
            scope_fingerprint: "sha256:service-scopes".to_string(),
            expires_ms,
        })
        .unwrap();
        let user_key = auth.create_api_key(ApiKeyCreate {
            id: "key-user".to_string(),
            service_actor_id: "user-1".to_string(),
            tenant_id: "org-1".to_string(),
            raw_key: "user-secret".to_string(),
            scope_fingerprint: "sha256:user-scopes".to_string(),
            expires_ms,
        });
        assert!(user_key.unwrap_err().to_string().contains("service actors"));

        let wrong_tenant = auth
            .resolve_api_key(
                "secret-1",
                "org-2",
                "trace-tenant",
                "replay-tenant",
                now_ms(),
            )
            .unwrap_err();
        assert!(wrong_tenant.to_string().contains("key not found"));

        let expired = auth
            .resolve_api_key(
                "secret-1",
                "org-1",
                "trace-expired",
                "replay-expired",
                expires_ms,
            )
            .unwrap_err();
        assert!(expired.to_string().contains("key expired"));

        auth.revoke_api_key("key-1", now_ms()).unwrap();
        let revoked = auth
            .resolve_api_key(
                "secret-1",
                "org-1",
                "trace-revoked",
                "replay-revoked",
                now_ms(),
            )
            .unwrap_err();
        assert!(revoked.to_string().contains("key not found"));
        let audit = auth.audit_events().unwrap();
        assert!(audit
            .iter()
            .all(|event| { !event.reason.contains("secret-1") && !event.id.contains("secret-1") }));
    }
}
