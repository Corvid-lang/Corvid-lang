use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use corvid_abi::{load_signing_key, sign_envelope, KeySource};
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Serialize)]
struct ReleaseManifest<'a> {
    schema: &'a str,
    channel: &'a str,
    version: &'a str,
    stability: &'a str,
    binary: String,
    binary_sha256: String,
    checksum_file: &'a str,
    changelog: &'a str,
    policy: &'a str,
}

pub fn run_release(channel: &str, version: Option<&str>, out: &Path) -> Result<()> {
    let normalized = normalize_channel(channel)?;
    let version = version
        .map(str::to_string)
        .unwrap_or_else(|| default_version(normalized));
    validate_version(normalized, &version)?;

    fs::create_dir_all(out)
        .with_context(|| format!("create release output dir `{}`", out.display()))?;

    let binary_path = copy_current_binary(normalized, &version, out)?;
    let binary_bytes = fs::read(&binary_path)
        .with_context(|| format!("read release binary `{}`", binary_path.display()))?;
    let binary_sha256 = hex::encode(Sha256::digest(&binary_bytes));

    let changelog_name = "CHANGELOG.md";
    let changelog = render_changelog(normalized, &version);
    fs::write(out.join(changelog_name), &changelog).context("write release changelog")?;

    let binary_name = file_name(&binary_path)?;
    let checksums = format!("{binary_sha256}  {binary_name}\n");
    fs::write(out.join("SHA256SUMS.txt"), checksums).context("write release checksums")?;

    let manifest = ReleaseManifest {
        schema: "corvid.release.manifest.v1",
        channel: normalized,
        version: &version,
        stability: stability_for(normalized),
        binary: binary_name,
        binary_sha256,
        checksum_file: "SHA256SUMS.txt",
        changelog: changelog_name,
        policy: "docs/release-policy.md",
    };
    let manifest_json =
        serde_json::to_string_pretty(&manifest).context("serialize release manifest")?;
    fs::write(out.join("release-manifest.json"), &manifest_json)
        .context("write release manifest")?;

    let attestation = sign_release_manifest(&manifest_json)?;
    fs::write(out.join("release-attestation.dsse.json"), attestation)
        .context("write release attestation")?;

    println!("release channel: {normalized}");
    println!("version: {version}");
    println!("binary: {}", binary_path.display());
    println!("checksums: {}", out.join("SHA256SUMS.txt").display());
    println!("manifest: {}", out.join("release-manifest.json").display());
    println!(
        "attestation: {}",
        out.join("release-attestation.dsse.json").display()
    );
    Ok(())
}

fn normalize_channel(channel: &str) -> Result<&'static str> {
    match channel {
        "nightly" => Ok("nightly"),
        "beta" => Ok("beta"),
        "stable" => Ok("stable"),
        other => bail!("unknown release channel `{other}`; expected nightly, beta, or stable"),
    }
}

fn default_version(channel: &str) -> String {
    match channel {
        "nightly" => "0.0.0-nightly.local".to_string(),
        "beta" => "1.0.0-beta.1".to_string(),
        "stable" => env!("CARGO_PKG_VERSION").to_string(),
        _ => unreachable!("channel normalized before default version"),
    }
}

fn validate_version(channel: &str, version: &str) -> Result<()> {
    match channel {
        "nightly" if version.contains("-nightly.") => Ok(()),
        "beta" if version.contains("-beta.") => Ok(()),
        "stable" if !version.contains('-') && version.split('.').count() == 3 => Ok(()),
        "nightly" => bail!("nightly versions must contain `-nightly.`"),
        "beta" => bail!("beta versions must contain `-beta.`"),
        "stable" => bail!("stable versions must be plain MAJOR.MINOR.PATCH"),
        _ => unreachable!("channel normalized before version validation"),
    }
}

fn copy_current_binary(channel: &str, version: &str, out: &Path) -> Result<PathBuf> {
    let current = std::env::current_exe().context("locate current corvid binary")?;
    let ext = std::env::consts::EXE_EXTENSION;
    let suffix = if ext.is_empty() {
        String::new()
    } else {
        format!(".{ext}")
    };
    let target = out.join(format!(
        "corvid-{channel}-{version}-{}-{}{}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        suffix
    ));
    fs::copy(&current, &target).with_context(|| {
        format!(
            "copy release binary `{}` to `{}`",
            current.display(),
            target.display()
        )
    })?;
    Ok(target)
}

fn file_name(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .with_context(|| format!("release path `{}` has no file name", path.display()))
}

fn render_changelog(channel: &str, version: &str) -> String {
    format!(
        r#"# Corvid {version}

Channel: {channel}

## Required Verification

- Verify `SHA256SUMS.txt` against the release binary.
- Verify `release-attestation.dsse.json` with the release public key.
- Run `corvid claim audit` before promoting this release.
- Run `corvid upgrade --check` for applications moving from the previous channel baseline.

## Compatibility

This release follows `docs/release-policy.md`. Any breaking change must appear in the upgrade report and migration guide.
"#
    )
}

fn stability_for(channel: &str) -> &'static str {
    match channel {
        "nightly" => "nightly-no-compatibility-promise",
        "beta" => "beta-train-compatible-with-migration-notes",
        "stable" => "semver-stable",
        _ => unreachable!("channel normalized before stability lookup"),
    }
}

fn sign_release_manifest(manifest_json: &str) -> Result<String> {
    let signing_key = std::env::var("CORVID_RELEASE_SIGNING_KEY")
        .context("CORVID_RELEASE_SIGNING_KEY is required for release attestation")?;
    let key = load_signing_key(&KeySource::Env(signing_key))
        .map_err(|err| anyhow::anyhow!("load release signing key: {err}"))?;
    let envelope = sign_envelope(
        manifest_json.as_bytes(),
        "application/vnd.corvid.release.manifest.v1+json",
        &key,
        "release-channel",
    );
    serde_json::to_string_pretty(&envelope).context("serialize release attestation")
}
