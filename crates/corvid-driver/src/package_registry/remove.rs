//! Package removal from manifest and lockfile.

use std::path::Path;

use anyhow::{anyhow, Result};

use crate::package_lock::{
    load_or_empty_at, lock_path_for_project, remove_packages_by_name, write_package_lock,
};
use crate::package_manifest::remove_dependency;
use crate::package_version::validate_package_name;

use super::PackageMutationOutcome;

pub fn remove_package(spec: &str, project_dir: &Path) -> Result<PackageMutationOutcome> {
    validate_package_name(spec)?;
    let manifest_updated = remove_dependency(project_dir, spec)?.is_some();
    let lockfile = lock_path_for_project(project_dir);
    let mut lock = load_or_empty_at(&lockfile).map_err(|message| anyhow!(message))?;
    let lock_entries_removed = remove_packages_by_name(&mut lock, spec);
    if !manifest_updated && lock_entries_removed == 0 {
        return Ok(PackageMutationOutcome::Rejected {
            reason: format!("package `{spec}` is not present in corvid.toml or Corvid.lock"),
        });
    }
    write_package_lock(&lockfile, &lock).map_err(|message| anyhow!(message))?;
    Ok(PackageMutationOutcome::Removed {
        name: spec.to_string(),
        manifest_updated,
        lock_entries_removed,
        lockfile,
    })
}
