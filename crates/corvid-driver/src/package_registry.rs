//! Package registry resolution for `corvid add`.
//!
//! This is deliberately lockfile-first. The registry chooses a concrete
//! package version; the installed result is a `Corvid.lock` entry with URL,
//! SHA-256, signature metadata, and the package's semantic summary.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use corvid_resolve::ModuleSemanticSummary;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::import_integrity::sha256_hex;
use crate::modules::summarize_module_source;
use crate::package_lock::{
    load_or_empty_at, lock_path_for_project, remove_packages_by_name, upsert_package,
    write_package_lock, LockedPackage,
};
use crate::package_manifest::{dependency, remove_dependency, upsert_dependency};
use crate::package_policy::{load_package_policy, package_policy_violation};
use crate::package_version::{
    normalize_version, parse_package_spec, validate_package_name, PackageSpec,
};

const DEFAULT_REGISTRY: &str = "https://registry.corvid.dev/index.toml";

#[derive(Debug, Clone)]
pub enum AddPackageOutcome {
    Added {
        uri: String,
        version: String,
        lockfile: PathBuf,
        exports: usize,
    },
    Rejected {
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub enum PackageMutationOutcome {
    Removed {
        name: String,
        manifest_updated: bool,
        lock_entries_removed: usize,
        lockfile: PathBuf,
    },
    Updated {
        uri: String,
        version: String,
        lockfile: PathBuf,
        exports: usize,
    },
    Rejected {
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub struct PublishPackageOptions<'a> {
    pub source: &'a Path,
    pub name: &'a str,
    pub version: &'a str,
    pub out_dir: &'a Path,
    pub url_base: &'a str,
    pub signing_seed_hex: &'a str,
    pub key_id: &'a str,
}

#[derive(Debug, Clone)]
pub struct PublishPackageOutcome {
    pub uri: String,
    pub index: PathBuf,
    pub artifact: PathBuf,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegistryVerificationReport {
    pub registry: String,
    pub checked: usize,
    pub failures: Vec<RegistryVerificationFailure>,
}

impl RegistryVerificationReport {
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RegistryVerificationFailure {
    pub package: String,
    pub version: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct RegistryIndex {
    #[serde(default)]
    package: Vec<RegistryPackage>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct RegistryPackage {
    name: String,
    version: String,
    uri: Option<String>,
    url: String,
    sha256: String,
    #[serde(default)]
    registry: Option<String>,
    #[serde(default)]
    signature: Option<String>,
    #[serde(default)]
    semantic_summary: Option<ModuleSemanticSummary>,
}

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
    let source = String::from_utf8(bytes)
        .with_context(|| format!("package `{}`@{} is not valid UTF-8", selected.name, selected.version))?;
    let summary = summarize_module_source(&source)
        .map_err(|message| anyhow!("package `{}`@{} failed semantic summary build: {message}", selected.name, selected.version))?;
    let policy = load_package_policy(project_dir)?;
    if let Some(reason) = package_policy_violation(&summary, &policy, selected.signature.as_deref()) {
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
            AddPackageOutcome::Rejected { reason } => Ok(PackageMutationOutcome::Rejected { reason }),
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

pub fn publish_package(options: PublishPackageOptions<'_>) -> Result<PublishPackageOutcome> {
    validate_package_name(options.name)?;
    let version = Version::parse(&normalize_version(options.version))
        .with_context(|| format!("invalid package version `{}`", options.version))?;
    std::fs::create_dir_all(options.out_dir)
        .with_context(|| format!("create registry output dir `{}`", options.out_dir.display()))?;
    let source = std::fs::read_to_string(options.source)
        .with_context(|| format!("read package source `{}`", options.source.display()))?;
    let summary = summarize_module_source(&source)
        .map_err(|message| anyhow!("package source failed semantic summary build: {message}"))?;
    let artifact_name = format!(
        "{}-{}.cor",
        options
            .name
            .trim_start_matches('@')
            .replace(['/', '\\'], "-"),
        version
    );
    let artifact = options.out_dir.join(&artifact_name);
    std::fs::write(&artifact, source.as_bytes())
        .with_context(|| format!("write package artifact `{}`", artifact.display()))?;
    let sha256 = sha256_hex(source.as_bytes());
    let uri = format!("corvid://{}/v{}", options.name, version);
    let url = format!(
        "{}/{}",
        options.url_base.trim_end_matches('/'),
        artifact_name
    );
    let mut package = RegistryPackage {
        name: options.name.to_string(),
        version: version.to_string(),
        uri: Some(uri.clone()),
        url,
        sha256: sha256.clone(),
        registry: None,
        signature: None,
        semantic_summary: Some(summary),
    };
    package.signature = Some(sign_package(&package, options.signing_seed_hex, options.key_id)?);

    let index_path = options.out_dir.join("index.toml");
    let mut index = match std::fs::read_to_string(&index_path) {
        Ok(source) => toml::from_str::<RegistryIndex>(&source)
            .with_context(|| format!("parse existing registry index `{}`", index_path.display()))?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => RegistryIndex::default(),
        Err(err) => return Err(anyhow!("read `{}`: {err}", index_path.display())),
    };
    upsert_registry_package(&mut index, package);
    let index_source = toml::to_string_pretty(&index)
        .with_context(|| format!("serialize registry index `{}`", index_path.display()))?;
    std::fs::write(&index_path, index_source)
        .with_context(|| format!("write registry index `{}`", index_path.display()))?;

    Ok(PublishPackageOutcome {
        uri,
        index: index_path,
        artifact,
        sha256,
    })
}

pub fn verify_registry_contract(location: &str) -> Result<RegistryVerificationReport> {
    let index = load_registry_index(location)?;
    let mut report = RegistryVerificationReport {
        registry: location.to_string(),
        checked: 0,
        failures: Vec::new(),
    };
    let mut seen = std::collections::BTreeSet::<(String, String)>::new();

    for package in &index.package {
        report.checked += 1;
        if !seen.insert((package.name.clone(), package.version.clone())) {
            report.failures.push(failure(
                package,
                "duplicate package name/version entry in registry index",
            ));
            continue;
        }
        if let Err(err) = validate_registry_entry_contract(package) {
            report.failures.push(failure(package, err));
            continue;
        }
        let fetched = match fetch_package_for_verification(&package.url) {
            Ok(fetched) => fetched,
            Err(err) => {
                report.failures.push(failure(package, err.to_string()));
                continue;
            }
        };
        if let Some(reason) = cache_header_violation(&fetched.cache_control) {
            report.failures.push(failure(package, reason));
        }
        let actual = sha256_hex(&fetched.bytes);
        if !actual.eq_ignore_ascii_case(&package.sha256) {
            report.failures.push(failure(
                package,
                format!(
                    "artifact hash mismatch: expected sha256:{}, actual sha256:{actual}",
                    package.sha256
                ),
            ));
            continue;
        }
        let source = match String::from_utf8(fetched.bytes) {
            Ok(source) => source,
            Err(err) => {
                report.failures.push(failure(package, format!("artifact is not UTF-8: {err}")));
                continue;
            }
        };
        let summary = match summarize_module_source(&source) {
            Ok(summary) => summary,
            Err(err) => {
                report.failures.push(failure(package, format!("semantic summary failed: {err}")));
                continue;
            }
        };
        if let Some(index_summary) = &package.semantic_summary {
            if index_summary != &summary {
                report.failures.push(failure(
                    package,
                    "index semantic_summary does not match artifact source",
                ));
            }
        }
        if let Some(signature) = &package.signature {
            if let Err(reason) = verify_package_signature(package, &summary, signature) {
                report.failures.push(failure(package, reason));
            }
        }
    }

    Ok(report)
}

fn select_package<'a>(
    index: &'a RegistryIndex,
    spec: &PackageSpec,
) -> Result<Option<&'a RegistryPackage>> {
    let mut candidates = Vec::new();
    for package in &index.package {
        if package.name != spec.name {
            continue;
        }
        let version = Version::parse(&normalize_version(&package.version))
            .with_context(|| format!("registry package `{}` has invalid version `{}`", package.name, package.version))?;
        if spec.requirement.matches(&version) {
            candidates.push((version, package));
        }
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(candidates.pop().map(|(_, package)| package))
}

fn load_registry_index(location: &str) -> Result<RegistryIndex> {
    let source = if location.starts_with("http://") || location.starts_with("https://") {
        let bytes = fetch_bytes(location)
            .with_context(|| format!("failed to fetch registry index `{location}`"))?;
        String::from_utf8(bytes).with_context(|| format!("registry index `{location}` is not UTF-8"))?
    } else {
        let path = Path::new(location);
        let path = if path.is_dir() {
            path.join("index.toml")
        } else {
            path.to_path_buf()
        };
        std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read registry index `{}`", path.display()))?
    };
    toml::from_str(&source).context("failed to parse package registry index")
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

fn upsert_registry_package(index: &mut RegistryIndex, package: RegistryPackage) {
    if let Some(existing) = index
        .package
        .iter_mut()
        .find(|entry| entry.name == package.name && entry.version == package.version)
    {
        *existing = package;
    } else {
        index.package.push(package);
        index.package.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| normalize_version(&a.version).cmp(&normalize_version(&b.version)))
        });
    }
}

fn validate_registry_entry_contract(package: &RegistryPackage) -> Result<(), String> {
    validate_package_name(&package.name).map_err(|err| err.to_string())?;
    let version = Version::parse(&normalize_version(&package.version))
        .map_err(|err| format!("invalid semver version `{}`: {err}", package.version))?;
    let expected_uri = format!("corvid://{}/v{}", package.name, version);
    if let Some(uri) = &package.uri {
        if uri != &expected_uri {
            return Err(format!(
                "uri `{uri}` does not match canonical package uri `{expected_uri}`"
            ));
        }
    }
    if !(package.url.starts_with("https://") || package.url.starts_with("http://")) {
        return Err("artifact URL must be http(s)".to_string());
    }
    if package.url.contains('?') || package.url.contains('#') {
        return Err("artifact URL must be immutable: query strings and fragments are forbidden".to_string());
    }
    if !package.url.ends_with(".cor") {
        return Err("artifact URL must point at a `.cor` source artifact".to_string());
    }
    if !package.url.contains(&version.to_string()) {
        return Err(format!(
            "artifact URL must include the concrete version `{}`",
            version
        ));
    }
    if package.sha256.len() != 64 || !package.sha256.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(format!("invalid sha256 digest `{}`", package.sha256));
    }
    Ok(())
}

struct FetchedPackage {
    bytes: Vec<u8>,
    cache_control: Option<String>,
}

fn fetch_package_for_verification(location: &str) -> Result<FetchedPackage> {
    let response = ureq::get(location).call().map_err(|err| anyhow!(err.to_string()))?;
    if !(200..=299).contains(&response.status()) {
        return Err(anyhow!("HTTP status {}", response.status()));
    }
    let cache_control = response.header("Cache-Control").map(str::to_string);
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .context("failed reading HTTP response")?;
    Ok(FetchedPackage {
        bytes,
        cache_control,
    })
}

fn cache_header_violation(cache_control: &Option<String>) -> Option<String> {
    let Some(value) = cache_control else {
        return Some("artifact response must include Cache-Control".to_string());
    };
    let lower = value.to_ascii_lowercase();
    if !lower.contains("immutable") {
        return Some("artifact Cache-Control must include `immutable`".to_string());
    }
    if !lower.contains("max-age=") {
        return Some("artifact Cache-Control must include `max-age=`".to_string());
    }
    None
}

fn failure(package: &RegistryPackage, reason: impl Into<String>) -> RegistryVerificationFailure {
    RegistryVerificationFailure {
        package: package.name.clone(),
        version: package.version.clone(),
        reason: reason.into(),
    }
}

fn sign_package(package: &RegistryPackage, seed_hex: &str, key_id: &str) -> Result<String> {
    let seed = decode_hex_32(seed_hex).context("signing seed must be 32 bytes of hex")?;
    let signing_key = SigningKey::from_bytes(&seed);
    let verifying_key = signing_key.verifying_key();
    let subject = package_signature_subject(package)?;
    let signature = signing_key.sign(&subject);
    Ok(format!(
        "ed25519:{}:{}:{}",
        key_id,
        encode_hex(verifying_key.as_bytes()),
        encode_hex(&signature.to_bytes())
    ))
}

fn verify_package_signature(
    package: &RegistryPackage,
    summary: &ModuleSemanticSummary,
    signature: &str,
) -> Result<(), String> {
    let mut signed_package = package.clone();
    signed_package.semantic_summary = Some(summary.clone());
    let parts = signature.split(':').collect::<Vec<_>>();
    if parts.len() != 4 || parts[0] != "ed25519" {
        return Err(format!(
            "package `{}`@{} has unsupported signature format",
            package.name, package.version
        ));
    }
    let key = decode_hex_32(parts[2])
        .map_err(|err| format!("package `{}`@{} has invalid verifying key: {err}", package.name, package.version))?;
    let verifying_key = VerifyingKey::from_bytes(&key)
        .map_err(|err| format!("package `{}`@{} has invalid verifying key: {err}", package.name, package.version))?;
    let sig_bytes = decode_hex_64(parts[3])
        .map_err(|err| format!("package `{}`@{} has invalid signature: {err}", package.name, package.version))?;
    let sig = Signature::from_bytes(&sig_bytes);
    let subject = package_signature_subject(&signed_package)
        .map_err(|err| format!("package `{}`@{} signature subject failed: {err}", package.name, package.version))?;
    verifying_key
        .verify(&subject, &sig)
        .map_err(|err| format!("package `{}`@{} signature verification failed: {err}", package.name, package.version))
}

fn package_signature_subject(package: &RegistryPackage) -> Result<Vec<u8>> {
    let summary = package
        .semantic_summary
        .as_ref()
        .ok_or_else(|| anyhow!("package signature requires semantic_summary"))?;
    let summary_json = serde_json::to_string(summary).context("serialize semantic summary")?;
    Ok(format!(
        "corvid-package-v1\nname:{}\nversion:{}\nuri:{}\nurl:{}\nsha256:{}\nsummary:{}\n",
        package.name,
        package.version,
        package
            .uri
            .as_deref()
            .unwrap_or("<none>"),
        package.url,
        package.sha256.to_ascii_lowercase(),
        summary_json
    )
    .into_bytes())
}

fn decode_hex_32(input: &str) -> Result<[u8; 32]> {
    let bytes = decode_hex(input)?;
    let len = bytes.len();
    bytes
        .try_into()
        .map_err(|_| anyhow!("expected 32 bytes, got {len}"))
}

fn decode_hex_64(input: &str) -> Result<[u8; 64]> {
    let bytes = decode_hex(input)?;
    let len = bytes.len();
    bytes
        .try_into()
        .map_err(|_| anyhow!("expected 64 bytes, got {len}"))
}

fn decode_hex(input: &str) -> Result<Vec<u8>> {
    if input.len() % 2 != 0 {
        return Err(anyhow!("hex string has odd length"));
    }
    let mut out = Vec::with_capacity(input.len() / 2);
    for pair in input.as_bytes().chunks(2) {
        let hi = hex_value(pair[0])?;
        let lo = hex_value(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_value(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(anyhow!("non-hex byte `{}`", byte as char)),
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package_version::VersionRequirement;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn package_spec_uses_last_at_for_scoped_name() {
        let spec = parse_package_spec("@anthropic/safety-baseline@2.3").unwrap();
        assert_eq!(spec.name, "@anthropic/safety-baseline");
        assert!(matches!(
            spec.requirement,
            VersionRequirement::Prefix { ref parts } if parts == &[2, 3]
        ));
    }

    #[test]
    fn resolver_selects_highest_matching_patch() {
        let index = RegistryIndex {
            package: vec![
                registry_pkg("2.3.0"),
                registry_pkg("2.3.4"),
                registry_pkg("2.4.0"),
            ],
        };
        let spec = parse_package_spec("@anthropic/safety-baseline@2.3").unwrap();
        let selected = select_package(&index, &spec).unwrap().unwrap();
        assert_eq!(selected.version, "2.3.4");
    }

    fn registry_pkg(version: &str) -> RegistryPackage {
        RegistryPackage {
            name: "@anthropic/safety-baseline".to_string(),
            version: version.to_string(),
            uri: None,
            url: "package.cor".to_string(),
            sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_string(),
            registry: None,
            signature: None,
            semantic_summary: None,
        }
    }

    #[test]
    fn add_package_writes_lock_with_semantic_summary() {
        let tmp = tempfile::tempdir().unwrap();
        let package_src = "public type SafetyReceipt:\n    id: String\n";
        let digest = sha256_hex(package_src.as_bytes());
        let url = serve_once("/safety.cor", package_src);
        let index = tmp.path().join("index.toml");
        fs::write(
            &index,
            format!(
                "\
[[package]]
name = \"@anthropic/safety-baseline\"
version = \"2.3.4\"
uri = \"corvid://@anthropic/safety-baseline/v2.3.4\"
url = \"{url}\"
sha256 = \"{digest}\"
"
            ),
        )
        .unwrap();

        let outcome = add_package(
            "@anthropic/safety-baseline@2.3",
            tmp.path(),
            Some(index.to_str().unwrap()),
        )
        .unwrap();

        match &outcome {
            AddPackageOutcome::Added { uri, .. } => {
                assert_eq!(uri, "corvid://@anthropic/safety-baseline/v2.3.4");
            }
            other => panic!("expected added package, got {other:?}"),
        }
        let lock = fs::read_to_string(tmp.path().join("Corvid.lock")).unwrap();
        assert!(lock.contains("semantic_summary"));
        assert!(lock.contains("SafetyReceipt"));
        let manifest = fs::read_to_string(tmp.path().join("corvid.toml")).unwrap();
        assert!(manifest.contains("[dependencies.\"@anthropic/safety-baseline\"]"), "{manifest}");
        assert!(manifest.contains("version = \"2.3\""), "{manifest}");
    }

    #[test]
    fn add_package_rejects_project_policy_violation() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("corvid.toml"),
            "[package-policy]\nrequire-deterministic = true\n",
        )
        .unwrap();
        let package_src = "public agent helper() -> Bool:\n    return true\n";
        let digest = sha256_hex(package_src.as_bytes());
        let url = serve_once("/helper.cor", package_src);
        let index = tmp.path().join("index.toml");
        fs::write(
            &index,
            format!(
                "\
[[package]]
name = \"@scope/helper\"
version = \"1.0.0\"
url = \"{url}\"
sha256 = \"{digest}\"
"
            ),
        )
        .unwrap();

        let outcome = add_package("@scope/helper@1", tmp.path(), Some(index.to_str().unwrap()))
            .unwrap();

        assert!(matches!(
            outcome,
            AddPackageOutcome::Rejected { ref reason }
                if reason.contains("require-deterministic")
        ));
        assert!(
            !tmp.path().join("Corvid.lock").exists(),
            "rejected packages must not write a lockfile"
        );
    }

    #[test]
    fn publish_package_signs_index_and_add_verifies_signature() {
        let tmp = tempfile::tempdir().unwrap();
        let package_src = "public type SafetyReceipt:\n    id: String\n";
        let source = tmp.path().join("policy.cor");
        fs::write(&source, package_src).unwrap();
        let url = serve_once("/scope-name-1.0.0.cor", package_src);
        let url_base = url.trim_end_matches("/scope-name-1.0.0.cor");
        let seed = "0000000000000000000000000000000000000000000000000000000000000000";

        let published = publish_package(PublishPackageOptions {
            source: &source,
            name: "@scope/name",
            version: "1.0.0",
            out_dir: tmp.path(),
            url_base,
            signing_seed_hex: seed,
            key_id: "test-key",
        })
        .unwrap();
        let outcome = add_package("@scope/name@1", tmp.path(), Some(published.index.to_str().unwrap()))
            .unwrap();

        assert!(matches!(outcome, AddPackageOutcome::Added { .. }));
        let lock = fs::read_to_string(tmp.path().join("Corvid.lock")).unwrap();
        assert!(lock.contains("ed25519:test-key"));
    }

    #[test]
    fn add_package_rejects_tampered_signature_when_policy_requires_signatures() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("corvid.toml"),
            "[package-policy]\nrequire-package-signatures = true\n",
        )
        .unwrap();
        let package_src = "public type SafetyReceipt:\n    id: String\n";
        let source = tmp.path().join("policy.cor");
        fs::write(&source, package_src).unwrap();
        let url = serve_once("/scope-name-1.0.0.cor", package_src);
        let url_base = url.trim_end_matches("/scope-name-1.0.0.cor");
        let seed = "0000000000000000000000000000000000000000000000000000000000000000";
        let published = publish_package(PublishPackageOptions {
            source: &source,
            name: "@scope/name",
            version: "1.0.0",
            out_dir: tmp.path(),
            url_base,
            signing_seed_hex: seed,
            key_id: "test-key",
        })
        .unwrap();
        let mut index = fs::read_to_string(&published.index).unwrap();
        let sig_start = index.find("signature = \"").unwrap() + "signature = \"".len();
        let sig_end = index[sig_start..].find('"').unwrap() + sig_start;
        let sig_last = sig_end - 1;
        index.replace_range(
            sig_last..sig_end,
            if &index[sig_last..sig_end] == "0" { "1" } else { "0" },
        );
        fs::write(&published.index, index).unwrap();

        let outcome = add_package("@scope/name@1", tmp.path(), Some(published.index.to_str().unwrap()))
            .unwrap();

        assert!(matches!(
            outcome,
            AddPackageOutcome::Rejected { ref reason }
                if reason.contains("signature verification failed")
        ));
    }

    #[test]
    fn registry_contract_verifies_hash_cache_and_summary() {
        let tmp = tempfile::tempdir().unwrap();
        let package_src = "public type SafetyReceipt:\n    id: String\n";
        let digest = sha256_hex(package_src.as_bytes());
        let (base_url, _server) = serve_many(vec![(
            "/scope-name-1.0.0.cor",
            package_src.to_string(),
            Some("public, max-age=31536000, immutable".to_string()),
        )]);
        fs::write(
            tmp.path().join("index.toml"),
            format!(
                "\
[[package]]
name = \"@scope/name\"
version = \"1.0.0\"
uri = \"corvid://@scope/name/v1.0.0\"
url = \"{base_url}/scope-name-1.0.0.cor\"
sha256 = \"{digest}\"
"
            ),
        )
        .unwrap();

        let report = verify_registry_contract(tmp.path().join("index.toml").to_str().unwrap())
            .unwrap();

        assert!(report.is_clean(), "{report:?}");
        assert_eq!(report.checked, 1);
    }

    #[test]
    fn registry_contract_reports_missing_immutable_cache_header() {
        let tmp = tempfile::tempdir().unwrap();
        let package_src = "public type SafetyReceipt:\n    id: String\n";
        let digest = sha256_hex(package_src.as_bytes());
        let (base_url, _server) = serve_many(vec![(
            "/scope-name-1.0.0.cor",
            package_src.to_string(),
            Some("public, max-age=31536000".to_string()),
        )]);
        fs::write(
            tmp.path().join("index.toml"),
            format!(
                "\
[[package]]
name = \"@scope/name\"
version = \"1.0.0\"
url = \"{base_url}/scope-name-1.0.0.cor\"
sha256 = \"{digest}\"
"
            ),
        )
        .unwrap();

        let report = verify_registry_contract(tmp.path().join("index.toml").to_str().unwrap())
            .unwrap();

        assert_eq!(report.failures.len(), 1, "{report:?}");
        assert!(report.failures[0].reason.contains("immutable"));
    }

    #[test]
    fn remove_package_updates_manifest_and_lock() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("corvid.toml"),
            "[dependencies]\n\"@scope/name\" = \"1\"\n",
        )
        .unwrap();
        fs::write(
            tmp.path().join("Corvid.lock"),
            "\
[[package]]
uri = \"corvid://@scope/name/v1.0.0\"
url = \"https://example.com/name.cor\"
sha256 = \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\"

[[package]]
uri = \"corvid://@scope/other/v1.0.0\"
url = \"https://example.com/other.cor\"
sha256 = \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\"
",
        )
        .unwrap();

        let outcome = remove_package("@scope/name", tmp.path()).unwrap();

        assert!(matches!(
            outcome,
            PackageMutationOutcome::Removed {
                manifest_updated: true,
                lock_entries_removed: 1,
                ..
            }
        ));
        let manifest = fs::read_to_string(tmp.path().join("corvid.toml")).unwrap();
        let lock = fs::read_to_string(tmp.path().join("Corvid.lock")).unwrap();
        assert!(!manifest.contains("@scope/name"), "{manifest}");
        assert!(!lock.contains("corvid://@scope/name/"), "{lock}");
        assert!(lock.contains("corvid://@scope/other/"), "{lock}");
    }

    #[test]
    fn update_package_uses_manifest_requirement_and_selects_newest_match() {
        let tmp = tempfile::tempdir().unwrap();
        let old_src = "public type OldReceipt:\n    id: String\n";
        let new_src = "public type NewReceipt:\n    id: String\n";
        let old_url = serve_once("/helper-1.0.0.cor", old_src);
        let new_url = serve_once("/helper-1.2.0.cor", new_src);
        fs::write(
            tmp.path().join("corvid.toml"),
            format!(
                "\
[dependencies.\"@scope/helper\"]
version = \"1\"
registry = \"{}\"
",
                tmp.path()
                    .join("index.toml")
                    .to_string_lossy()
                    .replace('\\', "/")
            ),
        )
        .unwrap();
        fs::write(
            tmp.path().join("index.toml"),
            format!(
                "\
[[package]]
name = \"@scope/helper\"
version = \"1.0.0\"
url = \"{old_url}\"
sha256 = \"{}\"

[[package]]
name = \"@scope/helper\"
version = \"1.2.0\"
url = \"{new_url}\"
sha256 = \"{}\"
",
                sha256_hex(old_src.as_bytes()),
                sha256_hex(new_src.as_bytes()),
            ),
        )
        .unwrap();

        let outcome = update_package("@scope/helper", tmp.path(), None).unwrap();

        assert!(matches!(
            outcome,
            PackageMutationOutcome::Updated { ref version, .. } if version == "1.2.0"
        ));
        let lock = fs::read_to_string(tmp.path().join("Corvid.lock")).unwrap();
        assert!(lock.contains("corvid://@scope/helper/v1.2.0"), "{lock}");
        assert!(lock.contains("NewReceipt"), "{lock}");
    }

    fn serve_once(path: &'static str, body: impl Into<String>) -> String {
        let body = body.into();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let n = stream.read(&mut request).unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..n]);
            let status = if request.starts_with(&format!("GET {path} ")) {
                "HTTP/1.1 200 OK"
            } else {
                "HTTP/1.1 404 Not Found"
            };
            let body = if status.contains("200") {
                body.as_str()
            } else {
                "not found"
            };
            write!(
                stream,
                "{status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.as_bytes().len(),
                body
            )
            .unwrap();
        });
        format!("http://{addr}{path}")
    }

    fn serve_many(
        routes: Vec<(&'static str, String, Option<String>)>,
    ) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            for _ in 0..routes.len() {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0u8; 1024];
                let n = stream.read(&mut request).unwrap_or(0);
                let request = String::from_utf8_lossy(&request[..n]);
                let route = routes
                    .iter()
                    .find(|(path, _, _)| request.starts_with(&format!("GET {path} ")));
                let (status, body, cache) = match route {
                    Some((_, body, cache)) => ("HTTP/1.1 200 OK", body.as_str(), cache.as_deref()),
                    None => ("HTTP/1.1 404 Not Found", "not found", None),
                };
                write!(
                    stream,
                    "{status}\r\nContent-Length: {}\r\nConnection: close\r\n",
                    body.as_bytes().len()
                )
                .unwrap();
                if let Some(cache) = cache {
                    write!(stream, "Cache-Control: {cache}\r\n").unwrap();
                }
                write!(stream, "\r\n{body}").unwrap();
            }
        });
        (format!("http://{addr}"), handle)
    }
}
