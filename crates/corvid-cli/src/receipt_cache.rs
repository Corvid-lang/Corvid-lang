//! Hash-addressed local cache for signed Corvid receipts.
//!
//! When `corvid trace-diff --sign` produces a DSSE envelope, the
//! CLI also writes a copy of the envelope + the inner receipt
//! into a filesystem cache keyed by the SHA-256 of the receipt's
//! canonical JSON bytes. A later `corvid receipt show <hash>`
//! resolves against that cache, giving operators a shell-friendly
//! way to ask "show me the exact receipt this signature commits
//! to."
//!
//! Cache layout:
//!
//! ```text
//! <cache_dir>/corvid/receipts/<sha256>.receipt.json
//! <cache_dir>/corvid/receipts/<sha256>.envelope.json
//! ```
//!
//! `<cache_dir>` comes from [`dirs::cache_dir`] — resolves to
//! `~/.cache` on Linux, `~/Library/Caches` on macOS, and
//! `%LOCALAPPDATA%` on Windows. Users who want a portable cache
//! location can set `CORVID_RECEIPT_CACHE_DIR` to override.
//!
//! The receipt is stored separately from the envelope so
//! `receipt show` can print the inner JSON without stripping
//! signature noise. Both files share a hash prefix so they're
//! easy to locate together via `ls <cache>/corvid/receipts/`.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Resolved cache root (the `<cache_dir>/corvid/receipts/`
/// directory). Creates the directory tree on first use so callers
/// don't have to worry about it.
pub(crate) fn cache_dir() -> std::io::Result<PathBuf> {
    let root = if let Ok(override_dir) = std::env::var("CORVID_RECEIPT_CACHE_DIR") {
        PathBuf::from(override_dir)
    } else {
        dirs::cache_dir()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no platform cache dir available; set `CORVID_RECEIPT_CACHE_DIR`",
                )
            })?
            .join("corvid")
            .join("receipts")
    };
    std::fs::create_dir_all(&root)?;
    Ok(root)
}

/// Hex-encoded SHA-256 of the receipt payload. Deterministic for
/// a given byte sequence — two `corvid trace-diff --sign` calls
/// on the same base/head SHAs + impact produce the same hash.
pub(crate) fn receipt_hash(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(payload);
    hex::encode(hasher.finalize())
}

/// Store both the receipt JSON and its signed envelope under the
/// hash of the receipt. Returns the hash + cache paths so the
/// caller can surface them (e.g. printing `Corvid-Receipt: <hash>`
/// to stderr).
pub(crate) fn store(
    receipt_json: &[u8],
    envelope_json: &[u8],
) -> std::io::Result<StoredReceipt> {
    let hash = receipt_hash(receipt_json);
    let root = cache_dir()?;
    let receipt_path = root.join(format!("{hash}.receipt.json"));
    let envelope_path = root.join(format!("{hash}.envelope.json"));
    std::fs::write(&receipt_path, receipt_json)?;
    std::fs::write(&envelope_path, envelope_json)?;
    Ok(StoredReceipt {
        hash,
        receipt_path,
        envelope_path,
    })
}

pub(crate) struct StoredReceipt {
    pub(crate) hash: String,
    pub(crate) receipt_path: PathBuf,
    pub(crate) envelope_path: PathBuf,
}

/// Resolve a hash to the on-disk receipt path. Accepts full
/// 64-character hashes OR short prefixes (min 8 chars) for
/// ergonomic CLI use — `corvid receipt show abc12345` finds
/// `abc12345*.receipt.json` if unique, errors if ambiguous.
pub(crate) fn find_receipt(hash_prefix: &str) -> Result<PathBuf, ReceiptLookupError> {
    if hash_prefix.len() < 8 {
        return Err(ReceiptLookupError::PrefixTooShort {
            got: hash_prefix.len(),
        });
    }
    let root = cache_dir().map_err(|error| ReceiptLookupError::CacheIo {
        message: error.to_string(),
    })?;
    resolve_prefix(&root, hash_prefix, "receipt.json")
}

