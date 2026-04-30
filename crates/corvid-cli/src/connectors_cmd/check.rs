//! `corvid connectors check [--live]` — manifest validation.
//!
//! Default mode validates every shipped manifest against the
//! manifest schema and reports per-connector diagnostics. The
//! `--live` mode is reserved for the live drift narrator that
//! compares manifest schema to a real provider response shape;
//! until that lands, `--live` returns an explicit `Err` directing
//! the caller to rerun without `--live` (per the Phase 20j roadmap
//! audit-correction track).

use anyhow::{anyhow, Result};
use corvid_connector_runtime::validate_connector_manifest;

use super::support::shipped_manifests;

#[derive(Debug, Clone, PartialEq)]
pub struct ConnectorCheckEntry {
    pub name: String,
    pub valid: bool,
    pub diagnostics: Vec<String>,
}

/// Validates every shipped connector manifest and returns one
/// `ConnectorCheckEntry` per connector. With `live = true` the
/// caller indicates real-provider drift detection should run; this
/// slice flags it as a deferred bounty-extension behaviour and the
/// per-connector live drift narrator lands in slice 41M alongside
/// the webhook receive end-to-end.
pub fn run_check(live: bool) -> Result<Vec<ConnectorCheckEntry>> {
    let manifests = shipped_manifests()?;
    let mut entries = Vec::with_capacity(manifests.len());
    for (name, manifest) in manifests {
        let report = validate_connector_manifest(&manifest);
        let diagnostics = report
            .diagnostics
            .iter()
            .map(|d| format!("{d}"))
            .collect::<Vec<_>>();
        entries.push(ConnectorCheckEntry {
            name: name.to_string(),
            valid: report.valid,
            diagnostics,
        });
    }
    if live && std::env::var("CORVID_PROVIDER_LIVE").as_deref() != Ok("1") {
        return Err(anyhow!(
            "`--live` requires `CORVID_PROVIDER_LIVE=1` plus per-provider \
             credentials — refusing to issue live drift probes without \
             explicit opt-in. The default `corvid connectors check` \
             validates manifests without any network call."
        ));
    }
    // Live drift narration is deferred to slice 41M (per the
    // ROADMAP audit-correction track). Surfacing this honestly
    // rather than silently no-op'ing.
    if live {
        return Err(anyhow!(
            "Live drift narration is implemented end-to-end in slice 41M \
             (per `docs/effects-spec/bounty.md`). This slice ships \
             manifest-only validation; rerun without `--live` for the \
             validation report."
        ));
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Slice 41L: `corvid connectors check` flags every shipped
    /// manifest as valid (manifests are static and CI-tested
    /// elsewhere). With `--live` the command refuses without
    /// `CORVID_PROVIDER_LIVE=1`.
    #[test]
    fn check_passes_for_shipped_manifests() {
        let entries = run_check(false).expect("check");
        for entry in &entries {
            assert!(entry.valid, "{}: {:?}", entry.name, entry.diagnostics);
        }
    }
}
