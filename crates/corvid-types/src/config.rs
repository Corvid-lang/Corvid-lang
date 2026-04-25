//! `corvid.toml` parser and dimension-table builder.
//!
//! Users declare custom effect-system dimensions in `corvid.toml`:
//!
//! ```toml
//! [effect-system.dimensions.freshness]
//! composition = "Max"
//! type = "timestamp"
//! default = "0"
//! semantics = "max age of data in a call chain"
//! ```
//!
//! The compiler reads the file at check-time and merges the declared
//! dimensions into the `EffectRegistry` alongside the built-ins. Custom
//! dimensions that collide with a built-in name are rejected — users
//! cannot redefine `cost`, `trust`, `reversible`, `data`, `latency`, or
//! `confidence`.

use corvid_ast::{CompositionRule, DimensionSchema, DimensionValue};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The parsed shape of `corvid.toml`. Unknown top-level keys (e.g.
/// `[llm]`, `[build]`) are tolerated — this struct only claims the
/// `[effect-system]` section. Future slices can flesh out the rest.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct CorvidConfig {
    #[serde(rename = "effect-system")]
    pub effect_system: EffectSystemConfig,
    #[serde(rename = "package-policy")]
    pub package_policy: PackagePolicyConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct PackagePolicyConfig {
    /// When false, `corvid add` rejects packages whose public exports
    /// require human/supervisor approval.
    pub allow_approval_required: bool,
    /// When false, `corvid add` rejects packages whose exported agents
    /// already have effect-constraint violations in their own module.
    pub allow_effect_violations: bool,
    /// When true, every exported agent must be marked `@deterministic`.
    pub require_deterministic: bool,
    /// When true, every exported agent must be marked `@replayable`.
    pub require_replayable: bool,
    /// When true, `corvid add` rejects unsigned package index entries.
    pub require_package_signatures: bool,
}

impl Default for PackagePolicyConfig {
    fn default() -> Self {
        Self {
            allow_approval_required: true,
            allow_effect_violations: true,
            require_deterministic: false,
            require_replayable: false,
            require_package_signatures: false,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct EffectSystemConfig {
    /// Map from dimension name → its declared composition rule, value
    /// type, default, and optional proof pointer. BTreeMap for stable
    /// iteration order in error messages.
    pub dimensions: BTreeMap<String, CustomDimensionConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CustomDimensionConfig {
    /// One of "Sum", "Max", "Min", "Union", "LeastReversible".
    pub composition: String,
    /// One of "bool", "name", "cost", "number", "timestamp".
    /// (timestamp is treated as Number at the type level.)
    #[serde(rename = "type", default = "default_type")]
    pub ty: String,
    /// Identity element for the composition rule, as a string. Parsed
    /// per the declared type.
    #[serde(default)]
    pub default: Option<String>,
    /// Human-readable semantics, emitted in error messages when the
    /// dimension's constraint fails.
    #[serde(default)]
    pub semantics: Option<String>,
    /// Path (relative to corvid.toml) to a machine-checkable proof of
    /// the composition rule's algebraic laws. Unused at load time —
    /// consumed by `corvid test dimensions` when it replays proofs.
    #[serde(default)]
    pub proof: Option<String>,
}

fn default_type() -> String {
    "number".into()
}

/// Error produced when loading or validating `corvid.toml` dimensions.
#[derive(Debug, Clone, PartialEq)]
pub enum DimensionConfigError {
    /// Failed to parse the TOML file.
    ParseError {
        path: PathBuf,
        message: String,
    },
    /// Composition rule string wasn't one of the five archetypes.
    UnknownComposition {
        dimension: String,
        got: String,
    },
    /// Type string wasn't one of the supported value kinds.
    UnknownType {
        dimension: String,
        got: String,
    },
    /// Default value couldn't be parsed according to the declared type.
    BadDefault {
        dimension: String,
        ty: String,
        got: String,
    },
    /// User declared a dimension whose name collides with a built-in.
    /// The built-ins (`cost`, `trust`, `reversible`, `data`, `latency`,
    /// `confidence`, plus the streaming helpers `tokens` and
    /// `latency_ms`) are owned by the compiler; users cannot redefine
    /// them.
    CollidesWithBuiltin { dimension: String },
}

impl std::fmt::Display for DimensionConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ParseError { path, message } => {
                write!(f, "parse error in {}: {}", path.display(), message)
            }
            Self::UnknownComposition { dimension, got } => write!(
                f,
                "dimension `{dimension}`: composition `{got}` is not one of \
                 Sum, Max, Min, Union, LeastReversible"
            ),
            Self::UnknownType { dimension, got } => write!(
                f,
                "dimension `{dimension}`: type `{got}` is not one of \
                 bool, name, cost, number, timestamp"
            ),
            Self::BadDefault { dimension, ty, got } => write!(
                f,
                "dimension `{dimension}`: default `{got}` cannot be parsed as {ty}"
            ),
            Self::CollidesWithBuiltin { dimension } => write!(
                f,
                "dimension `{dimension}` collides with a built-in dimension name; \
                 rename your dimension or remove it from corvid.toml"
            ),
        }
    }
}

impl std::error::Error for DimensionConfigError {}

/// Names the compiler owns. Users cannot redefine these in
/// `corvid.toml` — they're reserved for the built-in semantics.
pub const BUILTIN_DIMENSION_NAMES: &[&str] = &[
    "cost",
    "trust",
    "reversible",
    "data",
    "latency",
    "latency_ms",
    "confidence",
    "tokens",
];

impl CorvidConfig {
    /// Load `corvid.toml` from an explicit path. Returns `Ok(None)` if
    /// the file doesn't exist; `Err` if it exists but fails to parse.
    pub fn load_from_path(path: &Path) -> Result<Option<Self>, DimensionConfigError> {
        let bytes = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(DimensionConfigError::ParseError {
                    path: path.to_path_buf(),
                    message: e.to_string(),
                })
            }
        };
        toml::from_str::<Self>(&bytes)
            .map(Some)
            .map_err(|e| DimensionConfigError::ParseError {
                path: path.to_path_buf(),
                message: e.to_string(),
            })
    }

    /// Walk from `start` upward looking for `corvid.toml`. Mirrors the
    /// `.env` discovery pattern so embedded hosts (CLI, REPL, LSP) all
    /// agree on which file configures the current project.
    pub fn load_walking(start: &Path) -> Result<Option<Self>, DimensionConfigError> {
        let mut cur: Option<&Path> = Some(start);
        while let Some(dir) = cur {
            let candidate = dir.join("corvid.toml");
            if candidate.exists() {
                return Self::load_from_path(&candidate);
            }
            cur = dir.parent();
        }
        Ok(None)
    }

    /// Translate the TOML-declared dimensions into `DimensionSchema`
    /// entries ready to merge into the `EffectRegistry`. Each entry is
    /// validated:
    /// - composition must be one of the five archetypes
    /// - type must be one of the six value kinds
    /// - default must parse against the declared type
    /// - name must not collide with a built-in
    pub fn into_dimension_schemas(
        &self,
    ) -> Result<Vec<(DimensionSchema, CustomDimensionMeta)>, DimensionConfigError> {
        let mut out = Vec::new();
        for (name, cfg) in &self.effect_system.dimensions {
            if BUILTIN_DIMENSION_NAMES.contains(&name.as_str()) {
                return Err(DimensionConfigError::CollidesWithBuiltin {
                    dimension: name.clone(),
                });
            }
            let rule = parse_composition_rule(name, &cfg.composition)?;
            let ty = parse_dimension_type(name, &cfg.ty)?;
            let default = parse_default(name, &ty, cfg.default.as_deref(), rule)?;
            let schema = DimensionSchema {
                name: name.clone(),
                composition: rule,
                default,
            };
            let meta = CustomDimensionMeta {
                name: name.clone(),
                ty,
                semantics: cfg.semantics.clone(),
                proof_path: cfg.proof.clone(),
            };
            out.push((schema, meta));
        }
        Ok(out)
    }
}

