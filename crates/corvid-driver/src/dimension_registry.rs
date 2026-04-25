//! Effect-dimension registry resolution for `corvid add-dimension`.
//!
//! The registry distributes signed dimension artifacts, not trusted compiler
//! state. Resolution therefore only fetches and hash-checks bytes; the normal
//! artifact verifier, law checker, proof replay, and regression corpus still
//! run before installation.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use semver::{Version, VersionReq};
use serde::Deserialize;

use crate::dimension_artifact::verify_dimension_artifact;
use crate::import_integrity::sha256_hex;

pub const DEFAULT_EFFECT_REGISTRY: &str = "https://effect.corvid-lang.org/index.toml";

#[derive(Debug)]
pub(crate) struct MaterializedDimensionArtifact {
    pub artifact_path: PathBuf,
    temp_dir: PathBuf,
}

impl Drop for MaterializedDimensionArtifact {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.temp_dir);
    }
}

#[derive(Debug, Deserialize)]
struct DimensionRegistryIndex {
    #[serde(default)]
    dimension: Vec<DimensionRegistryEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct DimensionRegistryEntry {
    name: String,
    version: String,
    url: String,
    sha256: String,
    #[serde(default)]
    proof_url: Option<String>,
    #[serde(default)]
    proof_sha256: Option<String>,
}

#[derive(Debug)]
struct DimensionSpec {
    name: String,
    requirement: VersionReq,
}

pub(crate) fn resolve_dimension_artifact(
    spec: &str,
    registry: Option<&str>,
) -> Result<MaterializedDimensionArtifact> {
    let spec = parse_dimension_spec(spec)?;
    let registry_location = registry
        .map(str::to_string)
        .or_else(|| std::env::var("CORVID_EFFECT_REGISTRY").ok())
        .unwrap_or_else(|| DEFAULT_EFFECT_REGISTRY.to_string());
    let index = load_registry_index(&registry_location)?;
    let Some(selected) = select_dimension(&index, &spec)? else {
        return Err(anyhow!(
            "effect registry `{registry_location}` has no dimension `{}` matching requested version",
            spec.name
        ));
    };
    validate_registry_entry_contract(selected)?;

    let artifact_location = resolve_registry_location(&registry_location, &selected.url)?;
    let bytes = fetch_bytes(&artifact_location)
        .with_context(|| format!("failed to fetch dimension artifact `{artifact_location}`"))?;
    verify_sha256(&selected.sha256, &bytes).with_context(|| {
        format!(
            "dimension `{}`@{} failed registry hash verification",
            selected.name, selected.version
        )
    })?;
    let artifact_source = String::from_utf8(bytes).with_context(|| {
        format!(
            "dimension artifact `{}`@{} is not UTF-8",
            selected.name, selected.version
        )
    })?;
    let report = verify_dimension_artifact(&artifact_source)?.ok_or_else(|| {
        anyhow!(
            "registry artifact `{}`@{} is missing required [artifact] signature table",
            selected.name,
            selected.version
        )
    })?;
    let selected_version = Version::parse(&normalize_version(&selected.version))?;
    if report.name != selected.name || report.version != selected_version {
        return Err(anyhow!(
            "registry entry declares `{}`@{} but artifact verifies as `{}`@{}",
            selected.name,
            selected.version,
            report.name,
            report.version
        ));
    }

    let proof = fetch_registry_proof(&registry_location, selected)?;
    materialize_artifact(&selected.name, &selected.version, &artifact_source, proof.as_ref())
}

fn parse_dimension_spec(spec: &str) -> Result<DimensionSpec> {
    let (name, version) = spec
        .split_once('@')
        .ok_or_else(|| anyhow!("dimension spec must be `name@version`"))?;
    if name.trim().is_empty() || version.trim().is_empty() {
        return Err(anyhow!("dimension spec must be `name@version`"));
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(anyhow!(
            "dimension name `{name}` may only contain ASCII letters, digits, `_`, and `-`"
        ));
    }
    Ok(DimensionSpec {
        name: name.to_string(),
        requirement: parse_version_req(version)?,
    })
}

fn parse_version_req(raw: &str) -> Result<VersionReq> {
    let trimmed = raw.trim();
    let has_operator = trimmed
        .chars()
        .next()
        .map(|ch| matches!(ch, '^' | '~' | '>' | '<' | '=' | '*'))
        .unwrap_or(false);
    if has_operator {
        VersionReq::parse(trimmed).with_context(|| format!("invalid version requirement `{raw}`"))
    } else {
        let normalized = normalize_version(trimmed);
        VersionReq::parse(&format!("={normalized}"))
            .with_context(|| format!("invalid version `{raw}`"))
    }
}

fn normalize_version(raw: &str) -> String {
    let mut parts: Vec<&str> = raw.split('.').collect();
    while parts.len() < 3 {
        parts.push("0");
    }
    parts.join(".")
}

fn load_registry_index(location: &str) -> Result<DimensionRegistryIndex> {
    let source = if location.starts_with("http://") || location.starts_with("https://") {
        let bytes = fetch_bytes(location)
            .with_context(|| format!("failed to fetch effect registry index `{location}`"))?;
        String::from_utf8(bytes)
            .with_context(|| format!("effect registry index `{location}` is not UTF-8"))?
    } else {
        let path = Path::new(location);
        let path = if path.is_dir() {
            path.join("index.toml")
        } else {
            path.to_path_buf()
        };
        std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read effect registry index `{}`", path.display()))?
    };
    toml::from_str(&source).context("failed to parse effect registry index")
}

fn select_dimension<'a>(
    index: &'a DimensionRegistryIndex,
    spec: &DimensionSpec,
) -> Result<Option<&'a DimensionRegistryEntry>> {
    let mut candidates = Vec::new();
    for entry in &index.dimension {
        if entry.name != spec.name {
            continue;
        }
        let version = Version::parse(&normalize_version(&entry.version)).with_context(|| {
            format!(
                "registry dimension `{}` has invalid version `{}`",
                entry.name, entry.version
            )
        })?;
        if spec.requirement.matches(&version) {
            candidates.push((version, entry));
        }
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(candidates.pop().map(|(_, entry)| entry))
}

