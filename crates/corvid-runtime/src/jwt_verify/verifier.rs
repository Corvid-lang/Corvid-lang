//! JWT verifier — `jsonwebtoken 9.3` wired against a
//! [`crate::jwt_verify::JwksFetcher`] so production uses reqwest
//! plus a TTL cache and tests inject canned keys.
//!
//! The verifier enforces the slice-39K adversarial corpus:
//! `alg=none` rejection, header/contract algorithm alignment,
//! kid resolution against the JWKS, RFC-7517 key-shape validation,
//! and explicit `exp` re-checks against a caller-supplied clock so
//! tests stay deterministic without depending on system time.

use super::jwks::{JsonWebKey, JsonWebKeySet, JwksFetcher};
use super::{JwtVerifyError, VerifiedJwtClaims};
use crate::auth::JwtVerificationContract;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use std::sync::Arc;

/// Real JWT verifier. Wraps a `JwksFetcher` so production uses
/// reqwest + cache and tests inject canned keys.
pub struct JwtVerifier {
    fetcher: Arc<dyn JwksFetcher>,
}

impl JwtVerifier {
    pub fn new(fetcher: Arc<dyn JwksFetcher>) -> Self {
        Self { fetcher }
    }

    /// Verify a JWT against the supplied `contract`. Fetches the
    /// JWKS, picks the key by `kid`, runs `jsonwebtoken::decode`
    /// with the contract's alg + iss + aud, and validates required
    /// claims (`subject_claim`, `tenant_claim`).
    pub fn verify(
        &self,
        token: &str,
        contract: &JwtVerificationContract,
        now_ms: u64,
    ) -> Result<VerifiedJwtClaims, JwtVerifyError> {
        let header = jsonwebtoken::decode_header(token)
            .map_err(|e| JwtVerifyError::MalformedToken(format!("{e}")))?;

        // Reject `alg=none` unconditionally — the only safe response
        // to a token claiming no signature is to drop it.
        if header.alg == Algorithm::HS256
            && contract.algorithm.as_str() != "HS256"
        {
            // Some providers downgrade RS256 → HS256 with the
            // public key as the secret; if the contract did not
            // declare HS256, refuse.
            return Err(JwtVerifyError::UnsupportedAlgorithm(format!(
                "{:?}",
                header.alg
            )));
        }
        if matches!(header.alg, Algorithm::RS256 | Algorithm::ES256 | Algorithm::EdDSA) {
            // OK — supported.
        } else {
            return Err(JwtVerifyError::UnsupportedAlgorithm(format!(
                "{:?}",
                header.alg
            )));
        }

        // Algorithm declared by the contract must align with the
        // header's alg. This catches a swap from RS256 to ES256
        // attempting to use the wrong key shape.
        let contract_alg = parse_alg(&contract.algorithm)?;
        if contract_alg != header.alg {
            return Err(JwtVerifyError::UnsupportedAlgorithm(format!(
                "header={:?} contract={:?}",
                header.alg, contract_alg
            )));
        }

        let kid = header
            .kid
            .ok_or_else(|| JwtVerifyError::MalformedToken("missing kid header".to_string()))?;
        let jwks = self.fetcher.fetch(&contract.jwks_url)?;
        let jwk = jwks
            .keys
            .iter()
            .find(|k| k.kid.as_deref() == Some(&kid))
            .ok_or_else(|| JwtVerifyError::KidNotFound(kid.clone()))?;

        let key = decoding_key_for(jwk)?;

        let mut validation = Validation::new(contract_alg);
        validation.set_issuer(&[&contract.issuer]);
        validation.set_audience(&[&contract.audience]);
        // `jsonwebtoken` checks `exp` / `nbf` against system time
        // by default; we set `validation.leeway` to the contract's
        // skew (in seconds) and adjust `validation.set_required_spec_claims`.
        validation.leeway = contract.clock_skew_ms / 1_000;
        validation.required_spec_claims =
            ["exp", "iss", "aud"].iter().map(|s| s.to_string()).collect();

        let token_data = jsonwebtoken::decode::<serde_json::Value>(
            token,
            &key,
            &validation,
        )
        .map_err(map_jwt_error)?;

        let claims = token_data.claims;
        let exp_ms = claims
            .get("exp")
            .and_then(|v| v.as_u64())
            .map(|s| s.saturating_mul(1_000))
            .unwrap_or(0);
        let iat_ms = claims
            .get("iat")
            .and_then(|v| v.as_u64())
            .map(|s| s.saturating_mul(1_000))
            .unwrap_or(0);

        // Belt-and-suspenders: jsonwebtoken already checked `exp`,
        // but the contract may carry a tighter clock-skew window
        // than `validation.leeway` allows. We re-check against the
        // caller-supplied `now_ms` so the semantics stay
        // deterministic in tests.
        if exp_ms > 0 && exp_ms.saturating_add(contract.clock_skew_ms) < now_ms {
            return Err(JwtVerifyError::ExpiredToken);
        }

        let subject = claims
            .get(&contract.required_subject_claim)
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                JwtVerifyError::MissingClaim(contract.required_subject_claim.clone())
            })?
            .to_string();
        let tenant = claims
            .get(&contract.required_tenant_claim)
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                JwtVerifyError::MissingClaim(contract.required_tenant_claim.clone())
            })?
            .to_string();
        let issuer = claims
            .get("iss")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let audience = claims
            .get("aud")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        Ok(VerifiedJwtClaims {
            subject,
            tenant,
            issuer,
            audience,
            exp_ms,
            iat_ms,
            raw: claims,
        })
    }
}

