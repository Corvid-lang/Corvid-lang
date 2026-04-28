//! DSSE-signed receipts for `corvid trace-diff`.
//!
//! Adds a thin cryptographic layer on top of the canonical
//! [`Receipt`](super::receipt::Receipt) JSON output. A signed
//! receipt is a DSSE (Dead Simple Signing Envelope) envelope:
//!
//! ```json
//! {
//!   "payloadType": "application/vnd.corvid-receipt+json",
//!   "payload": "<base64 of the receipt JSON>",
//!   "signatures": [{
//!     "keyid": "<opaque key identifier>",
//!     "sig": "<base64 of the ed25519 signature over PAE(payloadType, payload)>"
//!   }]
//! }
//! ```
//!
//! The signature is an ed25519 signature over the DSSE
//! Pre-Authentication Encoding (PAE) of the payload, per the DSSE
//! v1 spec. That gives two security properties:
//!
//! 1. The signature binds both the payload *and* its type, so an
//!    attacker who crafts an envelope with a different
//!    `payloadType` can't replay the signature.
//! 2. PAE includes explicit length prefixes, so there's no
//!    ambiguity between "long payloadType + short payload" vs
//!    "short payloadType + long payload" — a class of length-
//!    extension attacks this encoding neutralises.
//!
//! Why DSSE rather than a hand-rolled format: the format is used
//! by Sigstore, in-toto, cosign, and the rest of the supply-chain
//! ecosystem. Corvid receipts plug into that ecosystem for free —
//! and the follow-up slice `21-inv-H-5-in-toto` wraps this exact
//! envelope in a specific in-toto Statement for SLSA attestations.
//!
//! Scope deliberately excluded from v1 (`21-inv-H-5-signed`):
//!
//! - **Keyless / Sigstore OIDC flow.** Requires a network round-
//!   trip to a Fulcio-like CA. Separate slice.
//! - **Multi-signature / threshold signatures.** The envelope
//!   schema supports multiple entries in `signatures`; today we
//!   only produce and verify single-sig envelopes.
//! - **Certificate chains.** v1 verifies against a raw ed25519
//!   public key. Keyless / Sigstore adds certs on top.
//! - **Transparency log integration.** Rekor / similar ledgers
//!   compose with DSSE but are independent.
//! - **RSA / ECDSA / other keytypes.** ed25519 is the default;
//!   other keytypes add implementation surface without obvious
//!   v1 demand.

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use ed25519_dalek::{ed25519::signature::Signer, Signature, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

/// The `payloadType` URI we sign. Registering a specific media
/// type binds the signature to Corvid-receipt-shaped JSON — a
/// verifier that encounters a different payloadType will refuse
/// to interpret the envelope as a receipt even if the signature
/// is otherwise valid.
pub(super) const CORVID_RECEIPT_PAYLOAD_TYPE: &str = "application/vnd.corvid-receipt+json";

/// A single signature entry inside a DSSE envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct DsseSignature {
    /// Opaque key identifier. Free-form — typically the hex
    /// prefix of the verifying key, a fingerprint, a KMS key
    /// name, or a user-supplied label.
    pub keyid: String,
    /// Base64-encoded signature bytes.
    pub sig: String,
}

/// DSSE v1 envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct DsseEnvelope {
    #[serde(rename = "payloadType")]
    pub payload_type: String,
    /// Base64-encoded payload bytes. For Corvid receipts this is
    /// the canonical JSON representation as emitted by
    /// [`super::receipt::render_json`].
    pub payload: String,
    pub signatures: Vec<DsseSignature>,
}

/// Errors surfaced by the signing / verification path.
#[allow(dead_code)]
#[derive(Debug)]
pub(super) enum SignError {
    KeyFileRead {
        path: std::path::PathBuf,
        error: std::io::Error,
    },
    /// The provided key material doesn't parse as a 32-byte
    /// ed25519 seed in the expected encoding (hex or raw
    /// bytes).
    KeyFormat {
        path_or_source: String,
        reason: String,
    },
    /// No key material was supplied. Either pass `--sign=<path>`
    /// or set `CORVID_SIGNING_KEY`.
    NoKey,
}

#[derive(Debug)]
pub(crate) enum VerifyError {
    EnvelopeJson(serde_json::Error),
    /// Envelope's `payloadType` doesn't match the receipt type —
    /// the envelope is well-formed but binds a different
    /// document type.
    PayloadTypeMismatch {
        got: String,
        expected: String,
    },
    /// Payload base64 decode failed.
    PayloadDecode(base64::DecodeError),
    /// Signature base64 decode failed.
    SignatureDecode(base64::DecodeError),
    /// Envelope has zero signatures.
    NoSignatures,
    /// No signature in the envelope verified against the
    /// supplied public key. This covers both tampered payloads
    /// and wrong-key cases — they're cryptographically
    /// indistinguishable and deserve the same failure mode.
    SignatureVerify,
    /// The supplied verifying key didn't parse as 32-byte
    /// ed25519 public key material.
    VerifyKeyFormat { reason: String },
    /// Reading the envelope or key from disk failed.
    Io { path: std::path::PathBuf, error: std::io::Error },
}

