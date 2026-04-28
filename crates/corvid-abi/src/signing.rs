//! DSSE v1 signing primitives shared across Corvid signed
//! artifacts: ABI attestations (this crate, `attestation.rs`) and
//! receipts (`corvid-cli::trace_diff`).
//!
//! A DSSE envelope is:
//!
//! ```json
//! {
//!   "payloadType": "<media-type uri>",
//!   "payload": "<base64 of the raw payload bytes>",
//!   "signatures": [{
//!     "keyid": "<opaque key identifier>",
//!     "sig": "<base64 of the ed25519 signature over PAE(payloadType, payload)>"
//!   }]
//! }
//! ```
//!
//! The signature covers the DSSE Pre-Authentication Encoding (PAE)
//! of `(payloadType, payload)`, not the raw payload, so an attacker
//! cannot replay the signature under a different payloadType. The
//! verifier rejects an envelope whose payloadType is not in the
//! caller-supplied accept list.

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use ed25519_dalek::{ed25519::signature::Signer, Signature, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

/// One signature entry inside a DSSE envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DsseSignature {
    /// Opaque key identifier (free-form: hex prefix, fingerprint,
    /// KMS key name, user-supplied label).
    pub keyid: String,
    /// Base64-encoded signature bytes.
    pub sig: String,
}

/// DSSE v1 envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DsseEnvelope {
    #[serde(rename = "payloadType")]
    pub payload_type: String,
    /// Base64-encoded payload bytes.
    pub payload: String,
    pub signatures: Vec<DsseSignature>,
}

#[derive(Debug)]
pub enum SignError {
    KeyFileRead {
        path: std::path::PathBuf,
        error: std::io::Error,
    },
    KeyFormat {
        path_or_source: String,
        reason: String,
    },
    NoKey,
}

#[derive(Debug)]
pub enum VerifyError {
    EnvelopeJson(serde_json::Error),
    PayloadTypeMismatch {
        got: String,
        expected: String,
    },
    PayloadDecode(base64::DecodeError),
    SignatureDecode(base64::DecodeError),
    NoSignatures,
    SignatureVerify,
    VerifyKeyFormat {
        reason: String,
    },
    Io {
        path: std::path::PathBuf,
        error: std::io::Error,
    },
}

impl std::fmt::Display for SignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KeyFileRead { path, error } => {
                write!(f, "cannot read signing key file `{}`: {error}", path.display())
            }
            Self::KeyFormat { path_or_source, reason } => write!(
                f,
                "signing key from `{path_or_source}` is not a valid ed25519 seed: {reason}"
            ),
            Self::NoKey => write!(
                f,
                "no signing key configured: pass `--sign=<path>` or set `CORVID_SIGNING_KEY` to the hex-encoded 32-byte ed25519 seed"
            ),
        }
    }
}

impl std::error::Error for SignError {}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EnvelopeJson(e) => write!(f, "envelope JSON is malformed: {e}"),
            Self::PayloadTypeMismatch { got, expected } => write!(
                f,
                "payload type `{got}` does not match expected `{expected}` — this envelope does not claim to be the signed artifact you asked to verify"
            ),
            Self::PayloadDecode(e) => write!(f, "payload base64 decode failed: {e}"),
            Self::SignatureDecode(e) => write!(f, "signature base64 decode failed: {e}"),
            Self::NoSignatures => write!(f, "envelope has no signatures to verify"),
            Self::SignatureVerify => write!(
                f,
                "signature verification failed: either the payload was tampered with or the verifying key does not match the signing key"
            ),
            Self::VerifyKeyFormat { reason } => {
                write!(f, "verifying key is not a valid ed25519 public key: {reason}")
            }
            Self::Io { path, error } => write!(f, "io error reading `{}`: {error}", path.display()),
        }
    }
}

impl std::error::Error for VerifyError {}

/// DSSE v1 Pre-Authentication Encoding. Format:
///
/// ```text
/// DSSEv1 SP LEN(type) SP type SP LEN(payload) SP payload
/// ```
///
/// where LEN is decimal-ASCII byte length and SP is ASCII space.
pub fn pae(payload_type: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + payload_type.len() + 32);
    out.extend_from_slice(b"DSSEv1 ");
    out.extend_from_slice(payload_type.len().to_string().as_bytes());
    out.push(b' ');
    out.extend_from_slice(payload_type.as_bytes());
    out.push(b' ');
    out.extend_from_slice(payload.len().to_string().as_bytes());
    out.push(b' ');
    out.extend_from_slice(payload);
    out
}

