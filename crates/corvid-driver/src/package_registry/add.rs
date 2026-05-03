//! Package add/install resolution.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use semver::Version;

use crate::import_integrity::sha256_hex;
use crate::modules::summarize_module_source;
use crate::package_lock::{
    load_or_empty_at, lock_path_for_project, upsert_package, write_package_lock, LockedPackage,
};
use crate::package_manifest::upsert_dependency;
use crate::package_policy::{load_package_policy, package_policy_violation};
use crate::package_version::{normalize_version, parse_package_spec, PackageSpec};

use super::{
    fetch_bytes, load_registry_index, verify_package_signature, AddPackageOutcome, RegistryIndex,
    RegistryPackage, DEFAULT_REGISTRY,
};

pub fn add_package(
    spec: &str,
    project_dir: &Path,
    registry: Option<&str>,
) -> Result<AddPackageOutcome> {
    let spec = parse_package_spec(spec)?;
    let registry_location = registry
        .map(str::to_string)
        .or_else(|| std::env::var("CORVID_PACKAGE_REGISTRY").ok())
        .unwrap_or_else(|| DEFAULT_REGISTRY.to_string());
    let index = load_registry_index(&registry_location)?;
    let Some(selected) = select_package(&index, &spec)? else {
        return Ok(AddPackageOutcome::Rejected {
            reason: format!(
                "registry `{registry_location}` has no package `{}` matching the requested version",
                spec.name
            ),
        });
    };

    let bytes = fetch_bytes(&selected.url)
        .with_context(|| format!("failed to fetch package source `{}`", selected.url))?;
    let actual = sha256_hex(&bytes);
    if !actual.eq_ignore_ascii_case(&selected.sha256) {
        return Ok(AddPackageOutcome::Rejected {
            reason: format!(
                "package `{}`@{} failed registry hash verification: expected sha256:{}, actual sha256:{actual}",
                selected.name, selected.version, selected.sha256
            ),
        });
    }
    let source = String::from_utf8(bytes).with_context(|| {
        format!(
            "package `{}`@{} is not valid UTF-8",
            selected.name, selected.version
        )
    })?;
    let summary = summarize_module_source(&source).map_err(|message| {
        anyhow!(
            "package `{}`@{} failed semantic summary build: {message}",
            selected.name,
            selected.version
        )
    })?;
    let policy = load_package_policy(project_dir)?;
    if let Some(reason) = package_policy_violation(&summary, &policy, selected.signature.as_deref())
    {
        return Ok(AddPackageOutcome::Rejected { reason });
    }
    if let Some(signature) = &selected.signature {
        if let Err(message) = verify_package_signature(selected, &summary, signature) {
            return Ok(AddPackageOutcome::Rejected { reason: message });
        }
    }

    let lockfile = lock_path_for_project(project_dir);
    let mut lock = load_or_empty_at(&lockfile).map_err(|message| anyhow!(message))?;
    let uri = selected
        .uri
        .clone()
        .unwrap_or_else(|| format!("corvid://{}/v{}", selected.name, selected.version));
    upsert_package(
        &mut lock,
        LockedPackage {
            uri: uri.clone(),
            url: selected.url.clone(),
            sha256: selected.sha256.to_ascii_lowercase(),
            registry: selected
                .registry
                .clone()
                .or_else(|| Some(registry_location.clone())),
            signature: selected.signature.clone(),
            semantic_summary: Some(summary.clone()),
        },
    );
    write_package_lock(&lockfile, &lock).map_err(|message| anyhow!(message))?;
    upsert_dependency(
        project_dir,
        &spec.name,
        &spec.raw_requirement,
        Some(registry_location.as_str()),
    )?;

    Ok(AddPackageOutcome::Added {
        uri,
        version: selected.version.clone(),
        lockfile,
        exports: summary.exports.len(),
    })
}

pub(super) fn select_package<'a>(
    index: &'a RegistryIndex,
    spec: &PackageSpec,
) -> Result<Option<&'a RegistryPackage>> {
    let mut candidates = Vec::new();
    for package in &index.package {
        if package.name != spec.name {
            continue;
        }
        let version = Version::parse(&normalize_version(&package.version)).with_context(|| {
            format!(
                "registry package `{}` has invalid version `{}`",
                package.name, package.version
            )
        })?;
        if spec.requirement.matches(&version) {
            candidates.push((version, package));
        }
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(candidates.pop().map(|(_, package)| package))
}
