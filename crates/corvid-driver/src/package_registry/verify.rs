//! Registry contract verification.

use std::io::Read;

use anyhow::{anyhow, Context, Result};
use semver::Version;

use crate::import_integrity::sha256_hex;
use crate::modules::summarize_module_source;
use crate::package_version::{normalize_version, validate_package_name};

use super::{
    load_registry_index, verify_package_signature, RegistryPackage, RegistryVerificationFailure,
    RegistryVerificationReport,
};

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
                report
                    .failures
                    .push(failure(package, format!("artifact is not UTF-8: {err}")));
                continue;
            }
        };
        let summary = match summarize_module_source(&source) {
            Ok(summary) => summary,
            Err(err) => {
                report
                    .failures
                    .push(failure(package, format!("semantic summary failed: {err}")));
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
        return Err(
            "artifact URL must be immutable: query strings and fragments are forbidden".to_string(),
        );
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
    let response = ureq::get(location)
        .call()
        .map_err(|err| anyhow!(err.to_string()))?;
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