fn parse_alg(alg: &str) -> Result<Algorithm, JwtVerifyError> {
    match alg {
        "RS256" => Ok(Algorithm::RS256),
        "ES256" => Ok(Algorithm::ES256),
        "EdDSA" => Ok(Algorithm::EdDSA),
        other => Err(JwtVerifyError::UnsupportedAlgorithm(other.to_string())),
    }
}

fn decoding_key_for(jwk: &JsonWebKey) -> Result<DecodingKey, JwtVerifyError> {
    match jwk.kty.as_str() {
        "RSA" => {
            let n = jwk
                .n
                .as_deref()
                .ok_or_else(|| JwtVerifyError::KeyMaterialMissing("rsa.n".to_string()))?;
            let e = jwk
                .e
                .as_deref()
                .ok_or_else(|| JwtVerifyError::KeyMaterialMissing("rsa.e".to_string()))?;
            DecodingKey::from_rsa_components(n, e)
                .map_err(|e| JwtVerifyError::KeyMaterialMissing(format!("rsa: {e}")))
        }
        "EC" => {
            let x = jwk
                .x
                .as_deref()
                .ok_or_else(|| JwtVerifyError::KeyMaterialMissing("ec.x".to_string()))?;
            let y = jwk
                .y
                .as_deref()
                .ok_or_else(|| JwtVerifyError::KeyMaterialMissing("ec.y".to_string()))?;
            DecodingKey::from_ec_components(x, y)
                .map_err(|e| JwtVerifyError::KeyMaterialMissing(format!("ec: {e}")))
        }
        "OKP" => {
            let x = jwk
                .x
                .as_deref()
                .ok_or_else(|| JwtVerifyError::KeyMaterialMissing("okp.x".to_string()))?;
            DecodingKey::from_ed_components(x)
                .map_err(|e| JwtVerifyError::KeyMaterialMissing(format!("okp: {e}")))
        }
        other => Err(JwtVerifyError::UnsupportedAlgorithm(format!(
            "kty={other}"
        ))),
    }
}