/// How the user asked us to locate the signing key.
#[derive(Debug, Clone)]
pub enum KeySource {
    Path(std::path::PathBuf),
    /// Pre-extracted env-var content (the CLI reads `CORVID_SIGNING_KEY`
    /// at flag-parse time and passes the value here, so we don't
    /// re-read env vars on every signing call).
    Env(String),
}

/// Load a signing key from the given source. Accepts hex (64 chars
/// → 32 bytes, whitespace stripped — matches `openssl rand -hex 32`)
/// or raw 32 bytes.
pub fn load_signing_key(source: &KeySource) -> Result<SigningKey, SignError> {
    let (bytes, source_label) = match source {
        KeySource::Path(p) => {
            let contents = std::fs::read(p).map_err(|error| SignError::KeyFileRead {
                path: p.clone(),
                error,
            })?;
            (contents, p.display().to_string())
        }
        KeySource::Env(v) => (
            v.as_bytes().to_vec(),
            "CORVID_SIGNING_KEY env var".to_string(),
        ),
    };
    let seed = decode_ed25519_seed(&bytes, &source_label)?;
    Ok(SigningKey::from_bytes(&seed))
}

fn decode_ed25519_seed(raw: &[u8], source_label: &str) -> Result<[u8; 32], SignError> {
    let trimmed = raw
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect::<Vec<u8>>();
    if trimmed.iter().all(|b| b.is_ascii_hexdigit()) && trimmed.len() == 64 {
        let mut out = [0u8; 32];
        hex::decode_to_slice(&trimmed, &mut out).map_err(|e| SignError::KeyFormat {
            path_or_source: source_label.to_string(),
            reason: format!("hex decode failed: {e}"),
        })?;
        return Ok(out);
    }
    if raw.len() == 32 {
        let mut out = [0u8; 32];
        out.copy_from_slice(raw);
        return Ok(out);
    }
    Err(SignError::KeyFormat {
        path_or_source: source_label.to_string(),
        reason: format!(
            "expected 64 hex chars or 32 raw bytes, got {} bytes (trimmed: {})",
            raw.len(),
            trimmed.len()
        ),
    })
}

/// Sign `payload` under `payload_type` and return the DSSE envelope.
pub fn sign_envelope(
    payload: &[u8],
    payload_type: &str,
    key: &SigningKey,
    key_id: &str,
) -> DsseEnvelope {
    let signed = key.sign(&pae(payload_type, payload));
    DsseEnvelope {
        payload_type: payload_type.to_string(),
        payload: B64.encode(payload),
        signatures: vec![DsseSignature {
            keyid: key_id.to_string(),
            sig: B64.encode(signed.to_bytes()),
        }],
    }
}

/// Parse a DSSE envelope from JSON bytes, verify every signature
/// against `key`, and return the base64-decoded payload on success.
/// `accepted_payload_types` is the caller's allow-list — the
/// envelope's payloadType must match one of these. Different signed
/// artifacts (receipt vs. ABI attestation vs. in-toto Statement)
/// pass different lists so the same key can sign all three but the
/// verifier never confuses them.
pub fn verify_envelope(
    envelope_json: &[u8],
    accepted_payload_types: &[&str],
    key: &VerifyingKey,
) -> Result<Vec<u8>, VerifyError> {
    let envelope: DsseEnvelope =
        serde_json::from_slice(envelope_json).map_err(VerifyError::EnvelopeJson)?;
    if !accepted_payload_types.contains(&envelope.payload_type.as_str()) {
        return Err(VerifyError::PayloadTypeMismatch {
            got: envelope.payload_type.clone(),
            expected: accepted_payload_types.join(" or "),
        });
    }
    if envelope.signatures.is_empty() {
        return Err(VerifyError::NoSignatures);
    }
    let payload = B64
        .decode(envelope.payload.as_bytes())
        .map_err(VerifyError::PayloadDecode)?;
    let pae_bytes = pae(&envelope.payload_type, &payload);
    let mut any_valid = false;
    for sig_entry in &envelope.signatures {
        let sig_bytes = B64
            .decode(sig_entry.sig.as_bytes())
            .map_err(VerifyError::SignatureDecode)?;
        if sig_bytes.len() != 64 {
            continue;
        }
        let sig_arr: [u8; 64] = sig_bytes.as_slice().try_into().expect("checked length");
        let sig = Signature::from_bytes(&sig_arr);
        if key.verify(&pae_bytes, &sig).is_ok() {
            any_valid = true;
            break;
        }
    }
    if any_valid {
        Ok(payload)
    } else {
        Err(VerifyError::SignatureVerify)
    }
}

