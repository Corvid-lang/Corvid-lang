use anyhow::{bail, Context, Result};
use base64::Engine as _;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct DsseEnvelope {
    #[serde(rename = "payloadType")]
    pub payload_type: String,
    pub payload: String,
    pub signatures: Vec<DsseSignature>,
}

#[derive(Debug, Deserialize)]
pub struct DsseSignature {
    #[allow(dead_code)]
    pub keyid: String,
    pub sig: String,
}

pub fn read_envelope(path: &Path) -> Result<DsseEnvelope> {
    let envelope_bytes =
        fs::read(path).with_context(|| format!("read receipt envelope `{}`", path.display()))?;
    serde_json::from_slice(&envelope_bytes)
        .with_context(|| format!("parse receipt envelope `{}`", path.display()))
}

pub fn verify_dsse_envelope(envelope_path: &Path, key_path: &Path) -> Result<Vec<u8>> {
    let envelope = read_envelope(envelope_path)?;
    if envelope.signatures.is_empty() {
        bail!(
            "BundleSignatureVerifyFailed: `{}` contains no signatures",
            envelope_path.display()
        );
    }

    let key = load_verifying_key(key_path)?;
    let payload = base64::engine::general_purpose::STANDARD
        .decode(envelope.payload.as_bytes())
        .context("decode envelope payload")?;
    let pae = pae(&envelope.payload_type, &payload);
    let mut any_valid = false;
    for signature in &envelope.signatures {
        let sig_bytes = base64::engine::general_purpose::STANDARD
            .decode(signature.sig.as_bytes())
            .context("decode envelope signature")?;
        if sig_bytes.len() != 64 {
            continue;
        }
        let sig = Signature::from_bytes(
            &sig_bytes
                .as_slice()
                .try_into()
                .expect("length checked above"),
        );
        if key.verify(&pae, &sig).is_ok() {
            any_valid = true;
            break;
        }
    }
    if !any_valid {
        bail!(
            "BundleSignatureVerifyFailed: `{}` did not verify against `{}`",
            envelope_path.display(),
            key_path.display()
        );
    }
    Ok(payload)
}

pub fn load_verifying_key(path: &Path) -> Result<VerifyingKey> {
    let raw = fs::read(path).with_context(|| format!("read verifying key `{}`", path.display()))?;
    let trimmed = raw
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<u8>>();
    let bytes: [u8; 32] = if trimmed.iter().all(|byte| byte.is_ascii_hexdigit()) && trimmed.len() == 64 {
        let mut out = [0u8; 32];
        hex::decode_to_slice(&trimmed, &mut out).context("hex decode verifying key")?;
        out
    } else if raw.len() == 32 {
        let mut out = [0u8; 32];
        out.copy_from_slice(&raw);
        out
    } else {
        bail!(
            "BundleSignatureVerifyFailed: `{}` is not a 32-byte ed25519 verifying key",
            path.display()
        );
    };
    VerifyingKey::from_bytes(&bytes).context("parse verifying key")
}

pub fn pae(payload_type: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload_type.len() + payload.len() + 32);
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
