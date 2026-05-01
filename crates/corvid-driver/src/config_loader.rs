//! `corvid.toml` discovery and loading.
//!
//! The driver consults a project-local `corvid.toml` for user-defined
//! effect dimensions during typecheck. The lookup walks upward from
//! the source file's parent until a `corvid.toml` is found, mirroring
//! how `cargo` finds `Cargo.toml`. A malformed file does not abort
//! compilation — `typecheck_with_config` surfaces the parse failure
//! as an `InvalidCustomDimension` diagnostic at the source file's
//! top span, so the user sees a regular compile error instead of a
//! tooling crash.

use std::path::{Path, PathBuf};

use corvid_types::CorvidConfig;

/// Walk upward from `source_path.parent()` looking for `corvid.toml`.
/// Returns `None` when no file is found or when parsing fails — a
/// malformed file doesn't crash the compile; instead it surfaces
/// through `typecheck_with_config` as an `InvalidCustomDimension`
/// diagnostic at the source file's top span.
pub fn load_corvid_config_for(source_path: &Path) -> Option<CorvidConfig> {
    load_corvid_config_with_path_for(source_path).map(|(_, config)| config)
}

/// Walk upward from `source_path.parent()` looking for `corvid.toml`,
/// returning both the config path and the parsed config. Use this when
/// tooling must resolve config-relative paths such as dimension proofs.
pub fn load_corvid_config_with_path_for(source_path: &Path) -> Option<(PathBuf, CorvidConfig)> {
    let start = source_path.parent()?;
    let mut cur: Option<&Path> = Some(start);
    while let Some(dir) = cur {
        let candidate = dir.join("corvid.toml");
        if candidate.exists() {
            return CorvidConfig::load_from_path(&candidate)
                .ok()
                .flatten()
                .map(|config| (candidate, config));
        }
        cur = dir.parent();
    }
    None
}