/// Sibling lookup for the envelope file — used by
/// `receipt verify <hash>` once it ships, and by debug tooling.
pub(crate) fn find_envelope(hash_prefix: &str) -> Result<PathBuf, ReceiptLookupError> {
    if hash_prefix.len() < 8 {
        return Err(ReceiptLookupError::PrefixTooShort {
            got: hash_prefix.len(),
        });
    }
    let root = cache_dir().map_err(|error| ReceiptLookupError::CacheIo {
        message: error.to_string(),
    })?;
    resolve_prefix(&root, hash_prefix, "envelope.json")
}

fn resolve_prefix(
    root: &Path,
    prefix: &str,
    suffix: &str,
) -> Result<PathBuf, ReceiptLookupError> {
    let mut matches = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|error| ReceiptLookupError::CacheIo {
        message: error.to_string(),
    })? {
        let entry = entry.map_err(|error| ReceiptLookupError::CacheIo {
            message: error.to_string(),
        })?;
        let name = entry.file_name();
        let name_str = match name.to_str() {
            Some(s) => s,
            None => continue,
        };
        if name_str.starts_with(prefix) && name_str.ends_with(suffix) {
            matches.push(entry.path());
        }
    }
    match matches.len() {
        0 => Err(ReceiptLookupError::NotFound {
            prefix: prefix.to_string(),
        }),
        1 => Ok(matches.into_iter().next().unwrap()),
        n => {
            let names: Vec<String> = matches
                .iter()
                .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
                .collect();
            Err(ReceiptLookupError::Ambiguous {
                prefix: prefix.to_string(),
                count: n,
                sample: names.into_iter().take(5).collect(),
            })
        }
    }
}

#[derive(Debug)]
pub(crate) enum ReceiptLookupError {
    PrefixTooShort { got: usize },
    NotFound { prefix: String },
    Ambiguous { prefix: String, count: usize, sample: Vec<String> },
    CacheIo { message: String },
}

impl std::fmt::Display for ReceiptLookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PrefixTooShort { got } => write!(
                f,
                "hash prefix must be at least 8 characters (got {got}); a sha256 is 64 hex chars"
            ),
            Self::NotFound { prefix } => write!(
                f,
                "no receipt with hash prefix `{prefix}` in the local cache; sign one with `corvid trace-diff --sign=<key>` first"
            ),
            Self::Ambiguous { prefix, count, sample } => write!(
                f,
                "hash prefix `{prefix}` matches {count} receipts: {} — use more characters to disambiguate",
                sample.join(", ")
            ),
            Self::CacheIo { message } => write!(f, "receipt cache IO error: {message}"),
        }
    }
}

impl std::error::Error for ReceiptLookupError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_hash_is_deterministic() {
        let payload = br#"{"schema_version": 1}"#;
        let h1 = receipt_hash(payload);
        let h2 = receipt_hash(payload);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64, "sha256 hex is 64 chars");
    }

    #[test]
    fn receipt_hash_differs_for_different_payloads() {
        let h_a = receipt_hash(br#"{"verdict": {"ok": true}}"#);
        let h_b = receipt_hash(br#"{"verdict": {"ok": false}}"#);
        assert_ne!(h_a, h_b);
    }

    #[test]
    fn store_and_find_roundtrip() {
        // Route the cache at a temp dir for test isolation.
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("CORVID_RECEIPT_CACHE_DIR", tmp.path());
        let receipt = br#"{"schema_version": 1, "verdict": {"ok": true}}"#;
        let envelope = br#"{"payloadType": "...", "payload": "...", "signatures": []}"#;

        let stored = store(receipt, envelope).unwrap();
        assert_eq!(stored.hash.len(), 64);
        assert!(stored.receipt_path.exists());
        assert!(stored.envelope_path.exists());

        // Full hash lookup
        let found = find_receipt(&stored.hash).unwrap();
        assert_eq!(found, stored.receipt_path);

        // Prefix lookup
        let found_prefix = find_receipt(&stored.hash[..12]).unwrap();
        assert_eq!(found_prefix, stored.receipt_path);

        std::env::remove_var("CORVID_RECEIPT_CACHE_DIR");
    }

    #[test]
    fn short_prefix_errors_with_hint() {
        let err = find_receipt("abc").unwrap_err();
        assert!(matches!(err, ReceiptLookupError::PrefixTooShort { .. }));
    }
}
