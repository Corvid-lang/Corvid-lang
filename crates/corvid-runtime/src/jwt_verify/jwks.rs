//! JWKS document model + pluggable fetcher.
//!
//! `JsonWebKey` / `JsonWebKeySet` deserialise the public-key
//! documents identity providers expose at their JWKS URL
//! (RFC 7517). `JwksFetcher` is the seam through which the host
//! supplies those documents — production wraps `reqwest` with a
//! TTL cache, tests inject canned keys.

use super::JwtVerifyError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

/// One key from a provider's JWKS document. Only the fields the
/// verifier consults are deserialised; provider-extension fields
/// are ignored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonWebKey {
    pub kty: String,
    #[serde(default)]
    pub alg: Option<String>,
    #[serde(default)]
    pub kid: Option<String>,
    /// `RSA` keys carry `n` (modulus) + `e` (exponent), base64url.
    #[serde(default)]
    pub n: Option<String>,
    #[serde(default)]
    pub e: Option<String>,
    /// `EC` keys carry `crv` (curve) + `x` + `y`, base64url.
    #[serde(default)]
    pub crv: Option<String>,
    #[serde(default)]
    pub x: Option<String>,
    #[serde(default)]
    pub y: Option<String>,
    /// `OKP` (EdDSA) keys carry `crv` (Ed25519) + `x` (public key).
    /// Ed25519 reuses the `x` field.
    #[serde(default)]
    pub r#use: Option<String>,
}

/// JWKS document — the array of public keys a provider rotates
/// through. Per RFC 7517 the document is `{ "keys": [ ... ] }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonWebKeySet {
    pub keys: Vec<JsonWebKey>,
}

/// Pluggable JWKS fetcher. Production wraps `reqwest`; tests
/// supply canned keys.
pub trait JwksFetcher: Send + Sync {
    fn fetch(&self, jwks_url: &str) -> Result<JsonWebKeySet, JwtVerifyError>;
}

/// `reqwest`-backed JWKS fetcher with a per-URL TTL cache. The
/// default TTL is 10 minutes, matching what most identity
/// providers recommend for kid-rotation polling.
pub struct ReqwestJwksFetcher {
    client: reqwest::blocking::Client,
    cache: Mutex<BTreeMap<String, CachedJwks>>,
    ttl: Duration,
}

#[derive(Debug, Clone)]
struct CachedJwks {
    keys: JsonWebKeySet,
    fetched_at: std::time::Instant,
}

impl ReqwestJwksFetcher {
    pub fn new() -> Result<Self, JwtVerifyError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent("corvid-runtime/39K")
            .build()
            .map_err(|e| {
                JwtVerifyError::JwksFetchFailed(format!("reqwest init: {e}"))
            })?;
        Ok(Self {
            client,
            cache: Mutex::new(BTreeMap::new()),
            ttl: Duration::from_secs(600),
        })
    }

    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }
}

impl Default for ReqwestJwksFetcher {
    fn default() -> Self {
        Self::new().expect("reqwest builder")
    }
}

impl JwksFetcher for ReqwestJwksFetcher {
    fn fetch(&self, jwks_url: &str) -> Result<JsonWebKeySet, JwtVerifyError> {
        // Cache lookup first.
        {
            let cache = self.cache.lock().unwrap();
            if let Some(entry) = cache.get(jwks_url) {
                if entry.fetched_at.elapsed() < self.ttl {
                    return Ok(entry.keys.clone());
                }
            }
        }
        let resp = self
            .client
            .get(jwks_url)
            .send()
            .map_err(|e| JwtVerifyError::JwksFetchFailed(format!("{e}")))?;
        if !resp.status().is_success() {
            return Err(JwtVerifyError::JwksFetchFailed(format!(
                "JWKS endpoint returned HTTP {}",
                resp.status().as_u16()
            )));
        }
        let keys: JsonWebKeySet = resp
            .json()
            .map_err(|e| JwtVerifyError::JwksFetchFailed(format!("decode: {e}")))?;
        let mut cache = self.cache.lock().unwrap();
        cache.insert(
            jwks_url.to_string(),
            CachedJwks {
                keys: keys.clone(),
                fetched_at: std::time::Instant::now(),
            },
        );
        Ok(keys)
    }
}