/// Parse an ed25519 verifying key from a file. Same hex/raw formats
/// as [`load_signing_key`] but for the 32-byte public key.
pub fn load_verifying_key(path: &std::path::Path) -> Result<VerifyingKey, VerifyError> {
    let raw = std::fs::read(path).map_err(|error| VerifyError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    let trimmed = raw
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect::<Vec<u8>>();
    let bytes: [u8; 32] = if trimmed.iter().all(|b| b.is_ascii_hexdigit()) && trimmed.len() == 64 {
        let mut out = [0u8; 32];
        hex::decode_to_slice(&trimmed, &mut out).map_err(|e| VerifyError::VerifyKeyFormat {
            reason: format!("hex decode failed: {e}"),
        })?;
        out
    } else if raw.len() == 32 {
        let mut out = [0u8; 32];
        out.copy_from_slice(&raw);
        out
    } else {
        return Err(VerifyError::VerifyKeyFormat {
            reason: format!(
                "expected 64 hex chars or 32 raw bytes, got {} bytes (trimmed: {})",
                raw.len(),
                trimmed.len()
            ),
        });
    };
    VerifyingKey::from_bytes(&bytes).map_err(|e| VerifyError::VerifyKeyFormat {
        reason: format!("malformed key bytes: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    /// Deterministic keypair from a fixed seed. Avoids pulling
    /// `rand` into the corvid-abi dep tree just for tests; signing
    /// callers in production supply the seed via key files.
    fn fresh_keypair() -> (SigningKey, VerifyingKey) {
        let seed: [u8; 32] = [
            0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x4b, 0x4c, 0x4d, 0x4e, 0x4f,
            0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x5b, 0x5c, 0x5d,
            0x5e, 0x5f, 0x60, 0x61,
        ];
        let sk = SigningKey::from_bytes(&seed);
        let vk = sk.verifying_key();
        (sk, vk)
    }

    fn second_keypair() -> (SigningKey, VerifyingKey) {
        let seed: [u8; 32] = [0xaa; 32];
        let sk = SigningKey::from_bytes(&seed);
        (sk.clone(), sk.verifying_key())
    }

    #[test]
    fn round_trip_signed_envelope() {
        let (sk, vk) = fresh_keypair();
        let payload = b"hello, attested world";
        let env = sign_envelope(payload, "application/vnd.test+json", &sk, "test-key");
        let json = serde_json::to_vec(&env).unwrap();
        let recovered = verify_envelope(&json, &["application/vnd.test+json"], &vk).unwrap();
        assert_eq!(recovered, payload);
    }

    #[test]
    fn rejects_payload_type_outside_accept_list() {
        let (sk, vk) = fresh_keypair();
        let env = sign_envelope(b"x", "application/vnd.bogus+json", &sk, "k");
        let json = serde_json::to_vec(&env).unwrap();
        let err = verify_envelope(&json, &["application/vnd.test+json"], &vk).unwrap_err();
        assert!(matches!(err, VerifyError::PayloadTypeMismatch { .. }));
    }

    #[test]
    fn rejects_tampered_payload() {
        let (sk, vk) = fresh_keypair();
        let mut env = sign_envelope(b"original", "application/vnd.test+json", &sk, "k");
        env.payload = B64.encode(b"tampered");
        let json = serde_json::to_vec(&env).unwrap();
        let err = verify_envelope(&json, &["application/vnd.test+json"], &vk).unwrap_err();
        assert!(matches!(err, VerifyError::SignatureVerify));
    }

    #[test]
    fn rejects_wrong_key() {
        let (sk1, _) = fresh_keypair();
        let (_, vk2) = second_keypair();
        let env = sign_envelope(b"x", "application/vnd.test+json", &sk1, "k");
        let json = serde_json::to_vec(&env).unwrap();
        let err = verify_envelope(&json, &["application/vnd.test+json"], &vk2).unwrap_err();
        assert!(matches!(err, VerifyError::SignatureVerify));
    }
}
