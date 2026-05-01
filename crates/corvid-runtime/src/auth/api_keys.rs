//! API-key hashing primitives + the `ApiKeyRecord` row reader.
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
//! `read_api_key_row` is the SQLite row → `ApiKeyRecord`
//! deserializer the impl methods on `SessionAuthRuntime` use to
//! return typed records from the `api_keys` table.

use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rand_core::OsRng;

use crate::errors::RuntimeError;

use super::{validate_non_empty, ApiKeyRecord};

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
