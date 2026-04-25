//! `Corvid.lock` support for package imports.
//!
//! Package imports are intentionally fail-closed. Source code names a
//! semantic package URI (`corvid://@scope/name/v1.2`), while the lockfile
//! supplies the immutable fetch URL and SHA-256 digest that the loader
//! verifies before parsing.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone)]
pub(crate) struct PackageLockFile {
    pub path: PathBuf,
    pub lock: PackageLock,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PackageLock {
    #[serde(default)]
    pub package: Vec<LockedPackage>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct LockedPackage {
    pub uri: String,
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub registry: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
}

impl PackageLock {
    pub fn find(&self, uri: &str) -> Option<&LockedPackage> {
        self.package.iter().find(|entry| entry.uri == uri)
    }
}

pub(crate) fn load_package_lock_for(root_path: &Path) -> Result<Option<PackageLockFile>, String> {
    let Some(path) = find_package_lock_path(root_path) else {
        return Ok(None);
    };
    let source = std::fs::read_to_string(&path)
        .map_err(|err| format!("failed to read `{}`: {err}", path.display()))?;
    let lock: PackageLock = toml::from_str(&source)
        .map_err(|err| format!("failed to parse `{}`: {err}", path.display()))?;
    for entry in &lock.package {
        validate_entry(entry, &path)?;
    }
    Ok(Some(PackageLockFile { path, lock }))
}

fn find_package_lock_path(root_path: &Path) -> Option<PathBuf> {
    let mut cursor = if root_path.is_dir() {
        root_path.to_path_buf()
    } else {
        root_path.parent()?.to_path_buf()
    };
    loop {
        let candidate = cursor.join("Corvid.lock");
        if candidate.exists() {
            return Some(candidate);
        }
        if !cursor.pop() {
            return None;
        }
    }
}

fn validate_entry(entry: &LockedPackage, path: &Path) -> Result<(), String> {
    if !entry.uri.starts_with("corvid://") {
        return Err(format!(
            "`{}` contains package entry `{}` that is not a corvid:// URI",
            path.display(),
            entry.uri
        ));
    }
    if !(entry.url.starts_with("https://") || entry.url.starts_with("http://")) {
        return Err(format!(
            "`{}` package `{}` must resolve to an http(s) source URL",
            path.display(),
            entry.uri
        ));
    }
    if entry.sha256.len() != 64 || !entry.sha256.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(format!(
            "`{}` package `{}` has invalid sha256 digest `{}`",
            path.display(),
            entry.uri,
            entry.sha256
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_lock_finds_exact_uri() {
        let lock = PackageLock {
            package: vec![LockedPackage {
                uri: "corvid://@scope/name/v1.2".to_string(),
                url: "https://example.com/name-v1.2.cor".to_string(),
                sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                    .to_string(),
                registry: Some("https://registry.corvid.dev".to_string()),
                signature: Some("ed25519:abc".to_string()),
            }],
        };
        let entry = lock.find("corvid://@scope/name/v1.2").expect("entry");
        assert_eq!(entry.url, "https://example.com/name-v1.2.cor");
    }

    #[test]
    fn package_lock_rejects_short_digest() {
        let entry = LockedPackage {
            uri: "corvid://@scope/name/v1.2".to_string(),
            url: "https://example.com/name-v1.2.cor".to_string(),
            sha256: "abc".to_string(),
            registry: None,
            signature: None,
        };
        assert!(validate_entry(&entry, Path::new("Corvid.lock")).is_err());
    }
}
