//! `corvid connectors list` — read-only catalog of every shipped
//! connector. Each entry is the parsed manifest summarised into a
//! one-line table row (modes, scope count, write-scope ids,
//! rate-limit summary, redaction count). Pure manifest projection;
//! no network and no provider call.

use anyhow::Result;

use super::support::{shipped_manifests, summarise_manifest};

#[derive(Debug, Clone, PartialEq)]
pub struct ConnectorListEntry {
    pub name: String,
    pub provider: String,
    pub modes: Vec<String>,
    pub scope_count: usize,
    pub write_scopes: Vec<String>,
    pub rate_limit_summary: String,
    pub redaction_count: usize,
}

/// Returns the catalog of connectors built into Corvid. Each entry
/// is the parsed manifest summarised into a one-line table row.
pub fn run_list() -> Result<Vec<ConnectorListEntry>> {
    let manifests = shipped_manifests()?;
    let mut entries = Vec::with_capacity(manifests.len());
    for (_, manifest) in manifests {
        entries.push(summarise_manifest(&manifest));
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Slice 41L: `corvid connectors list` returns one entry per
    /// shipped manifest with its mode set, write-scope list, and
    /// rate-limit summary.
    #[test]
    fn list_returns_every_shipped_connector() {
        let entries = run_list().expect("list");
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        // Manifest names are the public source of truth; ms365's
        // manifest spells itself `microsoft365` while the
        // shipped_manifests dictionary keys it as `ms365`.
        for required in ["gmail", "slack", "calendar", "files"] {
            assert!(names.contains(&required), "missing {required}: {names:?}");
        }
        // Tasks manifest uses "linear_github_tasks" as its name; we
        // assert at least one tasks-shaped entry exists.
        assert!(
            names.iter().any(|n| n.contains("task") || n.contains("linear")
                || n.contains("github")),
            "missing tasks-shaped connector: {names:?}",
        );
        // Microsoft 365 manifest names itself "microsoft365".
        assert!(
            names.iter().any(|n| n.contains("365")),
            "missing ms365-shaped connector: {names:?}",
        );
        let gmail = entries.iter().find(|e| e.name == "gmail").unwrap();
        assert!(gmail.modes.contains(&"real".to_string()));
        assert!(gmail.scope_count > 0);
        assert!(gmail.write_scopes.iter().any(|s| s.contains("send")));
    }
}
