//! `corvid receipt` subcommand family: hash-addressed show +
//! envelope verification.
//!
//! The commands are deliberately narrow. They operate on
//! artifacts already produced by `corvid trace-diff --sign` —
//! the cache + DSSE envelope machinery lives in
//! [`crate::receipt_cache`] and [`crate::trace_diff::signing`].
//! This file is the CLI-facing glue.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use crate::receipt_cache::{find_envelope, find_receipt};
use crate::trace_diff::signing;

use corvid_abi::{
    parse_embedded_attestation_bytes, read_embedded_section_from_library,
    CORVID_ABI_ATTESTATION_PAYLOAD_TYPE, CORVID_ABI_ATTESTATION_SYMBOL,
};

/// `corvid receipt show <hash>` — resolve a receipt in the local
/// cache by its SHA-256 hash (or a unique prefix of at least 8
/// characters) and print the canonical JSON to stdout.
///
/// Exit 0 on success, non-zero on a prefix miss / ambiguity /
/// cache-read failure. The receipt is printed unchanged so
/// downstream tools can pipe the output through `jq` or similar
/// without re-parsing.
pub fn run_show(hash_prefix: &str) -> Result<u8> {
    let path = find_receipt(hash_prefix).map_err(|e| anyhow!("{e}"))?;
    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("reading cached receipt `{}`", path.display()))?;
    print!("{contents}");
    if !contents.ends_with('\n') {
        println!();
    }
    Ok(0)
}

/// `corvid receipt verify <envelope-path> --key <verify-key>` —
/// verify a DSSE envelope against a supplied ed25519 verifying
/// key and print the inner receipt payload on success. Exit 0 on
/// valid signature, non-zero on any verification or IO failure.
///
/// The `envelope_path` can also be a hash-prefix — we first check
/// the local cache for a matching `<hash>.envelope.json` and fall
/// back to treating the argument as a filesystem path.
pub fn run_verify(envelope_path_or_hash: &str, key_path: &Path) -> Result<u8> {
    let envelope_path = resolve_envelope_arg(envelope_path_or_hash);
    let envelope_bytes = std::fs::read(&envelope_path).with_context(|| {
        format!("reading envelope `{}`", envelope_path.display())
    })?;
    let verifying_key =
        signing::load_verifying_key(key_path).map_err(|e| anyhow!("{e}"))?;

    match signing::verify_envelope(&envelope_bytes, &verifying_key) {
        Ok(payload) => {
            // Re-emit the inner receipt payload as a string so
            // shell users can pipe through jq. The payload is
            // raw JSON bytes; print as-is.
            let payload_str = String::from_utf8(payload)
                .context("signed payload is not valid UTF-8")?;
            print!("{payload_str}");
            if !payload_str.ends_with('\n') {
                println!();
            }
            eprintln!("signature OK");
            Ok(0)
        }
        Err(e) => {
            eprintln!("verification failed: {e}");
            Ok(1)
        }
    }
}

/// If `arg` looks like a hex hash-prefix (>= 8 hex chars with no
/// path separators), try the cache first. Otherwise treat as a
/// filesystem path.
fn resolve_envelope_arg(arg: &str) -> PathBuf {
    if arg.len() >= 8
        && arg.chars().all(|c| c.is_ascii_hexdigit())
        && !arg.contains('/')
        && !arg.contains('\\')
    {
        if let Ok(cached) = find_envelope(arg) {
            return cached;
        }
    }
    PathBuf::from(arg)
}