fn validate_registry_entry_contract(entry: &DimensionRegistryEntry) -> Result<()> {
    Version::parse(&normalize_version(&entry.version))
        .with_context(|| format!("registry dimension `{}` has invalid semver", entry.name))?;
    if entry.sha256.len() != 64 || !entry.sha256.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(anyhow!(
            "registry dimension `{}`@{} has invalid sha256 digest",
            entry.name,
            entry.version
        ));
    }
    if !(entry.url.ends_with(".toml") || entry.url.ends_with(".dim.toml")) {
        return Err(anyhow!(
            "registry dimension `{}`@{} must point at a `.toml` dimension artifact",
            entry.name,
            entry.version
        ));
    }
    if entry.url.contains('?') || entry.url.contains('#') {
        return Err(anyhow!(
            "registry dimension `{}`@{} URL must be immutable: query strings and fragments are forbidden",
            entry.name,
            entry.version
        ));
    }
    match (&entry.proof_url, &entry.proof_sha256) {
        (Some(url), Some(hash)) => {
            if !(url.ends_with(".lean") || url.ends_with(".v")) {
                return Err(anyhow!(
                    "registry dimension `{}`@{} proof_url must end in `.lean` or `.v`",
                    entry.name,
                    entry.version
                ));
            }
            if hash.len() != 64 || !hash.chars().all(|ch| ch.is_ascii_hexdigit()) {
                return Err(anyhow!(
                    "registry dimension `{}`@{} has invalid proof_sha256 digest",
                    entry.name,
                    entry.version
                ));
            }
        }
        (None, None) => {}
        _ => {
            return Err(anyhow!(
                "registry dimension `{}`@{} must declare proof_url and proof_sha256 together",
                entry.name,
                entry.version
            ))
        }
    }
    Ok(())
}

fn fetch_registry_proof(
    registry_location: &str,
    entry: &DimensionRegistryEntry,
) -> Result<Option<Vec<u8>>> {
    let Some(proof_url) = &entry.proof_url else {
        return Ok(None);
    };
    let proof_location = resolve_registry_location(registry_location, proof_url)?;
    let bytes = fetch_bytes(&proof_location)
        .with_context(|| format!("failed to fetch dimension proof `{proof_location}`"))?;
    verify_sha256(entry.proof_sha256.as_deref().unwrap(), &bytes).with_context(|| {
        format!(
            "dimension `{}`@{} failed proof hash verification",
            entry.name, entry.version
        )
    })?;
    Ok(Some(bytes))
}