impl std::fmt::Display for SignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KeyFileRead { path, error } => {
                write!(f, "cannot read signing key file `{}`: {error}", path.display())
            }
            Self::KeyFormat { path_or_source, reason } => {
                write!(
                    f,
                    "signing key from `{path_or_source}` is not a valid ed25519 seed: {reason}"
                )
            }
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
                "payload type `{got}` does not match expected `{expected}` — this envelope does not claim to be a Corvid receipt"
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
            Self::Io { path, error } => {
                write!(f, "io error reading `{}`: {error}", path.display())
            }
        }
    }
}

impl std::error::Error for VerifyError {}

/// DSSE v1 Pre-Authentication Encoding. The signature is made
/// over these bytes (not the raw payload), so a verifier can't
/// be tricked by a crafted envelope with a different payload-
/// type that encodes to the same bytes as a real signed payload.
///
/// Format from the DSSE spec:
///
/// ```text
/// DSSEv1 SP LEN(type) SP type SP LEN(payload) SP payload
/// ```
///
/// where LEN(...) is the byte length as decimal ASCII and SP is
/// a single space character.
fn pae(payload_type: &str, payload: &[u8]) -> Vec<u8> {
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
pub(super) enum KeySource {
    /// A file on disk containing the signing key material.
    Path(std::path::PathBuf),
    /// The `CORVID_SIGNING_KEY` env var is set — read from there.
    /// Value is the env var's content, not the variable name, so
    /// the CLI layer reads the env var once at flag-parse time.
    Env(String),
}

/// Load a signing key from the given source. Accepts two
/// encodings:
///
/// - **Hex**: 64 hex characters encoding 32 bytes. This is the
///   default output of `openssl rand -hex 32` and mirrors the
///   ed25519-dalek seed format. Whitespace is trimmed.
/// - **Raw**: exactly 32 bytes. Used when the key file was
///   written as raw binary rather than hex.
///
/// PEM is deferred — adds a parser dependency for negligible v1
/// payoff. The follow-up keyless slice (`21-inv-H-5-keyless`)
/// will replace file-backed keys with Sigstore OIDC anyway.
pub(super) fn load_signing_key(source: &KeySource) -> Result<SigningKey, SignError> {
    let (bytes, source_label) = match source {
        KeySource::Path(p) => {
            let contents = std::fs::read(p).map_err(|error| SignError::KeyFileRead {
                path: p.clone(),
                error,
            })?;
            (contents, p.display().to_string())
        }
        KeySource::Env(v) => (v.as_bytes().to_vec(), "CORVID_SIGNING_KEY env var".to_string()),
    };

    let seed = decode_ed25519_seed(&bytes, &source_label)?;
    Ok(SigningKey::from_bytes(&seed))
}

/// Interpret key bytes as either hex-encoded or raw 32-byte
/// seed. Whitespace is stripped before parsing so `echo <hex>` +
/// trailing newline parse cleanly.
fn decode_ed25519_seed(raw: &[u8], source_label: &str) -> Result<[u8; 32], SignError> {
    let trimmed = raw
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect::<Vec<u8>>();

    // Try hex first if the bytes look textual.
    if trimmed.iter().all(|b| b.is_ascii_hexdigit()) && trimmed.len() == 64 {
        let mut out = [0u8; 32];
        hex::decode_to_slice(&trimmed, &mut out).map_err(|e| SignError::KeyFormat {
            path_or_source: source_label.to_string(),
            reason: format!("hex decode failed: {e}"),
        })?;
        return Ok(out);
    }

    // Fall back to raw 32 bytes.
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

/// Sign a payload with the given `payload_type` + return the DSSE
/// envelope. The signature covers the PAE of `(payload_type,
/// payload)`, so an attacker can't replay the signature under a
/// different payloadType. Caller supplies the payloadType so this
/// function works for both Corvid receipts
/// (`application/vnd.corvid-receipt+json`) and in-toto Statements
/// (`application/vnd.in-toto+json`) without branching.
pub(super) fn sign_envelope(
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

/// Backward-compatible wrapper: sign a Corvid receipt payload
/// with the `application/vnd.corvid-receipt+json` payloadType.
/// Retained so existing callers + tests don't break.
#[cfg(test)]
pub(super) fn sign_receipt(payload: &[u8], key: &SigningKey, key_id: &str) -> DsseEnvelope {
    sign_envelope(payload, CORVID_RECEIPT_PAYLOAD_TYPE, key, key_id)
}

/// The set of payloadTypes we accept as known-good at verify
/// time. A well-formed envelope with a payloadType outside this
/// set is rejected — unknown types could be anything, and we
/// shouldn't interpret them as Corvid-meaningful.
const ACCEPTED_PAYLOAD_TYPES: &[&str] = &[
    CORVID_RECEIPT_PAYLOAD_TYPE,
    // `application/vnd.in-toto+json` for in-toto Statements
    // (21-inv-H-5-in-toto). Kept as a literal rather than a
    // cross-module import to avoid a circular dependency.
    "application/vnd.in-toto+json",
];

/// Parse a DSSE envelope from JSON bytes, verify every included
/// signature against the supplied verifying key, and return the
/// base64-decoded payload on success. A single valid signature
/// against the given key suffices — extra signatures from other
/// keys don't cause rejection. Accepts any payloadType in
/// [`ACCEPTED_PAYLOAD_TYPES`] — the caller decides what to do
/// with the returned payload based on the envelope's type.
pub(crate) fn verify_envelope(
    envelope_json: &[u8],
    key: &VerifyingKey,
) -> Result<Vec<u8>, VerifyError> {
    let envelope: DsseEnvelope =
        serde_json::from_slice(envelope_json).map_err(VerifyError::EnvelopeJson)?;

    if !ACCEPTED_PAYLOAD_TYPES.contains(&envelope.payload_type.as_str()) {
        return Err(VerifyError::PayloadTypeMismatch {
            got: envelope.payload_type.clone(),
            expected: ACCEPTED_PAYLOAD_TYPES.join(" or "),
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
            // Malformed signature length — not an ed25519 sig.
            // Treat as verification failure so a malicious
            // envelope with padded/truncated sigs gets rejected
            // uniformly.
            continue;
        }
        let sig_arr: [u8; 64] = sig_bytes
            .as_slice()
            .try_into()
            .expect("already checked length");
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

/// Parse an ed25519 verifying key from a file. Accepts the same
/// hex/raw formats as [`load_signing_key`], but for the 32-byte
/// public key.
pub(crate) fn load_verifying_key(path: &std::path::Path) -> Result<VerifyingKey, VerifyError> {
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
        reason: e.to_string(),
    })
}

/// Serialise an envelope to pretty JSON with a trailing newline.
/// Convenience wrapper so the CLI's output path stays uniform
/// across all formats (each ends with `\n`).
pub(super) fn envelope_to_json(envelope: &DsseEnvelope) -> String {
    let mut s = serde_json::to_string_pretty(envelope)
        .expect("DsseEnvelope is trivially serializable");
    s.push('\n');
    s
}

/// Locate the signing-key source from CLI flag + env var. File
/// path wins when both are provided (explicit beats implicit).
/// Returns `None` when neither is set — the caller decides
/// whether that's an error (under `--sign` explicitly) or just
/// "no signing configured" (default behavior).
pub(super) fn resolve_key_source(cli_path: Option<&std::path::Path>) -> Option<KeySource> {
    if let Some(p) = cli_path {
        return Some(KeySource::Path(p.to_path_buf()));
    }
    if let Ok(v) = std::env::var("CORVID_SIGNING_KEY") {
        if !v.is_empty() {
            return Some(KeySource::Env(v));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    /// Deterministic test keys. Each call produces a different
    /// key by seeding from the supplied `label` byte, so tests
    /// that want "two distinct keys" can use `test_key(1)` and
    /// `test_key(2)`. No RNG dependency — tests fail
    /// deterministically.
    fn test_key(label: u8) -> SigningKey {
        let mut seed = [0u8; 32];
        seed[0] = label;
        SigningKey::from_bytes(&seed)
    }

    fn fresh_key() -> SigningKey {
        test_key(7)
    }

    #[test]
    fn pae_matches_dsse_v1_spec() {
        // Example from the DSSE spec:
        // payloadType = "https://in-toto.io/Statement/v1"
        // payload = "{}"
        let pae_bytes = pae("https://in-toto.io/Statement/v1", b"{}");
        let expected = b"DSSEv1 31 https://in-toto.io/Statement/v1 2 {}";
        assert_eq!(pae_bytes, expected);
    }

    #[test]
    fn sign_then_verify_roundtrips() {
        let key = fresh_key();
        let payload = br#"{"schema_version": 1, "verdict": {"ok": true}}"#;
        let envelope = sign_receipt(payload, &key, "test-key");
        let envelope_json = envelope_to_json(&envelope);

        let recovered = verify_envelope(envelope_json.as_bytes(), &key.verifying_key())
            .expect("verify roundtrip");
        assert_eq!(recovered, payload);
    }

    #[test]
    fn tampered_payload_fails_verification() {
        let key = fresh_key();
        let original = br#"{"verdict": {"ok": true}}"#;
        let envelope = sign_receipt(original, &key, "test-key");

        // Tamper: flip the verdict from true to false by editing
        // the base64 payload. We re-encode a different payload
        // under the ORIGINAL envelope's signature.
        let tampered_payload = br#"{"verdict": {"ok": false}}"#;
        let tampered = DsseEnvelope {
            payload_type: envelope.payload_type.clone(),
            payload: B64.encode(tampered_payload),
            signatures: envelope.signatures.clone(),
        };
        let tampered_json = envelope_to_json(&tampered);

        assert!(matches!(
            verify_envelope(tampered_json.as_bytes(), &key.verifying_key()),
            Err(VerifyError::SignatureVerify)
        ));
    }

    #[test]
    fn wrong_key_fails_verification() {
        let signing_key = test_key(1);
        let other_key = test_key(2);
        let payload = br#"{"verdict": {"ok": true}}"#;
        let envelope = sign_receipt(payload, &signing_key, "k1");
        let envelope_json = envelope_to_json(&envelope);

        // Verify with an unrelated key — same failure mode as
        // tampered payload (cryptographically indistinguishable
        // and the error type reflects that).
        assert!(matches!(
            verify_envelope(envelope_json.as_bytes(), &other_key.verifying_key()),
            Err(VerifyError::SignatureVerify)
        ));
    }

    #[test]
    fn payload_type_mismatch_is_rejected() {
        let key = fresh_key();
        let payload = b"{}";
        let mut envelope = sign_receipt(payload, &key, "k1");
        envelope.payload_type = "application/vnd.not-a-corvid-receipt+json".into();
        let envelope_json = envelope_to_json(&envelope);

        assert!(matches!(
            verify_envelope(envelope_json.as_bytes(), &key.verifying_key()),
            Err(VerifyError::PayloadTypeMismatch { .. })
        ));
    }

    #[test]
    fn empty_signatures_list_is_rejected() {
        let key = fresh_key();
        let mut envelope = sign_receipt(b"{}", &key, "k1");
        envelope.signatures.clear();
        let envelope_json = envelope_to_json(&envelope);

        assert!(matches!(
            verify_envelope(envelope_json.as_bytes(), &key.verifying_key()),
            Err(VerifyError::NoSignatures)
        ));
    }

    #[test]
    fn hex_encoded_seed_round_trips() {
        let key = fresh_key();
        let hex_seed = hex::encode(key.to_bytes());
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("key.hex");
        std::fs::write(&path, &hex_seed).unwrap();

        let loaded = load_signing_key(&KeySource::Path(path)).unwrap();
        assert_eq!(loaded.to_bytes(), key.to_bytes());
    }

    #[test]
    fn hex_seed_with_trailing_newline_is_accepted() {
        let key = fresh_key();
        let hex_seed = format!("{}\n", hex::encode(key.to_bytes()));
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("key.hex");
        std::fs::write(&path, hex_seed).unwrap();

        let loaded = load_signing_key(&KeySource::Path(path)).unwrap();
        assert_eq!(loaded.to_bytes(), key.to_bytes());
    }

    #[test]
    fn raw_32_byte_seed_works() {
        let key = fresh_key();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("key.raw");
        std::fs::write(&path, key.to_bytes()).unwrap();

        let loaded = load_signing_key(&KeySource::Path(path)).unwrap();
        assert_eq!(loaded.to_bytes(), key.to_bytes());
    }

    #[test]
    fn malformed_key_produces_typed_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bogus.key");
        std::fs::write(&path, b"definitely not a valid ed25519 seed").unwrap();

        let err = load_signing_key(&KeySource::Path(path)).unwrap_err();
        assert!(matches!(err, SignError::KeyFormat { .. }));
    }

    #[test]
    fn env_var_source_works() {
        let key = fresh_key();
        let hex_seed = hex::encode(key.to_bytes());
        let loaded = load_signing_key(&KeySource::Env(hex_seed)).unwrap();
        assert_eq!(loaded.to_bytes(), key.to_bytes());
    }

    #[test]
    fn resolve_key_source_prefers_cli_path_over_env() {
        // Can't reliably mutate env in a test-parallel context,
        // so just assert the `Some(path)` branch wins when
        // provided.
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("k");
        let src = resolve_key_source(Some(&p)).expect("path wins");
        assert!(matches!(src, KeySource::Path(_)));
    }
}
