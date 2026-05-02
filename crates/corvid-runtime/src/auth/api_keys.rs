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
        let actor = self
            .get_actor(&input.service_actor_id)?
            .ok_or_else(|| {
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
                return self.resolve_api_key_record(key, expected_tenant_id, trace_id, replay_key, at_ms);
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
            .map_err(|err| RuntimeError::Other(format!("failed to list api key candidates: {err}")))?;
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
