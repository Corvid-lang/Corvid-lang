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