/// Metadata about a custom dimension that doesn't fit in
/// `DimensionSchema` — preserved for error-message rendering and for
/// `corvid test dimensions` to drive the archetype's law-check
/// proptest.
#[derive(Debug, Clone)]
pub struct CustomDimensionMeta {
    pub name: String,
    pub ty: DimensionValueType,
    pub semantics: Option<String>,
    pub proof_path: Option<String>,
}

/// The six value kinds a dimension can inhabit. `Timestamp` is a
/// `Number` at the DimensionValue level but retained here for error
/// messages and future tooling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DimensionValueType {
    Bool,
    Name,
    Cost,
    Number,
    Timestamp,
}

impl DimensionValueType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Name => "name",
            Self::Cost => "cost",
            Self::Number => "number",
            Self::Timestamp => "timestamp",
        }
    }
}

fn parse_composition_rule(
    dimension: &str,
    raw: &str,
) -> Result<CompositionRule, DimensionConfigError> {
    match raw {
        "Sum" | "sum" => Ok(CompositionRule::Sum),
        "Max" | "max" => Ok(CompositionRule::Max),
        "Min" | "min" => Ok(CompositionRule::Min),
        "Union" | "union" => Ok(CompositionRule::Union),
        "LeastReversible" | "least-reversible" | "least_reversible" => {
            Ok(CompositionRule::LeastReversible)
        }
        other => Err(DimensionConfigError::UnknownComposition {
            dimension: dimension.to_string(),
            got: other.to_string(),
        }),
    }
}

