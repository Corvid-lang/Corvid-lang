//! Lockfile graph validation and package-policy conflict diagnostics.
//!
//! This module validates the installed package graph described by
//! `corvid.toml` and `Corvid.lock`. It does not resolve new versions; it
//! checks whether the already-locked graph is internally coherent and still
//! satisfies the current project package policy.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{anyhow, Result};
use semver::Version;
use serde::Serialize;

use crate::package_lock::{load_or_empty_at, lock_path_for_project, LockedPackage};
use crate::package_manifest::dependencies;
use crate::package_policy::{load_package_policy, package_policy_violation};
use crate::package_version::{normalize_version, parse_version_requirement, validate_package_name};

#[derive(Debug, Clone, Serialize)]
pub struct PackageConflictReport {
    pub project: String,
    pub manifest_dependencies: usize,
    pub locked_packages: usize,
    pub failures: Vec<PackageConflictFailure>,
}

impl PackageConflictReport {
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PackageConflictFailure {
    pub package: String,
    pub kind: PackageConflictKind,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageConflictKind {
    DuplicateLockUri,
    DuplicateLockedVersion,
    EffectPolicyViolation,
    InvalidManifestDependency,
    MalformedLockUri,
    MissingLockEntry,
    MissingSemanticSummary,
    UndeclaredLockedPackage,
    UnsatisfiedVersionRequirement,
    VersionConflict,
}

#[derive(Debug, Clone)]
struct LockedPackageKey {
    name: String,
    version: Version,
}

pub fn verify_package_lock(project_dir: &Path) -> Result<PackageConflictReport> {
    let manifest_deps = dependencies(project_dir)?;
    let lockfile = lock_path_for_project(project_dir);
    let lock = load_or_empty_at(&lockfile).map_err(|message| anyhow!(message))?;
    let policy = load_package_policy(project_dir)?;
    let mut report = PackageConflictReport {
        project: project_dir.display().to_string(),
        manifest_dependencies: manifest_deps.len(),
        locked_packages: lock.package.len(),
        failures: Vec::new(),
    };

    let manifest_names = manifest_deps
        .iter()
        .map(|dep| dep.name.as_str())
        .collect::<BTreeSet<_>>();
    let mut entries_by_name = BTreeMap::<String, Vec<(&LockedPackage, LockedPackageKey)>>::new();
    let mut seen_uris = BTreeSet::<String>::new();

    for entry in &lock.package {
        if !seen_uris.insert(entry.uri.clone()) {
            report.failures.push(failure(
                package_label_from_uri(&entry.uri),
                PackageConflictKind::DuplicateLockUri,
                format!("Corvid.lock contains duplicate package URI `{}`", entry.uri),
            ));
        }
        let key = match parse_locked_package_uri(&entry.uri) {
            Ok(key) => key,
            Err(reason) => {
                report.failures.push(failure(
                    package_label_from_uri(&entry.uri),
                    PackageConflictKind::MalformedLockUri,
                    reason,
                ));
                continue;
            }
        };
        if !manifest_names.contains(key.name.as_str()) {
            report.failures.push(failure(
                key.name.clone(),
                PackageConflictKind::UndeclaredLockedPackage,
                format!(
                    "Corvid.lock contains `{}` but corvid.toml [dependencies] does not declare it",
                    entry.uri
                ),
            ));
        }
        entries_by_name
            .entry(key.name.clone())
            .or_default()
            .push((entry, key));
    }

    for dep in manifest_deps {
        if let Err(err) = validate_package_name(&dep.name) {
            report.failures.push(failure(
                dep.name.clone(),
                PackageConflictKind::InvalidManifestDependency,
                err.to_string(),
            ));
            continue;
        }
        let requirement = match parse_version_requirement(&dep.version) {
            Ok(requirement) => requirement,
            Err(err) => {
                report.failures.push(failure(
                    dep.name.clone(),
                    PackageConflictKind::InvalidManifestDependency,
                    err.to_string(),
                ));
                continue;
            }
        };
        let Some(entries) = entries_by_name.get(&dep.name) else {
            report.failures.push(failure(
                dep.name.clone(),
                PackageConflictKind::MissingLockEntry,
                format!(
                    "corvid.toml declares `{}` = `{}` but Corvid.lock has no matching package",
                    dep.name, dep.version
                ),
            ));
            continue;
        };

        let versions = entries
            .iter()
            .map(|(_, key)| key.version.clone())
            .collect::<BTreeSet<_>>();
        if versions.len() > 1 {
            report.failures.push(failure(
                dep.name.clone(),
                PackageConflictKind::VersionConflict,
                format!(
                    "Corvid.lock contains multiple versions for `{}`: {}",
                    dep.name,
                    versions
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }

        let mut seen_versions = BTreeSet::<Version>::new();
        for (entry, key) in entries {
            if !seen_versions.insert(key.version.clone()) {
                report.failures.push(failure(
                    dep.name.clone(),
                    PackageConflictKind::DuplicateLockedVersion,
                    format!(
                        "Corvid.lock repeats `{}` version `{}` through multiple URIs",
                        dep.name, key.version
                    ),
                ));
            }
            if !requirement.matches(&key.version) {
                report.failures.push(failure(
                    dep.name.clone(),
                    PackageConflictKind::UnsatisfiedVersionRequirement,
                    format!(
                        "locked `{}` does not satisfy manifest requirement `{}`",
                        key.version, dep.version
                    ),
                ));
            }
            let Some(summary) = &entry.semantic_summary else {
                report.failures.push(failure(
                    dep.name.clone(),
                    PackageConflictKind::MissingSemanticSummary,
                    format!(
                        "locked `{}` has no semantic_summary, so effect-policy compatibility cannot be verified",
                        entry.uri
                    ),
                ));
                continue;
            };
            if let Some(reason) =
                package_policy_violation(summary, &policy, entry.signature.as_deref())
            {
                report.failures.push(failure(
                    dep.name.clone(),
                    PackageConflictKind::EffectPolicyViolation,
                    reason,
                ));
            }
        }
    }

    Ok(report)
}

pub fn render_package_conflict_report(report: &PackageConflictReport) -> String {
    let mut out = String::new();
    out.push_str("Package lock verification\n\n");
    out.push_str(&format!("- project: `{}`\n", report.project));
    out.push_str(&format!(
        "- manifest dependencies: `{}`\n",
        report.manifest_dependencies
    ));
    out.push_str(&format!("- locked packages: `{}`\n", report.locked_packages));
    if report.failures.is_empty() {
        out.push_str("- status: `clean`\n");
        return out;
    }
    out.push_str("- status: `conflicts found`\n\n");
    out.push_str("| Package | Kind | Reason |\n");
    out.push_str("|---|---|---|\n");
    for failure in &report.failures {
        out.push_str(&format!(
            "| `{}` | `{:?}` | {} |\n",
            failure.package, failure.kind, failure.reason
        ));
    }
    out
}

fn parse_locked_package_uri(uri: &str) -> Result<LockedPackageKey, String> {
    let Some(rest) = uri.strip_prefix("corvid://") else {
        return Err(format!("package URI `{uri}` must start with `corvid://`"));
    };
    let Some((name, version)) = rest.rsplit_once("/v") else {
        return Err(format!(
            "package URI `{uri}` must end with `/v<semver>`, e.g. `corvid://@scope/name/v1.2.3`"
        ));
    };
    validate_package_name(name).map_err(|err| err.to_string())?;
    let version = Version::parse(&normalize_version(version))
        .map_err(|err| format!("package URI `{uri}` has invalid semantic version: {err}"))?;
    Ok(LockedPackageKey {
        name: name.to_string(),
        version,
    })
}

fn failure(
    package: String,
    kind: PackageConflictKind,
    reason: impl Into<String>,
) -> PackageConflictFailure {
    PackageConflictFailure {
        package,
        kind,
        reason: reason.into(),
    }
}

fn package_label_from_uri(uri: &str) -> String {
    uri.strip_prefix("corvid://")
        .and_then(|rest| rest.rsplit_once("/v").map(|(name, _)| name.to_string()))
        .unwrap_or_else(|| uri.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package_lock::{write_package_lock, LockedPackage, PackageLock};
    use corvid_resolve::{AgentSemanticSummary, ModuleSemanticSummary};

    #[test]
    fn verify_lock_accepts_manifest_lock_policy_match() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("corvid.toml"),
            "[dependencies]\n\"@scope/name\" = \"^1.0.0\"\n",
        )
        .unwrap();
        write_package_lock(
            &tmp.path().join("Corvid.lock"),
            &PackageLock {
                package: vec![locked("@scope/name", "1.2.3", Some(clean_summary()), None)],
            },
        )
        .unwrap();

        let report = verify_package_lock(tmp.path()).unwrap();
        assert!(report.is_clean(), "{report:#?}");
    }

    #[test]
    fn verify_lock_reports_missing_lock_entry() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("corvid.toml"),
            "[dependencies]\n\"@scope/name\" = \"1\"\n",
        )
        .unwrap();

        let report = verify_package_lock(tmp.path()).unwrap();
        assert!(report.failures.iter().any(|failure| matches!(
            failure.kind,
            PackageConflictKind::MissingLockEntry
        )));
    }

    #[test]
    fn verify_lock_reports_multiple_locked_versions() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("corvid.toml"),
            "[dependencies]\n\"@scope/name\" = \"^1.0.0\"\n",
        )
        .unwrap();
        write_package_lock(
            &tmp.path().join("Corvid.lock"),
            &PackageLock {
                package: vec![
                    locked("@scope/name", "1.0.0", Some(clean_summary()), None),
                    locked("@scope/name", "1.1.0", Some(clean_summary()), None),
                ],
            },
        )
        .unwrap();

        let report = verify_package_lock(tmp.path()).unwrap();
        assert!(report
            .failures
            .iter()
            .any(|failure| matches!(failure.kind, PackageConflictKind::VersionConflict)));
    }

