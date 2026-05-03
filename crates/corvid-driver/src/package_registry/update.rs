//! Package update resolution.

use std::path::Path;

use anyhow::Result;

use crate::package_manifest::dependency;
use crate::package_version::{parse_package_spec, validate_package_name};

use super::{add_package, AddPackageOutcome, PackageMutationOutcome};

pub fn update_package(
    spec: &str,
    project_dir: &Path,
    registry: Option<&str>,
) -> Result<PackageMutationOutcome> {
    if parse_package_spec(spec).is_ok() {
        return match add_package(spec, project_dir, registry)? {
            AddPackageOutcome::Added {
                uri,
                version,
                lockfile,
                exports,
            } => Ok(PackageMutationOutcome::Updated {
                uri,
                version,
                lockfile,
                exports,
            }),
            AddPackageOutcome::Rejected { reason } => {
                Ok(PackageMutationOutcome::Rejected { reason })
            }
        };
    }
    validate_package_name(spec)?;
    let Some(dep) = dependency(project_dir, spec)? else {
        return Ok(PackageMutationOutcome::Rejected {
            reason: format!("package `{spec}` is not declared in corvid.toml [dependencies]"),
        });
    };
    let registry = registry.or(dep.registry.as_deref());
    let add_spec = format!("{}@{}", dep.name, dep.version);
    match add_package(&add_spec, project_dir, registry)? {
        AddPackageOutcome::Added {
            uri,
            version,
            lockfile,
            exports,
        } => Ok(PackageMutationOutcome::Updated {
            uri,
            version,
            lockfile,
            exports,
        }),
        AddPackageOutcome::Rejected { reason } => Ok(PackageMutationOutcome::Rejected { reason }),
    }
}
