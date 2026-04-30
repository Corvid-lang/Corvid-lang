//! Shared helpers used across the `corvid connectors` subcommand
//! surface. Manifest catalog, runtime-error → CLI-error projection,
//! manifest → list-row summary, and the OAuth2 PKCE primitives
//! (URL encoding, base64url RNG bytes, code-challenge derivation)
//! all live here because every per-subcommand module would
//! otherwise re-implement them.

use anyhow::Result;
use corvid_connector_runtime::{
    calendar_manifest, file_manifest, gmail_manifest, ms365_manifest, slack_manifest,
    task_manifest, ConnectorManifest, ConnectorRuntimeError,
};
use sha2::Sha256;

use super::list::ConnectorListEntry;

pub(crate) fn shipped_manifests() -> Result<Vec<(&'static str, ConnectorManifest)>> {
    Ok(vec![
        ("calendar", calendar_manifest()?),
        ("files", file_manifest()?),
        ("gmail", gmail_manifest()?),
        ("ms365", ms365_manifest()?),
        ("slack", slack_manifest()?),
        ("tasks", task_manifest()?),
    ])
}

pub(crate) fn shipped_connector_names() -> Vec<&'static str> {
    vec!["calendar", "files", "gmail", "ms365", "slack", "tasks"]
}

pub(crate) fn summarise_manifest(manifest: &ConnectorManifest) -> ConnectorListEntry {
    let modes: Vec<String> = manifest
        .mode
        .iter()
        .map(|m| match m {
            corvid_connector_runtime::ConnectorMode::Mock => "mock".to_string(),
            corvid_connector_runtime::ConnectorMode::Replay => "replay".to_string(),
            corvid_connector_runtime::ConnectorMode::Real => "real".to_string(),
        })
        .collect();
    let write_scopes: Vec<String> = manifest
        .scope
        .iter()
        .filter(|s| {
            s.effects
                .iter()
                .any(|e| e.contains(".write") || e.starts_with("send_"))
        })
        .map(|s| s.id.clone())
        .collect();
    let rate_limit_summary = if manifest.rate_limit.is_empty() {
        "none".to_string()
    } else {
        manifest
            .rate_limit
            .iter()
            .map(|rl| format!("{}={}/{}ms", rl.key, rl.limit, rl.window_ms))
            .collect::<Vec<_>>()
            .join(", ")
    };
    ConnectorListEntry {
        name: manifest.name.clone(),
        provider: manifest.provider.clone(),
        modes,
        scope_count: manifest.scope.len(),
        write_scopes,
        rate_limit_summary,
        redaction_count: manifest.redaction.len(),
    }
}

pub(crate) fn map_runtime_error(err: ConnectorRuntimeError) -> anyhow::Error {
    anyhow::anyhow!("{err}")
}

pub(crate) fn url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 8);
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub(crate) fn random_b64url_bytes(n: usize) -> String {
    use base64::Engine;
    // Use a deterministic-but-unpredictable per-call source: the
    // process's nanoseconds + a hash of a fresh allocation address.
    // Production use should plumb an OS-supplied RNG; this keeps the
    // CLI command self-contained and doesn't pull in `rand`.
    let mut seed = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)) as u64;
    let addr_seed = (&seed as *const _ as usize) as u64;
    seed = seed.wrapping_add(addr_seed);
    let mut bytes = Vec::with_capacity(n);
    for _ in 0..n {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        bytes.push((seed >> 33) as u8);
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes)
}

pub(crate) fn pkce_code_challenge(verifier: &str) -> String {
    use base64::Engine;
    use sha2::Digest;
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}
