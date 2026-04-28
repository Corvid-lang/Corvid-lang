use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const BUNDLE_SCHEMA_VERSION: u32 = 1;
pub const MANIFEST_FILE: &str = "corvid-bundle.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleManifest {
    pub bundle_schema_version: u32,
    pub name: String,
    pub target_triple: String,
    pub primary_source: String,
    #[serde(default)]
    pub tools_staticlib_path: Option<String>,
    pub library_path: String,
    pub descriptor_path: String,
    #[serde(default)]
    pub header_path: Option<String>,
    pub bindings_rust_dir: String,
    pub bindings_python_dir: String,
    #[serde(default)]
    pub capsule_path: Option<String>,
    #[serde(default)]
    pub receipt_envelope_path: Option<String>,
    #[serde(default)]
    pub receipt_verify_key_path: Option<String>,
    #[serde(default)]
    pub lineage: BundleLineage,
    #[serde(default)]
    pub traces: Vec<BundleTrace>,
    pub hashes: BundleHashes,
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleHashes {
    pub library: String,
    pub descriptor: String,
    #[serde(default)]
    pub header: Option<String>,
    pub bindings_rust: String,
    pub bindings_python: String,
    #[serde(default)]
    pub capsule: Option<String>,
    #[serde(default)]
    pub receipt_envelope: Option<String>,
    #[serde(default)]
    pub receipt_verify_key: Option<String>,
    #[serde(default)]
    pub tools_staticlib: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleTrace {
    pub name: String,
    pub path: String,
    pub source: String,
    pub sha256: String,
    pub expected_agent: String,
    #[serde(default)]
    pub expected_result_json: String,
    #[serde(default)]
    pub expected_grounded_sources: Vec<String>,
    #[serde(default)]
    pub expected_observation: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BundleLineage {
    #[serde(default)]
    pub bundle_id: Option<String>,
    #[serde(default)]
    pub predecessors: Vec<BundlePredecessor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundlePredecessor {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub relation: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LoadedManifest {
    pub bundle_dir: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest: BundleManifest,
}

impl LoadedManifest {
    pub fn load(bundle_path: &Path) -> Result<Self> {
        let bundle_dir = if bundle_path.is_dir() {
            bundle_path.to_path_buf()
        } else {
            bundle_path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("bundle path `{}` has no parent", bundle_path.display()))?
                .to_path_buf()
        };
        let manifest_path = if bundle_path.is_dir() {
            bundle_dir.join(MANIFEST_FILE)
        } else {
            bundle_path.to_path_buf()
        };
        let raw = fs::read_to_string(&manifest_path)
            .with_context(|| format!("read bundle manifest `{}`", manifest_path.display()))?;
        let manifest: BundleManifest = toml::from_str(&raw)
            .with_context(|| format!("parse bundle manifest `{}`", manifest_path.display()))?;
        if manifest.bundle_schema_version != BUNDLE_SCHEMA_VERSION {
            bail!(
                "BundleSchemaVersionMismatch: `{}` declares schema version {}, expected {}",
                manifest_path.display(),
                manifest.bundle_schema_version,
                BUNDLE_SCHEMA_VERSION
            );
        }
        Ok(Self {
            bundle_dir,
            manifest_path,
            manifest,
        })
    }

    pub fn resolve(&self, relative: &str) -> PathBuf {
        self.bundle_dir.join(relative)
    }

    pub fn primary_source_path(&self) -> PathBuf {
        self.resolve(&self.manifest.primary_source)
    }

    pub fn descriptor_path(&self) -> PathBuf {
        self.resolve(&self.manifest.descriptor_path)
    }

    pub fn library_path(&self) -> PathBuf {
        self.resolve(&self.manifest.library_path)
    }

    pub fn header_path(&self) -> Option<PathBuf> {
        self.manifest.header_path.as_deref().map(|path| self.resolve(path))
    }

    pub fn tools_staticlib_path(&self) -> Option<PathBuf> {
        self.manifest
            .tools_staticlib_path
            .as_deref()
            .map(|path| self.resolve(path))
    }

    pub fn bindings_rust_dir(&self) -> PathBuf {
        self.resolve(&self.manifest.bindings_rust_dir)
    }

    pub fn bindings_python_dir(&self) -> PathBuf {
        self.resolve(&self.manifest.bindings_python_dir)
    }

    pub fn capsule_path(&self) -> Option<PathBuf> {
        self.manifest.capsule_path.as_deref().map(|path| self.resolve(path))
    }

    pub fn receipt_envelope_path(&self) -> Option<PathBuf> {
        self.manifest
            .receipt_envelope_path
            .as_deref()
            .map(|path| self.resolve(path))
    }

    pub fn receipt_verify_key_path(&self) -> Option<PathBuf> {
        self.manifest
            .receipt_verify_key_path
            .as_deref()
            .map(|path| self.resolve(path))
    }

    pub fn predecessor_path(&self, name: Option<&str>) -> Result<PathBuf> {
        let predecessors = &self.manifest.lineage.predecessors;
        if predecessors.is_empty() {
            bail!(
                "BundleLineageMissing: bundle `{}` declares no predecessors",
                self.manifest.name
            );
        }
        match name {
            Some(name) => predecessors
                .iter()
                .find(|pred| pred.name == name)
                .map(|pred| self.resolve(&pred.path))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "BundlePredecessorNotFound: bundle `{}` has no predecessor named `{}`",
                        self.manifest.name,
                        name
                    )
                }),
            None if predecessors.len() == 1 => Ok(self.resolve(&predecessors[0].path)),
            None => bail!(
                "BundlePredecessorAmbiguous: bundle `{}` has {} predecessors; pass --predecessor <name>",
                self.manifest.name,
                predecessors.len()
            ),
        }
    }
}

