//! `corvid package` / `corvid add` / `corvid remove` / `corvid
//! update` CLI dispatch — slice 33 / package-manager surface,
//! decomposed in Phase 20j-A1.
//!
//! Owns every dispatch arm under the registry-publish lifecycle:
//!
//! - [`cmd_add_package`] / [`cmd_remove_package`] /
//!   [`cmd_update_package`] mutate `corvid.toml` + `Corvid.lock`
//!   per the registry's policy.
//! - [`cmd_package_publish`] signs and writes a source package
//!   into a registry directory.
//! - [`cmd_package_metadata`] renders the public per-package
//!   metadata page consumed by registry sites.
//! - [`cmd_package_verify_registry`] verifies a registry index
//!   and every referenced source artifact.
//! - [`cmd_package_verify_lock`] verifies `corvid.toml` and
//!   `Corvid.lock` agree with package policy.
//!
//! Every operation goes through `corvid_driver`'s package-manager
//! API; this module owns only the CLI shape + JSON / text output.

use anyhow::Result;
use std::path::{Path, PathBuf};

pub(crate) fn cmd_add_package(spec: &str, registry: Option<&str>) -> Result<u8> {
    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    println!("corvid add {spec}\n");
    let outcome = corvid_driver::add_package(spec, &project_dir, registry)?;
    match outcome {
        corvid_driver::AddPackageOutcome::Added {
            uri,
            version,
            lockfile,
            exports,
        } => {
            println!(
                "added `{uri}` ({version}) to {} with {exports} exported contract item{}",
                lockfile.display(),
                if exports == 1 { "" } else { "s" }
            );
            Ok(0)
        }
        corvid_driver::AddPackageOutcome::Rejected { reason } => {
            eprintln!("package rejected: {reason}");
            Ok(1)
        }
    }
}

pub(crate) fn cmd_remove_package(name: &str) -> Result<u8> {
    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    println!("corvid remove {name}\n");
    let outcome = corvid_driver::remove_package(name, &project_dir)?;
    match outcome {
        corvid_driver::PackageMutationOutcome::Removed {
            name,
            manifest_updated,
            lock_entries_removed,
            lockfile,
        } => {
            println!(
                "removed `{name}` (manifest: {}, lock entries: {})",
                if manifest_updated {
                    "updated"
                } else {
                    "unchanged"
                },
                lock_entries_removed
            );
            println!("lockfile: {}", lockfile.display());
            Ok(0)
        }
        corvid_driver::PackageMutationOutcome::Rejected { reason } => {
            eprintln!("package rejected: {reason}");
            Ok(1)
        }
        corvid_driver::PackageMutationOutcome::Updated { .. } => unreachable!(),
    }
}

pub(crate) fn cmd_update_package(spec: &str, registry: Option<&str>) -> Result<u8> {
    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    println!("corvid update {spec}\n");
    let outcome = corvid_driver::update_package(spec, &project_dir, registry)?;
    match outcome {
        corvid_driver::PackageMutationOutcome::Updated {
            uri,
            version,
            lockfile,
            exports,
        } => {
            println!(
                "updated `{uri}` ({version}) in {} with {exports} exported contract item{}",
                lockfile.display(),
                if exports == 1 { "" } else { "s" }
            );
            Ok(0)
        }
        corvid_driver::PackageMutationOutcome::Rejected { reason } => {
            eprintln!("package rejected: {reason}");
            Ok(1)
        }
        corvid_driver::PackageMutationOutcome::Removed { .. } => unreachable!(),
    }
}

pub(crate) fn cmd_package_publish(
    source: &Path,
    name: &str,
    version: &str,
    out: &Path,
    url_base: &str,
    key: &str,
    key_id: &str,
) -> Result<u8> {
    let outcome = corvid_driver::publish_package(corvid_driver::PublishPackageOptions {
        source,
        name,
        version,
        out_dir: out,
        url_base,
        signing_seed_hex: key,
        key_id,
    })?;
    println!(
        "published `{}` to {}\nartifact: {}\nsha256: {}",
        outcome.uri,
        outcome.index.display(),
        outcome.artifact.display(),
        outcome.sha256
    );
    Ok(0)
}

pub(crate) fn cmd_package_metadata(
    source: &Path,
    name: &str,
    version: &str,
    signature: Option<&str>,
    json: bool,
) -> Result<u8> {
    let metadata = corvid_driver::package_metadata_from_source(source, name, version, signature)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&metadata)?);
    } else {
        print!(
            "{}",
            corvid_driver::render_package_metadata_markdown(&metadata)
        );
    }
    Ok(0)
}

pub(crate) fn cmd_package_verify_registry(registry: &str, json: bool) -> Result<u8> {
    let report = corvid_driver::verify_registry_contract(registry)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("corvid package verify-registry {registry}\n");
        println!("checked package entries: {}", report.checked);
        if report.failures.is_empty() {
            println!("registry contract: ok");
        } else {
            println!("registry contract: failed");
            for failure in &report.failures {
                println!(
                    "- {}@{}: {}",
                    failure.package, failure.version, failure.reason
                );
            }
        }
    }
    Ok(if report.is_clean() { 0 } else { 1 })
}

pub(crate) fn cmd_package_verify_lock(json: bool) -> Result<u8> {
    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let report = corvid_driver::verify_package_lock(&project_dir)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", corvid_driver::render_package_conflict_report(&report));
    }
    Ok(if report.is_clean() { 0 } else { 1 })
}
