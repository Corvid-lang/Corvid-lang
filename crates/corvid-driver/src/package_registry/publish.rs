//! Package publishing into a registry index.

use anyhow::{anyhow, Context, Result};
use semver::Version;

use crate::import_integrity::sha256_hex;
use crate::modules::summarize_module_source;
use crate::package_version::{normalize_version, validate_package_name};

use super::{
    sign_package, PublishPackageOptions, PublishPackageOutcome, RegistryIndex, RegistryPackage,
};

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
    package.signature = Some(sign_package(
        &package,
        options.signing_seed_hex,
        options.key_id,
    )?);

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