fn map_jwt_error(err: jsonwebtoken::errors::Error) -> JwtVerifyError {
    use jsonwebtoken::errors::ErrorKind;
    match err.kind() {
        ErrorKind::ExpiredSignature => JwtVerifyError::ExpiredToken,
        ErrorKind::ImmatureSignature => JwtVerifyError::NotYetValid,
        ErrorKind::InvalidIssuer => JwtVerifyError::IssuerMismatch,
        ErrorKind::InvalidAudience => JwtVerifyError::AudienceMismatch,
        ErrorKind::InvalidSignature => JwtVerifyError::SignatureMismatch,
        ErrorKind::InvalidAlgorithm => {
            JwtVerifyError::UnsupportedAlgorithm("jsonwebtoken rejected".to_string())
        }
        ErrorKind::MissingRequiredClaim(name) => JwtVerifyError::MissingClaim(name.clone()),
        _ => JwtVerifyError::MalformedToken(format!("{err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Test fetcher returning canned JWKS.
    struct StubFetcher {
        keys: Mutex<JsonWebKeySet>,
        last_url: Mutex<Option<String>>,
    }
    impl StubFetcher {
        fn new(keys: JsonWebKeySet) -> Self {
            Self {
                keys: Mutex::new(keys),
                last_url: Mutex::new(None),
            }
        }
    }
    impl JwksFetcher for StubFetcher {
        fn fetch(&self, jwks_url: &str) -> Result<JsonWebKeySet, JwtVerifyError> {
            *self.last_url.lock().unwrap() = Some(jwks_url.to_string());
            Ok(self.keys.lock().unwrap().clone())
        }
    }

    /// Fetcher that always fails — exercises the
    /// `JwksFetchFailed` mapping.
    struct FailingFetcher;
    impl JwksFetcher for FailingFetcher {
        fn fetch(&self, _jwks_url: &str) -> Result<JsonWebKeySet, JwtVerifyError> {
            Err(JwtVerifyError::JwksFetchFailed("network down".to_string()))
        }
    }

    fn contract() -> JwtVerificationContract {
        JwtVerificationContract {
            issuer: "https://issuer.test".to_string(),
            audience: "corvid-test".to_string(),
            jwks_url: "https://issuer.test/.well-known/jwks".to_string(),
            algorithm: "RS256".to_string(),
            required_subject_claim: "sub".to_string(),
            required_tenant_claim: "tenant".to_string(),
            clock_skew_ms: 60_000,
        }
    }

    /// Slice 39K positive: `parse_alg` accepts the three production
    /// algorithms (RS256 / ES256 / EdDSA) and refuses the rest.
    #[test]
    fn parse_alg_accepts_supported_and_refuses_others() {
        assert_eq!(parse_alg("RS256").unwrap(), Algorithm::RS256);
        assert_eq!(parse_alg("ES256").unwrap(), Algorithm::ES256);
        assert_eq!(parse_alg("EdDSA").unwrap(), Algorithm::EdDSA);
        let err = parse_alg("HS256").unwrap_err();
        assert_eq!(err.slug(), "unsupported_algorithm");
        let err = parse_alg("none").unwrap_err();
        assert_eq!(err.slug(), "unsupported_algorithm");
    }

    /// Slice 39K positive: `decoding_key_for` constructs an RSA
    /// `DecodingKey` from a properly-shaped JWK. We don't test the
    /// resulting signature path (that needs a paired private key),
    /// but the construction succeeding proves the JWK → key
    /// adapter handles the standard `RSA` shape.
    #[test]
    fn decoding_key_for_rsa_jwk_constructs() {
        let jwk = JsonWebKey {
            kty: "RSA".to_string(),
            alg: Some("RS256".to_string()),
            kid: Some("k1".to_string()),
            // Test vector from RFC 7517 Appendix A.1 (with the
            // `=` padding removed since base64url is unpadded).
            n: Some("0vx7agoebGcQSuuPiLJXZptN9nndrQmbXEps2aiAFbWhM78LhWx4cbbfAAtVT86zwu1RK7aPFFxuhDR1L6tSoc_BJECPebWKRXjBZCiFV4n3oknjhMstn64tZ_2W-5JsGY4Hc5n9yBXArwl93lqt7_RN5w6Cf0h4QyQ5v-65YGjQR0_FDW2QvzqY368QQMicAtaSqzs8KJZgnYb9c7d0zgdAZHzu6qMQvRL5hajrn1n91CbOpbISD08qNLyrdkt-bFTWhAI4vMQFh6WeZu0fM4lFd2NcRwr3XPksINHaQ-G_xBniIqbw0Ls1jF44-csFCur-kEgU8awapJzKnqDKgw".to_string()),
            e: Some("AQAB".to_string()),
            crv: None,
            x: None,
            y: None,
            r#use: Some("sig".to_string()),
        };
        decoding_key_for(&jwk).expect("rsa key constructs");
    }

    /// Slice 39K adversarial: a malformed JWK without the required
    /// material (RSA missing `n`) is refused with a clear
    /// `KeyMaterialMissing` error.
    #[test]
    fn decoding_key_for_rejects_rsa_without_n() {
        let jwk = JsonWebKey {
            kty: "RSA".to_string(),
            alg: Some("RS256".to_string()),
            kid: Some("k1".to_string()),
            n: None,
            e: Some("AQAB".to_string()),
            crv: None,
            x: None,
            y: None,
            r#use: Some("sig".to_string()),
        };
        let err = match decoding_key_for(&jwk) {
            Err(e) => e,
            Ok(_) => panic!("expected refusal"),
        };
        assert!(matches!(err, JwtVerifyError::KeyMaterialMissing(s) if s.contains("rsa.n")));
    }

    /// Slice 39K adversarial: an unknown `kty` is refused.
    #[test]
    fn decoding_key_for_rejects_unknown_kty() {
        let jwk = JsonWebKey {
            kty: "DSA".to_string(),
            alg: None,
            kid: Some("k1".to_string()),
            n: None,
            e: None,
            crv: None,
            x: None,
            y: None,
            r#use: None,
        };
        let err = match decoding_key_for(&jwk) {
            Err(e) => e,
            Ok(_) => panic!("expected refusal"),
        };
        assert!(matches!(err, JwtVerifyError::UnsupportedAlgorithm(s) if s.contains("DSA")));
    }

    /// Slice 39K adversarial: a token that is not even base64url-
    /// shaped (`decode_header` fails) surfaces as `MalformedToken`,
    /// before any JWKS fetch.
    #[test]
    fn malformed_token_is_refused_before_fetch() {
        let fetcher = Arc::new(StubFetcher::new(JsonWebKeySet { keys: vec![] }));
        let verifier = JwtVerifier::new(fetcher.clone());
        let err = verifier.verify("not.a.jwt", &contract(), 0).unwrap_err();
        assert_eq!(err.slug(), "malformed_token");
        // No JWKS fetch should have happened.
        assert!(fetcher.last_url.lock().unwrap().is_none());
    }

    /// Slice 39K adversarial: a `JwksFetcher` that fails surfaces
    /// `JwksFetchFailed` end-to-end. Any token whose header parses
    /// far enough to trigger the JWKS fetch hits this path.
    #[test]
    fn jwks_fetch_failure_is_surfaced() {
        // A header-only token with a valid b64 header containing
        // alg=RS256 and a kid. We use jsonwebtoken's encoder to
        // produce a header-shaped token even though the signature
        // is meaningless — we never reach signature verification
        // because the JWKS fetch fails first.
        use base64::Engine;
        let header = serde_json::json!({"alg": "RS256", "typ": "JWT", "kid": "k1"});
        let payload = serde_json::json!({"iss": "x", "aud": "y", "sub": "u", "exp": 0});
        let header_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&header).unwrap());
        let payload_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap());
        let token = format!("{header_b64}.{payload_b64}.deadbeef");

        let fetcher = Arc::new(FailingFetcher);
        let verifier = JwtVerifier::new(fetcher);
        let err = verifier.verify(&token, &contract(), 0).unwrap_err();
        assert_eq!(err.slug(), "jwks_fetch_failed");
    }

    /// Slice 39K adversarial: a token whose header advertises a kid
    /// not present in the JWKS surfaces `KidNotFound`. This is the
    /// kid-downgrade attack — a forged kid trying to point the
    /// verifier at a non-existent (or attacker-supplied) key.
    #[test]
    fn kid_downgrade_returns_kid_not_found() {
        use base64::Engine;
        let header = serde_json::json!({"alg": "RS256", "typ": "JWT", "kid": "forged-kid"});
        let payload = serde_json::json!({"iss": "x", "aud": "y", "sub": "u", "exp": 0});
        let header_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&header).unwrap());
        let payload_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap());
        let token = format!("{header_b64}.{payload_b64}.deadbeef");

        // JWKS contains a different kid — the attacker's kid is not in.
        let other = JsonWebKey {
            kty: "RSA".to_string(),
            alg: Some("RS256".to_string()),
            kid: Some("real-kid".to_string()),
            n: Some("AB".to_string()),
            e: Some("AQAB".to_string()),
            crv: None,
            x: None,
            y: None,
            r#use: Some("sig".to_string()),
        };
        let fetcher = Arc::new(StubFetcher::new(JsonWebKeySet { keys: vec![other] }));
        let verifier = JwtVerifier::new(fetcher);
        let err = verifier.verify(&token, &contract(), 0).unwrap_err();
        assert!(matches!(&err, JwtVerifyError::KidNotFound(k) if k == "forged-kid"));
        assert_eq!(err.slug(), "kid_not_found");
    }

    /// Slice 39K adversarial: a header alg the contract did not
    /// declare (RS256 in header, ES256 in contract) is refused
    /// before signature verification — preventing a key-confusion
    /// attack where the wrong key shape would silently succeed.
    #[test]
    fn header_alg_must_match_contract_alg() {
        use base64::Engine;
        // Token header says ES256.
        let header = serde_json::json!({"alg": "ES256", "typ": "JWT", "kid": "k1"});
        let payload = serde_json::json!({"iss": "x", "aud": "y", "sub": "u", "exp": 0});
        let header_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&header).unwrap());
        let payload_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap());
        let token = format!("{header_b64}.{payload_b64}.deadbeef");

        // Contract demands RS256.
        let mut con = contract();
        con.algorithm = "RS256".to_string();
        let fetcher = Arc::new(StubFetcher::new(JsonWebKeySet { keys: vec![] }));
        let verifier = JwtVerifier::new(fetcher);
        let err = verifier.verify(&token, &con, 0).unwrap_err();
        assert_eq!(err.slug(), "unsupported_algorithm");
    }

    /// Slice 39K adversarial: a token claiming `alg=none` or a
    /// non-production algorithm is refused. RFC 7515 alg=none is a
    /// known footgun.
    #[test]
    fn alg_none_in_header_is_refused() {
        use base64::Engine;
        let header = serde_json::json!({"alg": "none", "typ": "JWT", "kid": "k1"});
        let payload = serde_json::json!({"iss": "x", "aud": "y", "sub": "u", "exp": 0});
        let header_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&header).unwrap());
        let payload_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap());
        let token = format!("{header_b64}.{payload_b64}.");

        let fetcher = Arc::new(StubFetcher::new(JsonWebKeySet { keys: vec![] }));
        let verifier = JwtVerifier::new(fetcher);
        let err = verifier.verify(&token, &contract(), 0).unwrap_err();
        // jsonwebtoken's own `decode_header` may reject `alg=none`
        // outright, surfacing as MalformedToken; or our
        // `parse_alg` rejects with UnsupportedAlgorithm. Both are
        // documented refusals.
        assert!(
            matches!(
                err,
                JwtVerifyError::UnsupportedAlgorithm(_) | JwtVerifyError::MalformedToken(_)
            ),
            "{err:?}",
        );
    }
}