pub fn current_target_triple() -> &'static str {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "linux") => "x86_64-unknown-linux-gnu",
        ("aarch64", "linux") => "aarch64-unknown-linux-gnu",
        ("x86_64", "windows") => "x86_64-pc-windows-msvc",
        ("aarch64", "windows") => "aarch64-pc-windows-msvc",
        ("x86_64", "macos") => "x86_64-apple-darwin",
        ("aarch64", "macos") => "aarch64-apple-darwin",
        _ => "unknown-target",
    }
}

pub fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    encode_hex(&hasher.finalize())
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read `{}` for hashing", path.display()))?;
    Ok(sha256_bytes(&bytes))
}

pub fn sha256_dir(path: &Path) -> Result<String> {
    let mut files = Vec::new();
    collect_files(path, path, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    for (relative, bytes) in files {
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        hasher.update(bytes.len().to_le_bytes());
        hasher.update(bytes);
    }
    Ok(encode_hex(&hasher.finalize()))
}

fn collect_files(root: &Path, current: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    let mut entries = fs::read_dir(current)
        .with_context(|| format!("read directory `{}`", current.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("enumerate directory `{}`", current.display()))?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else if path.is_file() {
            let relative = path
                .strip_prefix(root)
                .expect("child path stays under root")
                .to_string_lossy()
                .replace('\\', "/");
            let bytes = fs::read(&path).with_context(|| format!("read `{}`", path.display()))?;
            out.push((relative, bytes));
        }
    }
    Ok(())
}

pub fn compare_bytes(label: &str, expected: &[u8], actual: &[u8]) -> Result<()> {
    if expected == actual {
        return Ok(());
    }
    let first_diff = expected
        .iter()
        .zip(actual.iter())
        .position(|(left, right)| left != right)
        .unwrap_or_else(|| expected.len().min(actual.len()));
    bail!(
        "BundleRebuildMismatch: {label} diverged at byte {} (expected len {}, actual len {})",
        first_diff,
        expected.len(),
        actual.len()
    );
}

#[allow(dead_code)]
pub fn compare_paths(label: &str, expected_path: &Path, actual_path: &Path) -> Result<()> {
    let expected =
        fs::read(expected_path).with_context(|| format!("read expected `{}`", expected_path.display()))?;
    let actual =
        fs::read(actual_path).with_context(|| format!("read rebuilt `{}`", actual_path.display()))?;
    compare_bytes(label, &expected, &actual)
}

pub fn compare_dirs(label: &str, expected_path: &Path, actual_path: &Path) -> Result<()> {
    let expected_hash = sha256_dir(expected_path)?;
    let actual_hash = sha256_dir(actual_path)?;
    if expected_hash == actual_hash {
        return Ok(());
    }
    bail!(
        "BundleRebuildMismatch: {label} directory hash diverged (expected {}, actual {})",
        expected_hash,
        actual_hash
    );
}