    #[test]
    fn verify_lock_rechecks_effect_policy_from_semantic_summary() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("corvid.toml"),
            "[dependencies]\n\"@scope/name\" = \"1\"\n\n[package-policy]\nrequire-replayable = true\n",
        )
        .unwrap();
        write_package_lock(
            &tmp.path().join("Corvid.lock"),
            &PackageLock {
                package: vec![locked("@scope/name", "1.0.0", Some(non_replayable_summary()), None)],
            },
        )
        .unwrap();

        let report = verify_package_lock(tmp.path()).unwrap();
        assert!(report.failures.iter().any(|failure| matches!(
            failure.kind,
            PackageConflictKind::EffectPolicyViolation
        )));
    }

    fn locked(
        name: &str,
        version: &str,
        semantic_summary: Option<ModuleSemanticSummary>,
        signature: Option<&str>,
    ) -> LockedPackage {
        LockedPackage {
            uri: format!("corvid://{name}/v{version}"),
            url: format!("https://registry.example/{}/{}.cor", name.trim_start_matches('@'), version),
            sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_string(),
            registry: Some("https://registry.example/index.toml".to_string()),
            signature: signature.map(str::to_string),
            semantic_summary,
        }
    }

    fn clean_summary() -> ModuleSemanticSummary {
        ModuleSemanticSummary::default()
    }

    fn non_replayable_summary() -> ModuleSemanticSummary {
        let mut summary = ModuleSemanticSummary::default();
        summary.agents.insert(
            "helper".to_string(),
            AgentSemanticSummary {
                name: "helper".to_string(),
                deterministic: false,
                replayable: false,
                composed_dimensions: Default::default(),
                violations: Vec::new(),
                cost: None,
                approval_required: false,
                grounded_return: false,
            },
        );
        summary
    }
}
