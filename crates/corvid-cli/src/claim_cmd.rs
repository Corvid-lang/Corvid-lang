//! `corvid claim --explain` — a quoteable, per-binary statement of
//! what a Corvid cdylib actually proves.

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use corvid_abi::{
    descriptor_from_json, parse_embedded_attestation_bytes, read_embedded_section_from_library,
    verify_envelope, CORVID_ABI_ATTESTATION_PAYLOAD_TYPE, CORVID_ABI_ATTESTATION_SYMBOL,
    CORVID_ABI_DESCRIPTOR_SYMBOL,
};
use corvid_guarantees::{GuaranteeClass, GUARANTEE_REGISTRY};
use sha2::{Digest, Sha256};

pub fn run_claim_explain(
    cdylib: &Path,
    explain: bool,
    key_path: Option<&Path>,
    source_path: Option<&Path>,
) -> Result<u8> {
    if !explain {
        bail!("`corvid claim` currently requires `--explain`");
    }
    if !cdylib.exists() {
        bail!("cdylib path `{}` does not exist", cdylib.display());
    }

    let descriptor_section = read_embedded_section_from_library(cdylib).with_context(|| {
        format!(
            "reading `{}` from `{}`",
            CORVID_ABI_DESCRIPTOR_SYMBOL,
            cdylib.display()
        )
    })?;
    let descriptor = descriptor_from_json(&descriptor_section.json)
        .context("embedded ABI descriptor JSON is malformed")?;
    descriptor
        .validate_supported_version()
        .map_err(|err| anyhow!("embedded ABI descriptor version is unsupported: {err:?}"))?;
    let descriptor_hash =
        corvid_abi_verify::hex_hash(&corvid_abi::hash_json_str(&descriptor_section.json));

    let signature = inspect_signature(cdylib, key_path)?;
    let source_agreement = inspect_source_agreement(cdylib, source_path);
    let mut exit_code = 0u8;
    if signature.failed_requested_verification() || source_agreement.failed_requested_verification()
    {
        exit_code = 1;
    }

    println!("Corvid cdylib claim explanation");
    println!("binary: {}", cdylib.display());
    println!("abi_descriptor:");
    println!("  version: {}", descriptor.corvid_abi_version);
    println!("  compiler_version: {}", descriptor.compiler_version);
    println!("  source_path: {}", descriptor.source_path);
    println!("  descriptor_sha256: {descriptor_hash}");
    println!(
        "  surface: {} agent(s), {} prompt(s), {} tool(s), {} type(s), {} store(s), {} approval site(s)",
        descriptor.agents.len(),
        descriptor.prompts.len(),
        descriptor.tools.len(),
        descriptor.types.len(),
        descriptor.stores.len(),
        descriptor.approval_sites.len()
    );
    println!("attestation:");
    for line in signature.lines() {
        println!("  {line}");
    }
    println!("source_descriptor_agreement:");
    for line in source_agreement.lines() {
        println!("  {line}");
    }
    println!("enforced_guarantees:");
    for guarantee in GUARANTEE_REGISTRY
        .iter()
        .filter(|g| g.class != GuaranteeClass::OutOfScope)
    {
        println!(
            "  - id: {}; class: {}; kind: {}; phase: {}",
            guarantee.id,
            guarantee.class.slug(),
            guarantee.kind.slug(),
            guarantee.phase.slug()
        );
    }
    println!("non_defenses:");
    for guarantee in GUARANTEE_REGISTRY
        .iter()
        .filter(|g| g.class == GuaranteeClass::OutOfScope)
    {
        println!(
            "  - id: {}; reason: {}",
            guarantee.id, guarantee.out_of_scope_reason
        );
    }

    Ok(exit_code)
}

#[derive(Debug)]
enum SignatureInspection {
    Verified {
        key_fingerprint: String,
        envelope_keyid: String,
        payload_bytes: usize,
    },
    PresentNotVerified {
        envelope_keyid: String,
    },
    AbsentNotRequested,
    AbsentRequested,
    VerificationFailed(String),
}

impl SignatureInspection {
    fn failed_requested_verification(&self) -> bool {
        matches!(self, Self::AbsentRequested | Self::VerificationFailed(_))
    }

    fn lines(&self) -> Vec<String> {
        match self {
            Self::Verified {
                key_fingerprint,
                envelope_keyid,
                payload_bytes,
            } => vec![
                "status: verified".to_string(),
                format!("signing_key_fingerprint: sha256:{key_fingerprint}"),
                format!("envelope_keyid: {envelope_keyid}"),
                format!("signed_descriptor_bytes: {payload_bytes}"),
            ],
            Self::PresentNotVerified { envelope_keyid } => vec![
                "status: present_not_verified".to_string(),
                format!("envelope_keyid: {envelope_keyid}"),
                "reason: pass `--key <pubkey>` to verify the signature".to_string(),
            ],
            Self::AbsentNotRequested => vec![
                "status: absent_not_verified".to_string(),
                "reason: cdylib does not export CORVID_ABI_ATTESTATION".to_string(),
            ],
            Self::AbsentRequested => vec![
                "status: failed".to_string(),
                "reason: cdylib does not export CORVID_ABI_ATTESTATION".to_string(),
            ],
            Self::VerificationFailed(reason) => {
                vec!["status: failed".to_string(), format!("reason: {reason}")]
            }
        }
    }
}

