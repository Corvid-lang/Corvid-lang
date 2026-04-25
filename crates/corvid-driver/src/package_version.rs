//! Shared package-name and version-requirement parsing.
//!
//! `corvid add`, `corvid update`, registry verification, and lock graph
//! validation must agree on what a package name and version requirement mean.

use anyhow::{anyhow, Result};
use semver::{Version, VersionReq};

#[derive(Debug, Clone)]
pub(crate) struct PackageSpec {
    pub name: String,
    pub raw_requirement: String,
    pub requirement: VersionRequirement,
}

#[derive(Debug, Clone)]
pub(crate) enum VersionRequirement {
    Prefix { parts: Vec<u64> },
    Semver(VersionReq),
}

impl VersionRequirement {
    pub(crate) fn matches(&self, version: &Version) -> bool {
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

pub(crate) fn parse_package_spec(spec: &str) -> Result<PackageSpec> {
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
        raw_requirement: version.to_string(),
        requirement: parse_version_requirement(version)?,
    })
}

pub(crate) fn parse_version_requirement(raw: &str) -> Result<VersionRequirement> {
    if raw
        .chars()
        .next()
        .is_some_and(|ch| matches!(ch, '^' | '~' | '>' | '<' | '=' | '*'))
    {
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
        return Err(anyhow!(
            "version `{raw}` must have one to three numeric components"
        ));
    }
    Ok(VersionRequirement::Prefix { parts })
}

pub(crate) fn normalize_version(version: &str) -> String {
    let count = version.split('.').count();
    match count {
        1 => format!("{version}.0.0"),
        2 => format!("{version}.0"),
        _ => version.to_string(),
    }
}

pub(crate) fn validate_package_name(name: &str) -> Result<()> {
    if name.starts_with('@') && name.contains('/') && !name.ends_with('/') {
        Ok(())
    } else {
        Err(anyhow!("package name must be scoped, e.g. `@scope/name`"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_spec_uses_last_at_for_scoped_name() {
        let spec = parse_package_spec("@anthropic/safety-baseline@2.3").unwrap();
        assert_eq!(spec.name, "@anthropic/safety-baseline");
        assert_eq!(spec.raw_requirement, "2.3");
        match spec.requirement {
            VersionRequirement::Prefix { ref parts } if parts == &[2, 3] => {}
            other => panic!("unexpected requirement: {other:?}"),
        }
    }

    #[test]
    fn semver_requirement_matches_version() {
        let req = parse_version_requirement("^2.3.0").unwrap();
        assert!(req.matches(&Version::parse("2.4.0").unwrap()));
        assert!(!req.matches(&Version::parse("3.0.0").unwrap()));
    }
}
