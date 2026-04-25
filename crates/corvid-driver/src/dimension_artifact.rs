//! Signed custom-dimension artifact verification.
//!
//! A plain local dimension TOML remains valid for development. A
//! distributable artifact adds an `[artifact]` table, an Ed25519
//! signature over a canonical payload, and optional regression programs
//! that must still compile or fail as declared under the dimension.

use crate::compile_with_config;
use anyhow::{anyhow, Context, Result};
use corvid_types::CorvidConfig;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use semver::Version;
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct DimensionArtifactReport {
    pub name: String,
    pub version: Version,
    pub regression_count: usize,
}

#[derive(Debug, Deserialize)]
struct ArtifactFile {
    artifact: Option<ArtifactHeader>,
    #[serde(default)]
    regression: Vec<RegressionCase>,
}

#[derive(Debug, Deserialize)]
struct ArtifactHeader {
    name: String,
    version: String,
    signing_key: String,
    signature: String,
}

#[derive(Debug, Deserialize, Clone)]
struct RegressionCase {
    name: String,
    source: String,
    expect: RegressionExpectation,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum RegressionExpectation {
    Compile,
    Error,
}

pub fn verify_dimension_artifact(source: &str) -> Result<Option<DimensionArtifactReport>> {
    let artifact_file: ArtifactFile =
        toml::from_str(source).context("failed to parse dimension artifact wrapper")?;
    let Some(header) = artifact_file.artifact else {
        return Ok(None);
    };
    let version = Version::parse(&header.version)
        .with_context(|| format!("dimension artifact `{}` has invalid semver", header.name))?;
    let config: CorvidConfig =
        toml::from_str(source).context("failed to parse dimension artifact declaration")?;
    let schemas = config
        .into_dimension_schemas()
        .map_err(|err| anyhow!("dimension artifact declaration is invalid: {err}"))?;
    if schemas.len() != 1 {
        return Err(anyhow!(
            "dimension artifact `{}` must contain exactly one dimension declaration; got {}",
            header.name,
            schemas.len()
        ));
    }
    let (schema, meta) = &schemas[0];
    if schema.name != header.name {
        return Err(anyhow!(
            "dimension artifact header names `{}` but declaration names `{}`",
            header.name,
            schema.name
        ));
    }
    verify_signature(&header, schema, meta, &artifact_file.regression)?;
    run_regressions(&config, &artifact_file.regression)?;
    Ok(Some(DimensionArtifactReport {
        name: header.name,
        version,
        regression_count: artifact_file.regression.len(),
    }))
}

pub fn canonical_payload_for_artifact(source: &str) -> Result<String> {
    let artifact_file: ArtifactFile =
        toml::from_str(source).context("failed to parse dimension artifact wrapper")?;
    let header = artifact_file
        .artifact
        .ok_or_else(|| anyhow!("missing [artifact] table"))?;
    let config: CorvidConfig =
        toml::from_str(source).context("failed to parse dimension artifact declaration")?;
    let schemas = config
        .into_dimension_schemas()
        .map_err(|err| anyhow!("dimension artifact declaration is invalid: {err}"))?;
    if schemas.len() != 1 {
        return Err(anyhow!("dimension artifact must contain exactly one dimension"));
    }
    let (schema, meta) = &schemas[0];
    Ok(canonical_payload(&header, schema, meta, &artifact_file.regression))
}

fn verify_signature(
    header: &ArtifactHeader,
    schema: &corvid_ast::DimensionSchema,
    meta: &corvid_types::CustomDimensionMeta,
    regressions: &[RegressionCase],
) -> Result<()> {
    let key = decode_hex_exact::<32>(&header.signing_key)
        .with_context(|| format!("artifact `{}` has invalid signing_key", header.name))?;
    let sig = decode_hex_exact::<64>(&header.signature)
        .with_context(|| format!("artifact `{}` has invalid signature", header.name))?;
    let verifying_key = VerifyingKey::from_bytes(&key)
        .with_context(|| format!("artifact `{}` signing_key is not an Ed25519 key", header.name))?;
    let signature = Signature::from_bytes(&sig);
    let payload = canonical_payload(header, schema, meta, regressions);
    verifying_key
        .verify(payload.as_bytes(), &signature)
        .with_context(|| format!("artifact `{}` signature verification failed", header.name))
}

fn run_regressions(config: &CorvidConfig, regressions: &[RegressionCase]) -> Result<()> {
    for case in regressions {
        let result = compile_with_config(&case.source, Some(config));
        match case.expect {
            RegressionExpectation::Compile if !result.ok() => {
                return Err(anyhow!(
                    "dimension artifact regression `{}` expected compile, got {} error(s)",
                    case.name,
                    result.diagnostics.len()
                ))
            }
            RegressionExpectation::Error if result.ok() => {
                return Err(anyhow!(
                    "dimension artifact regression `{}` expected error, but compiled cleanly",
                    case.name
                ))
            }
            _ => {}
        }
    }
    Ok(())
}

fn canonical_payload(
    header: &ArtifactHeader,
    schema: &corvid_ast::DimensionSchema,
    meta: &corvid_types::CustomDimensionMeta,
    regressions: &[RegressionCase],
) -> String {
    let mut out = String::new();
    out.push_str("corvid-dimension-artifact-v1\n");
    out.push_str(&format!("name={}\n", header.name));
    out.push_str(&format!("version={}\n", header.version));
    out.push_str(&format!("dimension={}\n", schema.name));
    out.push_str(&format!("composition={:?}\n", schema.composition));
    out.push_str(&format!("type={}\n", meta.ty.as_str()));
    out.push_str(&format!("default={}\n", dimension_value(&schema.default)));
    out.push_str(&format!(
        "semantics={}\n",
        meta.semantics.as_deref().unwrap_or("")
    ));
    out.push_str(&format!("proof={}\n", meta.proof_path.as_deref().unwrap_or("")));
    for case in regressions {
        out.push_str(&format!("regression.name={}\n", case.name));
        out.push_str(&format!("regression.expect={:?}\n", case.expect));
        out.push_str("regression.source<<\n");
        out.push_str(&case.source);
        if !case.source.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(">>\n");
    }
    out
}

fn dimension_value(value: &corvid_ast::DimensionValue) -> String {
    match value {
        corvid_ast::DimensionValue::Bool(v) => v.to_string(),
        corvid_ast::DimensionValue::Name(v) => v.clone(),
        corvid_ast::DimensionValue::Cost(v) => v.to_string(),
        corvid_ast::DimensionValue::Number(v) => v.to_string(),
        corvid_ast::DimensionValue::Streaming { backpressure } => backpressure.label().to_string(),
        corvid_ast::DimensionValue::ConfidenceGated {
            threshold,
            above,
            below,
        } => format!("autonomous_if_confident({threshold}, {above}, {below})"),
    }
}

fn decode_hex_exact<const N: usize>(input: &str) -> Result<[u8; N]> {
    let trimmed = input.trim();
    if trimmed.len() != N * 2 {
        return Err(anyhow!("expected {} hex chars, got {}", N * 2, trimmed.len()));
    }
    let mut out = [0u8; N];
    for idx in 0..N {
        let pair = &trimmed[idx * 2..idx * 2 + 2];
        out[idx] = u8::from_str_radix(pair, 16)
            .with_context(|| format!("invalid hex byte `{pair}`"))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn signed_dimension_artifact_verifies_signature_and_regressions() {
        let unsigned = artifact_with_signature("", "");
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let verifying_key = signing_key.verifying_key();
        let with_key = artifact_with_signature(&hex(verifying_key.as_bytes()), "");
        let payload = canonical_payload_for_artifact(&with_key).unwrap();
        let signature = signing_key.sign(payload.as_bytes());
        let signed = artifact_with_signature(&hex(verifying_key.as_bytes()), &hex(&signature.to_bytes()));
        let report = verify_dimension_artifact(&signed).unwrap().unwrap();
        assert_eq!(report.name, "freshness");
        assert_eq!(report.regression_count, 2);
        assert!(verify_dimension_artifact(&unsigned).is_err());
    }

    #[test]
    fn signed_dimension_artifact_rejects_tampered_payload() {
        let signing_key = SigningKey::from_bytes(&[9u8; 32]);
        let verifying_key = signing_key.verifying_key();
        let with_key = artifact_with_signature(&hex(verifying_key.as_bytes()), "");
        let payload = canonical_payload_for_artifact(&with_key).unwrap();
        let signature = signing_key.sign(payload.as_bytes());
        let signed = artifact_with_signature(&hex(verifying_key.as_bytes()), &hex(&signature.to_bytes()));
        let tampered = signed.replace("maximum data age", "tampered meaning");
        let err = verify_dimension_artifact(&tampered).unwrap_err();
        assert!(err.to_string().contains("signature verification failed"), "{err}");
    }

    fn artifact_with_signature(signing_key: &str, signature: &str) -> String {
        format!(
            r#"
[artifact]
name = "freshness"
version = "1.0.0"
signing_key = "{signing_key}"
signature = "{signature}"

[effect-system.dimensions.freshness]
composition = "Max"
type = "timestamp"
default = "0"
semantics = "maximum data age"

[[regression]]
name = "freshness_compiles"
expect = "compile"
source = '''
effect stale:
    freshness: 2

tool read_cache() -> String uses stale

agent main() -> String:
    return read_cache()
'''

[[regression]]
name = "freshness_unknown_effect_errors"
expect = "error"
source = '''
tool read_cache() -> String uses missing_freshness

agent main() -> String:
    return read_cache()
'''
"#
        )
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}
