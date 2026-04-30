//! Real JWT verification — slice 39K.
//!
//! Phase 39 originally shipped `validate_jwt_verification_contract`,
//! which checked the *shape* of a verification contract: issuer URL
//! prefix, alg name, claim presence. It did not fetch the JWKS,
//! resolve the `kid` header, verify the signature, or validate
//! `exp`/`nbf`/`iss`/`aud`. A token signed with `alg=none`, an
//! expired token, or one with a forged `kid` would all pass that
//! validator unchanged.
//!
//! 39K replaces that with a real verifier built on `jsonwebtoken
//! 9.3`. The shape validator stays — it is still the right tool
//! for `corvid auth migrate` to assert a config block is well-
//! formed before a worker boots — and the real verifier sits
//! alongside it for `verify_jwt(token, contract, now_ms)` calls
//! that actually check signatures.
//!
//! The module is split per responsibility:
//!
//! - [`jwks`] holds the JWKS document model + pluggable fetcher
//!   ([`JsonWebKey`], [`JsonWebKeySet`], [`JwksFetcher`],
//!   [`ReqwestJwksFetcher`]).
//! - [`verifier`] holds the JWT verifier ([`JwtVerifier`]) and
//!   its private key/algorithm helpers (`parse_alg`,
//!   `decoding_key_for`, `map_jwt_error`) plus the verifier's
//!   adversarial test corpus.
//!
//! This module owns only the cross-cutting surface — the error
//! enum and the verified-claims record — so consumers reach
//! through [`crate::jwt_verify::JwtVerifyError`] /
//! [`crate::jwt_verify::VerifiedJwtClaims`] without caring which
//! submodule produced them.

pub mod jwks;
pub mod verifier;

pub use jwks::{JsonWebKey, JsonWebKeySet, JwksFetcher, ReqwestJwksFetcher};
pub use verifier::JwtVerifier;

/// Reasons a JWT verification can fail. Each variant maps to a
/// stable failure_kind string the audit log records — operators
/// pivot incident triage on these names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JwtVerifyError {
    MalformedToken(String),
    UnsupportedAlgorithm(String),
    AlgNoneRefused,
    KidNotFound(String),
    KeyMaterialMissing(String),
    JwksFetchFailed(String),
    SignatureMismatch,
    ExpiredToken,
    NotYetValid,
    IssuerMismatch,
    AudienceMismatch,
    MissingClaim(String),
}

impl JwtVerifyError {
    /// Stable slug for audit logs and the runtime error envelope.
    /// Matches the `failure_kind` strings the existing
    /// `JwtContractDiagnostic` records.
    pub fn slug(&self) -> &'static str {
        match self {
            Self::MalformedToken(_) => "malformed_token",
            Self::UnsupportedAlgorithm(_) => "unsupported_algorithm",
            Self::AlgNoneRefused => "alg_none_refused",
            Self::KidNotFound(_) => "kid_not_found",
            Self::KeyMaterialMissing(_) => "key_material_missing",
            Self::JwksFetchFailed(_) => "jwks_fetch_failed",
            Self::SignatureMismatch => "signature_mismatch",
            Self::ExpiredToken => "expired_token",
            Self::NotYetValid => "nbf_in_future",
            Self::IssuerMismatch => "issuer_mismatch",
            Self::AudienceMismatch => "audience_mismatch",
            Self::MissingClaim(_) => "missing_claim",
        }
    }
}

impl std::fmt::Display for JwtVerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "jwt_verify: {}", self.slug())
    }
}

impl std::error::Error for JwtVerifyError {}

/// Decoded + verified claims. Returned on successful
/// `verify_jwt`. The JSON value covers provider-specific extensions
/// the contract did not name; required claims are surfaced as
/// typed fields.
#[derive(Debug, Clone, PartialEq)]
pub struct VerifiedJwtClaims {
    pub subject: String,
    pub tenant: String,
    pub issuer: String,
    pub audience: String,
    pub exp_ms: u64,
    pub iat_ms: u64,
    pub raw: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Slice 39K positive: `JwtVerifyError::slug()` returns stable
    /// strings that match the existing `JwtContractDiagnostic`
    /// failure_kind taxonomy. Audit log consumers pivot on these.
    #[test]
    fn error_slugs_are_stable_for_audit_log() {
        assert_eq!(JwtVerifyError::AlgNoneRefused.slug(), "alg_none_refused");
        assert_eq!(JwtVerifyError::SignatureMismatch.slug(), "signature_mismatch");
        assert_eq!(JwtVerifyError::ExpiredToken.slug(), "expired_token");
        assert_eq!(JwtVerifyError::NotYetValid.slug(), "nbf_in_future");
        assert_eq!(JwtVerifyError::IssuerMismatch.slug(), "issuer_mismatch");
        assert_eq!(JwtVerifyError::AudienceMismatch.slug(), "audience_mismatch");
        assert_eq!(
            JwtVerifyError::MissingClaim("foo".to_string()).slug(),
            "missing_claim"
        );
    }
}
