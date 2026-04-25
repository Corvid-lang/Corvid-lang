//! `corvid.toml` dependency editing for the package manager.
//!
//! The compiler already tolerates unknown `corvid.toml` sections. This module
//! owns the package-manager side of that file: preserving existing TOML while
//! updating `[dependencies]` entries in a deterministic shape.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use toml::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestDependency {
    pub name: String,
    pub version: String,
    pub registry: Option<String>,
}

pub(crate) fn manifest_path_for_project(project_dir: &Path) -> PathBuf {
    project_dir.join("corvid.toml")
}

pub(crate) fn upsert_dependency(
    project_dir: &Path,
    name: &str,
    version: &str,
    registry: Option<&str>,
) -> Result<PathBuf> {
    let path = manifest_path_for_project(project_dir);
    let mut root = load_manifest_value(&path)?;
    let table = root
        .as_table_mut()
        .ok_or_else(|| anyhow!("`{}` root must be a TOML table", path.display()))?;
    let deps = table
        .entry("dependencies".to_string())
        .or_insert_with(|| Value::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| anyhow!("`{}` [dependencies] must be a TOML table", path.display()))?;
    let value = match registry {
        Some(registry) => {
            let mut dep = toml::map::Map::new();
            dep.insert("version".to_string(), Value::String(version.to_string()));
            dep.insert("registry".to_string(), Value::String(registry.to_string()));
            Value::Table(dep)
        }
        None => Value::String(version.to_string()),
    };
    deps.insert(name.to_string(), value);
    write_manifest_value(&path, &root)?;
    Ok(path)
}

pub(crate) fn remove_dependency(project_dir: &Path, name: &str) -> Result<Option<PathBuf>> {
    let path = manifest_path_for_project(project_dir);
    if !path.exists() {
        return Ok(None);
    }
    let mut root = load_manifest_value(&path)?;
    let Some(table) = root.as_table_mut() else {
        return Err(anyhow!("`{}` root must be a TOML table", path.display()));
    };
    let removed = table
        .get_mut("dependencies")
        .and_then(Value::as_table_mut)
        .and_then(|deps| deps.remove(name))
        .is_some();
    if removed {
        write_manifest_value(&path, &root)?;
        Ok(Some(path))
    } else {
        Ok(None)
    }
}

pub(crate) fn dependency(project_dir: &Path, name: &str) -> Result<Option<ManifestDependency>> {
    let path = manifest_path_for_project(project_dir);
    if !path.exists() {
        return Ok(None);
    }
    let root = load_manifest_value(&path)?;
    let Some(deps) = root.get("dependencies").and_then(Value::as_table) else {
        return Ok(None);
    };
    let Some(value) = deps.get(name) else {
        return Ok(None);
    };
    parse_dependency(name, value).map(Some)
}

fn parse_dependency(name: &str, value: &Value) -> Result<ManifestDependency> {
    match value {
        Value::String(version) => Ok(ManifestDependency {
            name: name.to_string(),
            version: version.clone(),
            registry: None,
        }),
        Value::Table(table) => {
            let version = table
                .get("version")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("dependency `{name}` table requires `version`"))?;
            let registry = table
                .get("registry")
                .and_then(Value::as_str)
                .map(str::to_string);
            Ok(ManifestDependency {
                name: name.to_string(),
                version: version.to_string(),
                registry,
            })
        }
        _ => Err(anyhow!(
            "dependency `{name}` must be a version string or a table with `version`"
        )),
    }
}

fn load_manifest_value(path: &Path) -> Result<Value> {
    match std::fs::read_to_string(path) {
        Ok(source) => toml::from_str::<Value>(&source)
            .with_context(|| format!("failed to parse `{}`", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Ok(Value::Table(Default::default()))
        }
        Err(err) => Err(anyhow!("failed to read `{}`: {err}", path.display())),
    }
}

fn write_manifest_value(path: &Path, value: &Value) -> Result<()> {
    let source = toml::to_string_pretty(value)
        .with_context(|| format!("failed to serialize `{}`", path.display()))?;
    std::fs::write(path, source).with_context(|| format!("failed to write `{}`", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_dependency_preserves_other_sections() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("corvid.toml"),
            "[llm]\ndefault_model = \"haiku\"\n",
        )
        .unwrap();
        upsert_dependency(
            tmp.path(),
            "@scope/name",
            "^1.2.0",
            Some("./registry/index.toml"),
        )
        .unwrap();

        let source = std::fs::read_to_string(tmp.path().join("corvid.toml")).unwrap();
        assert!(source.contains("[llm]"), "{source}");
        assert!(source.contains("[dependencies.\"@scope/name\"]"), "{source}");
        assert!(source.contains("version = \"^1.2.0\""), "{source}");
        assert!(source.contains("registry = \"./registry/index.toml\""), "{source}");
    }

    #[test]
    fn dependency_reads_string_and_table_shapes() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("corvid.toml"),
            "[dependencies]\n\"@a/one\" = \"1\"\n\n[dependencies.\"@b/two\"]\nversion = \"^2.0.0\"\nregistry = \"./registry\"\n",
        )
        .unwrap();
        let one = dependency(tmp.path(), "@a/one").unwrap().unwrap();
        let two = dependency(tmp.path(), "@b/two").unwrap().unwrap();
        assert_eq!(one.version, "1");
        assert_eq!(one.registry, None);
        assert_eq!(two.version, "^2.0.0");
        assert_eq!(two.registry.as_deref(), Some("./registry"));
    }

    #[test]
    fn remove_dependency_only_edits_dependencies_table() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("corvid.toml"),
            "[dependencies]\n\"@scope/name\" = \"1\"\n\n[package-policy]\nrequire-replayable = true\n",
        )
        .unwrap();
        assert!(remove_dependency(tmp.path(), "@scope/name").unwrap().is_some());
        let source = std::fs::read_to_string(tmp.path().join("corvid.toml")).unwrap();
        assert!(!source.contains("@scope/name"), "{source}");
        assert!(source.contains("[package-policy]"), "{source}");
    }
}