fn inspect_signature(cdylib: &Path, key_path: Option<&Path>) -> Result<SignatureInspection> {
    let bytes = match read_attestation_bytes(cdylib) {
        Ok(bytes) => bytes,
        Err(ReadAttestationError::Absent) if key_path.is_none() => {
            return Ok(SignatureInspection::AbsentNotRequested);
        }
        Err(ReadAttestationError::Absent) => return Ok(SignatureInspection::AbsentRequested),
        Err(ReadAttestationError::Other(err)) => return Err(err),
    };
    let parsed = parse_embedded_attestation_bytes(&bytes)
        .with_context(|| format!("embedded `{CORVID_ABI_ATTESTATION_SYMBOL}` is malformed"))?;
    let envelope: corvid_abi::DsseEnvelope = serde_json::from_str(&parsed.envelope_json)
        .context("embedded ABI attestation envelope JSON is malformed")?;
    let envelope_keyid = envelope
        .signatures
        .first()
        .map(|sig| sig.keyid.clone())
        .unwrap_or_else(|| "<none>".to_string());

    let Some(key_path) = key_path else {
        return Ok(SignatureInspection::PresentNotVerified { envelope_keyid });
    };

    let verifying_key = corvid_abi::load_verifying_key(key_path)
        .map_err(|err| anyhow!("loading verifying key `{}`: {err}", key_path.display()))?;
    match verify_envelope(
        parsed.envelope_json.as_bytes(),
        &[CORVID_ABI_ATTESTATION_PAYLOAD_TYPE],
        &verifying_key,
    ) {
        Ok(payload) => Ok(SignatureInspection::Verified {
            key_fingerprint: hex::encode(Sha256::digest(verifying_key.as_bytes())),
            envelope_keyid,
            payload_bytes: payload.len(),
        }),
        Err(err) => Ok(SignatureInspection::VerificationFailed(err.to_string())),
    }
}

#[derive(Debug)]
enum SourceAgreementInspection {
    Verified {
        source_hash: String,
        embedded_hash: String,
        bytes: usize,
    },
    NotRequested,
    Failed(String),
}

impl SourceAgreementInspection {
    fn failed_requested_verification(&self) -> bool {
        matches!(self, Self::Failed(_))
    }

    fn lines(&self) -> Vec<String> {
        match self {
            Self::Verified {
                source_hash,
                embedded_hash,
                bytes,
            } => vec![
                "status: verified".to_string(),
                format!("source_descriptor_sha256: {source_hash}"),
                format!("embedded_descriptor_sha256: {embedded_hash}"),
                format!("descriptor_bytes: {bytes}"),
            ],
            Self::NotRequested => vec![
                "status: not_verified".to_string(),
                "reason: pass `--source <file.cor>` to rebuild and compare the descriptor"
                    .to_string(),
            ],
            Self::Failed(reason) => vec!["status: failed".to_string(), format!("reason: {reason}")],
        }
    }
}

fn inspect_source_agreement(
    cdylib: &Path,
    source_path: Option<&Path>,
) -> SourceAgreementInspection {
    let Some(source_path) = source_path else {
        return SourceAgreementInspection::NotRequested;
    };
    match corvid_abi_verify::verify_source_matches_cdylib(source_path, cdylib) {
        Ok(report) => SourceAgreementInspection::Verified {
            source_hash: corvid_abi_verify::hex_hash(&report.source_json_hash),
            embedded_hash: corvid_abi_verify::hex_hash(&report.embedded_json_hash),
            bytes: report.embedded_json_len,
        },
        Err(err) => SourceAgreementInspection::Failed(err.to_string()),
    }
}

enum ReadAttestationError {
    Absent,
    Other(anyhow::Error),
}

fn read_attestation_bytes(cdylib: &Path) -> std::result::Result<Vec<u8>, ReadAttestationError> {
    let lib = unsafe { libloading::Library::new(cdylib) }.map_err(|err| {
        ReadAttestationError::Other(anyhow!("loading cdylib `{}`: {err}", cdylib.display()))
    })?;
    let header_ptr: libloading::Symbol<*const u8> =
        match unsafe { lib.get(CORVID_ABI_ATTESTATION_SYMBOL.as_bytes()) } {
            Ok(symbol) => symbol,
            Err(_) => return Err(ReadAttestationError::Absent),
        };
    let header = unsafe { std::slice::from_raw_parts(*header_ptr, 16) };
    let envelope_len = u64::from_le_bytes(header[8..16].try_into().expect("8-byte len"));
    let total = usize::try_from(envelope_len)
        .ok()
        .and_then(|len| len.checked_add(16))
        .ok_or_else(|| {
            ReadAttestationError::Other(anyhow!(
                "attestation envelope length {envelope_len} does not fit in memory"
            ))
        })?;
    let bytes = unsafe { std::slice::from_raw_parts(*header_ptr, total) }.to_vec();
    std::mem::forget(lib);
    Ok(bytes)
}