fn parse_dimension_type(
    dimension: &str,
    raw: &str,
) -> Result<DimensionValueType, DimensionConfigError> {
    match raw {
        "bool" => Ok(DimensionValueType::Bool),
        "name" => Ok(DimensionValueType::Name),
        "cost" | "money" => Ok(DimensionValueType::Cost),
        "number" | "float" | "f64" => Ok(DimensionValueType::Number),
        "timestamp" => Ok(DimensionValueType::Timestamp),
        other => Err(DimensionConfigError::UnknownType {
            dimension: dimension.to_string(),
            got: other.to_string(),
        }),
    }
}

fn parse_default(
    dimension: &str,
    ty: &DimensionValueType,
    raw: Option<&str>,
    rule: CompositionRule,
) -> Result<DimensionValue, DimensionConfigError> {
    match (ty, raw) {
        (DimensionValueType::Bool, Some("true")) => Ok(DimensionValue::Bool(true)),
        (DimensionValueType::Bool, Some("false")) => Ok(DimensionValue::Bool(false)),
        (DimensionValueType::Bool, Some(got)) => Err(DimensionConfigError::BadDefault {
            dimension: dimension.to_string(),
            ty: "bool".into(),
            got: got.to_string(),
        }),
        (DimensionValueType::Bool, None) => Ok(DimensionValue::Bool(true)),
        (DimensionValueType::Name, Some(v)) => Ok(DimensionValue::Name(v.to_string())),
        (DimensionValueType::Name, None) => Ok(DimensionValue::Name("none".into())),
        (DimensionValueType::Cost, Some(v)) => v
            .trim_start_matches('$')
            .parse::<f64>()
            .map(DimensionValue::Cost)
            .map_err(|_| DimensionConfigError::BadDefault {
                dimension: dimension.to_string(),
                ty: "cost".into(),
                got: v.to_string(),
            }),
        (DimensionValueType::Cost, None) => Ok(DimensionValue::Cost(0.0)),
        (DimensionValueType::Number | DimensionValueType::Timestamp, Some(v)) => v
            .parse::<f64>()
            .map(DimensionValue::Number)
            .map_err(|_| DimensionConfigError::BadDefault {
                dimension: dimension.to_string(),
                ty: ty.as_str().to_string(),
                got: v.to_string(),
            }),
        (DimensionValueType::Number | DimensionValueType::Timestamp, None) => match rule {
            CompositionRule::Min => Ok(DimensionValue::Number(f64::INFINITY)),
            _ => Ok(DimensionValue::Number(0.0)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_minimal_effect_system_section() {
        let toml = r#"
            [effect-system.dimensions.freshness]
            composition = "Max"
            type = "timestamp"
            default = "0"
        "#;
        let cfg: CorvidConfig = toml::from_str(toml).unwrap();
        let schemas = cfg.into_dimension_schemas().unwrap();
        assert_eq!(schemas.len(), 1);
        let (schema, meta) = &schemas[0];
        assert_eq!(schema.name, "freshness");
        assert_eq!(schema.composition, CompositionRule::Max);
        assert_eq!(meta.ty, DimensionValueType::Timestamp);
    }

    #[test]
    fn loads_multiple_dimensions_stable_order() {
        let toml = r#"
            [effect-system.dimensions.fairness]
            composition = "Max"
            type = "number"
            default = "0.0"

            [effect-system.dimensions.carbon]
            composition = "Sum"
            type = "number"
            default = "0.0"
        "#;
        let cfg: CorvidConfig = toml::from_str(toml).unwrap();
        let schemas = cfg.into_dimension_schemas().unwrap();
        let names: Vec<&str> = schemas.iter().map(|(s, _)| s.name.as_str()).collect();
        // BTreeMap gives alphabetical order — carbon before fairness.
        assert_eq!(names, vec!["carbon", "fairness"]);
    }

    #[test]
    fn unknown_composition_is_rejected_with_helpful_message() {
        let toml = r#"
            [effect-system.dimensions.freshness]
            composition = "Product"
            type = "number"
        "#;
        let cfg: CorvidConfig = toml::from_str(toml).unwrap();
        let err = cfg.into_dimension_schemas().unwrap_err();
        match err {
            DimensionConfigError::UnknownComposition { dimension, got } => {
                assert_eq!(dimension, "freshness");
                assert_eq!(got, "Product");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn collision_with_builtin_is_rejected() {
        let toml = r#"
            [effect-system.dimensions.cost]
            composition = "Sum"
            type = "cost"
        "#;
        let cfg: CorvidConfig = toml::from_str(toml).unwrap();
        let err = cfg.into_dimension_schemas().unwrap_err();
        assert!(matches!(
            err,
            DimensionConfigError::CollidesWithBuiltin { dimension } if dimension == "cost"
        ));
    }

    #[test]
    fn each_builtin_name_is_rejected() {
        for name in BUILTIN_DIMENSION_NAMES {
            let toml = format!(
                r#"
                [effect-system.dimensions.{name}]
                composition = "Max"
                type = "number"
            "#
            );
            let cfg: CorvidConfig = toml::from_str(&toml).unwrap();
            let err = cfg.into_dimension_schemas().unwrap_err();
            assert!(
                matches!(err, DimensionConfigError::CollidesWithBuiltin { .. }),
                "`{name}` was not rejected as a built-in collision"
            );
        }
    }

    #[test]
    fn bad_default_is_rejected_with_type_context() {
        let toml = r#"
            [effect-system.dimensions.carbon]
            composition = "Sum"
            type = "number"
            default = "not-a-number"
        "#;
        let cfg: CorvidConfig = toml::from_str(toml).unwrap();
        let err = cfg.into_dimension_schemas().unwrap_err();
        match err {
            DimensionConfigError::BadDefault { dimension, ty, got } => {
                assert_eq!(dimension, "carbon");
                assert_eq!(ty, "number");
                assert_eq!(got, "not-a-number");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn missing_default_is_filled_from_archetype_identity() {
        let toml = r#"
            [effect-system.dimensions.confidence_user]
            composition = "Min"
            type = "number"
        "#;
        let cfg: CorvidConfig = toml::from_str(toml).unwrap();
        let schemas = cfg.into_dimension_schemas().unwrap();
        let (schema, _) = &schemas[0];
        // Min's identity is +∞ so the running min starts there and any
        // real value lowers it. Verify the identity was filled in.
        match schema.default {
            DimensionValue::Number(n) => assert!(n.is_infinite()),
            ref other => panic!("unexpected default: {other:?}"),
        }
    }

    #[test]
    fn unknown_top_level_sections_are_tolerated() {
        let toml = r#"
            name = "my-project"
            version = "0.1.0"

            [llm]
            default_model = "claude-opus-4-6"

            [build]
            target = "native"

            [effect-system.dimensions.freshness]
            composition = "Max"
            type = "timestamp"
            default = "0"
        "#;
        let cfg: CorvidConfig = toml::from_str(toml).unwrap();
        let schemas = cfg.into_dimension_schemas().unwrap();
        assert_eq!(schemas.len(), 1);
    }

    #[test]
    fn missing_file_returns_none() {
        let tmp = std::env::temp_dir().join("corvid-test-missing");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let result = CorvidConfig::load_walking(&tmp).unwrap();
        assert!(result.is_none());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn walking_finds_config_in_parent() {
        let tmp = std::env::temp_dir().join("corvid-test-walking");
        let _ = std::fs::remove_dir_all(&tmp);
        let nested = tmp.join("a").join("b").join("c");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            tmp.join("corvid.toml"),
            r#"
[effect-system.dimensions.freshness]
composition = "Max"
type = "timestamp"
default = "0"
"#,
        )
        .unwrap();
        let cfg = CorvidConfig::load_walking(&nested).unwrap().unwrap();
        assert_eq!(cfg.effect_system.dimensions.len(), 1);
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