fn resolve_registry_location(registry_location: &str, artifact: &str) -> Result<String> {
    if artifact.starts_with("http://") || artifact.starts_with("https://") {
        return Ok(artifact.to_string());
    }
    if registry_location.starts_with("http://") || registry_location.starts_with("https://") {
        let base = url::Url::parse(registry_location)
            .with_context(|| format!("invalid effect registry URL `{registry_location}`"))?;
        return Ok(base
            .join(artifact)
            .with_context(|| format!("invalid relative registry artifact URL `{artifact}`"))?
            .to_string());
    }
    let registry_path = Path::new(registry_location);
    let base = if registry_path.is_dir() {
        registry_path.to_path_buf()
    } else {
        registry_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    };
    Ok(base.join(artifact).to_string_lossy().to_string())
}

fn fetch_bytes(location: &str) -> Result<Vec<u8>> {
    if location.starts_with("http://") || location.starts_with("https://") {
        let response = ureq::get(location).call().map_err(|err| anyhow!(err.to_string()))?;
        if !(200..=299).contains(&response.status()) {
            return Err(anyhow!("HTTP status {}", response.status()));
        }
        let mut bytes = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .context("failed reading HTTP response")?;
        Ok(bytes)
    } else {
        std::fs::read(location).with_context(|| format!("failed to read `{location}`"))
    }
}

fn verify_sha256(expected: &str, bytes: &[u8]) -> Result<()> {
    let actual = sha256_hex(bytes);
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(anyhow!("expected sha256:{expected}, actual sha256:{actual}"))
    }
}

fn materialize_artifact(
    name: &str,
    version: &str,
    artifact_source: &str,
    proof: Option<&Vec<u8>>,
) -> Result<MaterializedDimensionArtifact> {
    let root = std::env::temp_dir().join(format!(
        "corvid-dimension-{}-{}-{}-{}",
        sanitize(name),
        sanitize(version),
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root)
        .with_context(|| format!("create temporary dimension artifact dir `{}`", root.display()))?;
    let artifact_path = root.join(format!("{}-{}.dim.toml", sanitize(name), sanitize(version)));
    std::fs::write(&artifact_path, artifact_source).with_context(|| {
        format!(
            "write temporary dimension artifact `{}`",
            artifact_path.display()
        )
    })?;

    if let Some(proof_bytes) = proof {
        let config: corvid_types::CorvidConfig = toml::from_str(artifact_source)
            .context("parse dimension artifact while materializing proof")?;
        let schemas = config
            .into_dimension_schemas()
            .map_err(|err| anyhow!("dimension artifact declaration is invalid: {err}"))?;
        let Some((_, meta)) = schemas.first() else {
            return Err(anyhow!("dimension artifact has no declaration"));
        };
        let proof_path = meta
            .proof_path
            .as_deref()
            .ok_or_else(|| anyhow!("registry supplied proof bytes but artifact declares no proof"))?;
        let proof_path = root.join(proof_path);
        if let Some(parent) = proof_path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("create temporary proof dir `{}`", parent.display())
            })?;
        }
        std::fs::write(&proof_path, proof_bytes)
            .with_context(|| format!("write temporary proof `{}`", proof_path.display()))?;
    }

    Ok(MaterializedDimensionArtifact {
        artifact_path,
        temp_dir: root,
    })
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_spec_without_operator_is_exact() {
        let spec = parse_dimension_spec("freshness@1.2").unwrap();
        assert!(spec.requirement.matches(&Version::parse("1.2.0").unwrap()));
        assert!(!spec.requirement.matches(&Version::parse("1.2.1").unwrap()));
    }

    #[test]
    fn registry_relative_paths_resolve_from_index_dir() {
        let resolved =
            resolve_registry_location("registry/index.toml", "artifacts/freshness-1.0.0.dim.toml")
                .unwrap();
        let path = Path::new(&resolved);
        assert!(path.ends_with(Path::new("registry").join("artifacts").join("freshness-1.0.0.dim.toml")));
    }
}
