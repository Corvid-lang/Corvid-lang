//! Package registry resolution for `corvid add`.
//!
//! This is deliberately lockfile-first. The registry chooses a concrete
//! package version; the installed result is a `Corvid.lock` entry with URL,
//! SHA-256, signature metadata, and the package's semantic summary.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use corvid_resolve::ModuleSemanticSummary;
use corvid_types::{CorvidConfig, PackagePolicyConfig};
use semver::{Version, VersionReq};
use serde::Deserialize;

use crate::import_integrity::sha256_hex;
use crate::modules::summarize_module_source;
use crate::package_lock::{
    load_or_empty_at, lock_path_for_project, upsert_package, write_package_lock, LockedPackage,
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

#[derive(Debug, Clone, Deserialize)]
struct RegistryIndex {
    #[serde(default)]
    package: Vec<RegistryPackage>,
}

#[derive(Debug, Clone, Deserialize)]
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
}

#[derive(Debug, Clone)]
struct PackageSpec {
    name: String,
    requirement: VersionRequirement,
}

#[derive(Debug, Clone)]
enum VersionRequirement {
    Prefix { parts: Vec<u64> },
    Semver(VersionReq),
}

impl VersionRequirement {
    fn matches(&self, version: &Version) -> bool {
        match self {
            Self::Prefix { parts } => {
                parts.first().is_none_or(|major| version.major == *major)
                    && parts.get(1).is_none_or(|minor| version.minor == *minor)
                    && parts.get(2).is_none_or(|patch| version.patch == *patch)
            }
            Self::Semver(req) => req.matches(version),
        }
    }
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
    if let Some(reason) = package_policy_violation(&summary, &policy) {
        return Ok(AddPackageOutcome::Rejected { reason });
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

    Ok(AddPackageOutcome::Added {
        uri,
        version: selected.version.clone(),
        lockfile,
        exports: summary.exports.len(),
    })
}

fn parse_package_spec(spec: &str) -> Result<PackageSpec> {
    let Some(idx) = spec.rfind('@') else {
        return Err(anyhow!(
            "package spec must be `@scope/name@version`, got `{spec}`"
        ));
    };
    if idx == 0 {
        return Err(anyhow!(
            "package spec must be `@scope/name@version`, got `{spec}`"
        ));
    }
    let name = &spec[..idx];
    let version = &spec[idx + 1..];
    if !name.starts_with('@') || !name.contains('/') || version.is_empty() {
        return Err(anyhow!(
            "package spec must be `@scope/name@version`, got `{spec}`"
        ));
    }
    Ok(PackageSpec {
        name: name.to_string(),
        requirement: parse_version_requirement(version)?,
    })
}

fn parse_version_requirement(raw: &str) -> Result<VersionRequirement> {
    if raw.chars().next().is_some_and(|ch| matches!(ch, '^' | '~' | '>' | '<' | '=' | '*')) {
        return VersionReq::parse(raw)
            .map(VersionRequirement::Semver)
            .map_err(|err| anyhow!("invalid semver requirement `{raw}`: {err}"));
    }
    let parts = raw
        .split('.')
        .map(|part| {
            part.parse::<u64>()
                .map_err(|err| anyhow!("invalid version component `{part}` in `{raw}`: {err}"))
        })
        .collect::<Result<Vec<_>>>()?;
    if parts.is_empty() || parts.len() > 3 {
        return Err(anyhow!("version `{raw}` must have one to three numeric components"));
    }
    Ok(VersionRequirement::Prefix { parts })
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

fn normalize_version(version: &str) -> String {
    let count = version.split('.').count();
    match count {
        1 => format!("{version}.0.0"),
        2 => format!("{version}.0"),
        _ => version.to_string(),
    }
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

fn load_package_policy(project_dir: &Path) -> Result<PackagePolicyConfig> {
    let config_path = project_dir.join("corvid.toml");
    let Some(config) = CorvidConfig::load_from_path(&config_path)
        .map_err(|err| anyhow!("failed to load `{}`: {err}", config_path.display()))?
    else {
        return Ok(PackagePolicyConfig::default());
    };
    Ok(config.package_policy)
}

fn package_policy_violation(
    summary: &ModuleSemanticSummary,
    policy: &PackagePolicyConfig,
) -> Option<String> {
    if !policy.allow_approval_required {
        if let Some(export) = summary
            .exports
            .values()
            .find(|export| export.approval_required)
        {
            return Some(format!(
                "package export `{}` requires approval, but package-policy.allow-approval-required=false",
                export.name
            ));
        }
    }
    if !policy.allow_effect_violations {
        if let Some(agent) = summary.agents.values().find(|agent| !agent.violations.is_empty()) {
            return Some(format!(
                "package agent `{}` has effect violations, but package-policy.allow-effect-violations=false",
                agent.name
            ));
        }
    }
    if policy.require_deterministic {
        if let Some(agent) = summary.agents.values().find(|agent| !agent.deterministic) {
            return Some(format!(
                "package agent `{}` is not @deterministic, but package-policy.require-deterministic=true",
                agent.name
            ));
        }
    }
    if policy.require_replayable {
        if let Some(agent) = summary.agents.values().find(|agent| !agent.replayable) {
            return Some(format!(
                "package agent `{}` is not @replayable, but package-policy.require-replayable=true",
                agent.name
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
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
signature = \"unsigned:test-fixture\"
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

        assert!(matches!(
            outcome,
            AddPackageOutcome::Added { ref uri, exports: 1, .. }
                if uri == "corvid://@anthropic/safety-baseline/v2.3.4"
        ));
        let lock = fs::read_to_string(tmp.path().join("Corvid.lock")).unwrap();
        assert!(lock.contains("semantic_summary"));
        assert!(lock.contains("SafetyReceipt"));
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
}