/// `corvid receipt verify-abi <cdylib> --key <verify-key>` —
/// confirm a Corvid cdylib's `CORVID_ABI_ATTESTATION` symbol holds
/// a DSSE envelope whose signature verifies against the supplied
/// ed25519 public key AND whose recovered payload matches the
/// `CORVID_ABI_DESCRIPTOR` symbol's JSON.
///
/// Exit codes:
///   - 0: verified (signature valid + descriptor matches)
///   - 2: attestation symbol absent (host policy decides whether
///        an unsigned cdylib is acceptable)
///   - 1: every other failure — signature mismatch, descriptor
///        drift, malformed envelope, IO error
pub fn run_verify_abi(cdylib: &Path, key_path: &Path) -> Result<u8> {
    if !cdylib.exists() {
        return Err(anyhow!("cdylib path `{}` does not exist", cdylib.display()));
    }
    let attestation_bytes = match read_attestation_bytes(cdylib) {
        Ok(bytes) => bytes,
        Err(VerifyAbiError::Absent) => {
            eprintln!(
                "no `{}` symbol in `{}` — cdylib is unsigned",
                CORVID_ABI_ATTESTATION_SYMBOL,
                cdylib.display()
            );
            return Ok(2);
        }
        Err(VerifyAbiError::Other(e)) => return Err(e),
    };
    let parsed = parse_embedded_attestation_bytes(&attestation_bytes).map_err(|e| {
        anyhow!(
            "embedded attestation in `{}` is malformed: {e}",
            cdylib.display()
        )
    })?;
    let verifying_key =
        corvid_abi::load_verifying_key(key_path).map_err(|e| anyhow!("{e}"))?;
    let recovered_descriptor = match corvid_abi::verify_envelope(
        parsed.envelope_json.as_bytes(),
        &[CORVID_ABI_ATTESTATION_PAYLOAD_TYPE],
        &verifying_key,
    ) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("attestation verification failed: {e}");
            return Ok(1);
        }
    };

    // Confirm the recovered payload bytes match the loaded
    // descriptor section. An attacker who swapped the descriptor
    // section without swapping the matching attestation would
    // produce a valid signature over the OLD descriptor; this check
    // catches that mismatch.
    let descriptor_section = read_embedded_section_from_library(cdylib).with_context(|| {
        format!(
            "reading CORVID_ABI_DESCRIPTOR section from `{}`",
            cdylib.display()
        )
    })?;
    if descriptor_section.json.as_bytes() != recovered_descriptor.as_slice() {
        eprintln!(
            "attestation envelope signs a descriptor that does not match the loaded `CORVID_ABI_DESCRIPTOR` symbol — the binary's descriptor was tampered with after signing"
        );
        return Ok(1);
    }
    eprintln!("attestation OK ({} bytes)", recovered_descriptor.len());
    Ok(0)
}

enum VerifyAbiError {
    Absent,
    Other(anyhow::Error),
}

/// Read the `CORVID_ABI_ATTESTATION` symbol's bytes via libloading,
/// returning `Absent` when the symbol is not exported (cdylib is
/// unsigned). Other failures (IO errors, malformed section header)
/// surface as `Other`.
fn read_attestation_bytes(cdylib: &Path) -> std::result::Result<Vec<u8>, VerifyAbiError> {
    // SAFETY: loading a cdylib invokes its initializers; for a
    // Corvid cdylib this is the embedded runtime's lazy init plus
    // the static Rust globals — no host-side state is at risk
    // beyond what `Library::new` itself documents.
    let lib = unsafe { libloading::Library::new(cdylib) }.map_err(|e| {
        VerifyAbiError::Other(anyhow!("loading cdylib `{}`: {e}", cdylib.display()))
    })?;
    let header_ptr: libloading::Symbol<*const u8> =
        match unsafe { lib.get(CORVID_ABI_ATTESTATION_SYMBOL.as_bytes()) } {
            Ok(symbol) => symbol,
            Err(_) => return Err(VerifyAbiError::Absent),
        };
    let header = unsafe { std::slice::from_raw_parts(*header_ptr, 16) };
    if header.len() < 16 {
        return Err(VerifyAbiError::Other(anyhow!(
            "attestation symbol header truncated: {} bytes",
            header.len()
        )));
    }
    let envelope_len = u64::from_le_bytes(header[8..16].try_into().expect("8-byte len"));
    let total = usize::try_from(envelope_len)
        .ok()
        .and_then(|len| len.checked_add(16))
        .ok_or_else(|| {
            VerifyAbiError::Other(anyhow!(
                "attestation envelope length {envelope_len} does not fit in memory"
            ))
        })?;
    let bytes = unsafe { std::slice::from_raw_parts(*header_ptr, total) }.to_vec();
    // Keep the library mapped for the duration of the verify call —
    // `bytes` is already copied above so leaking the handle is the
    // safer move than a Drop that races with the slice borrow.
    std::mem::forget(lib);
    Ok(bytes)
}
