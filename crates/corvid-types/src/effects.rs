//! Dimensional effect composition algebra and call-graph effect analyzer.
//!
//! Each effect declaration carries typed dimensions (cost, trust,
//! reversibility, data, latency). When effects compose through a call
//! graph, each dimension composes independently via its declared rule:
//!
//!   cost        → Sum (accumulates along the call path)
//!   trust       → Max (most restrictive wins)
//!   reversible  → LeastReversible (one irreversible call makes the chain irreversible)
//!   data        → Union (all data classifications accumulate)
//!   latency     → Max (slowest call determines the chain's latency class)
//!   confidence  → Min (least confident result determines the chain)
//!
//! The analyzer walks the call graph from each agent, collects the
//! effects of every tool/prompt/agent call in its body, and computes
//! the composed dimensional profile. Constraints on the agent (e.g.,
//! `@budget($1.00)`) are then checked against the composed profile.

use corvid_ast::{
    BackpressurePolicy, CompositionRule, DimensionSchema, DimensionValue, Effect,
    EffectConstraint, EffectDecl,
};
use corvid_resolve::DefId;
use std::collections::{HashMap, HashSet};

use crate::config::{CorvidConfig, CustomDimensionMeta};

/// Registry of declared effect dimensions and their composition rules.
/// Built from the file's `effect` declarations, plus any custom
/// dimensions declared in `corvid.toml` under
/// `[effect-system.dimensions.*]`.
#[derive(Debug, Clone, Default)]
pub struct EffectRegistry {
    /// Effect name → its declared dimensions.
    pub effects: HashMap<String, EffectProfile>,
    /// Dimension name → composition rule + default. Inferred from
    /// all effect declarations (each dimension that appears in any
    /// effect gets a schema entry).
    pub dimensions: HashMap<String, DimensionSchema>,
    /// Metadata for user-declared dimensions from `corvid.toml`.
    /// Empty for projects without a `[effect-system]` section or
    /// without a `corvid.toml` at all. Preserved so error messages can
    /// cite the user's `semantics` string and `corvid test dimensions`
    /// can drive the archetype's law-check proptest per entry.
    pub custom_dimensions: HashMap<String, CustomDimensionMeta>,
}

/// Built-in trust tiers in ascending restrictiveness order. Runtime
/// consumers that need to mirror the checker lattice should depend on
/// this list in tests rather than silently re-encode their own order.
pub const BUILTIN_TRUST_TIERS: &[&str] =
    &["autonomous", "supervisor_required", "human_required"];

/// The dimensional profile of a single declared effect.
#[derive(Debug, Clone, Default)]
pub struct EffectProfile {
    pub name: String,
    pub dimensions: HashMap<String, DimensionValue>,
}

/// A composed dimensional profile — the result of combining multiple
/// effects through a call graph.
#[derive(Debug, Clone, Default)]
pub struct ComposedProfile {
    pub dimensions: HashMap<String, DimensionValue>,
    pub effect_names: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CostEstimate {
    pub dimensions: HashMap<String, f64>,
    pub tree: CostTreeNode,
    pub warnings: Vec<CostWarning>,
    pub bounded: bool,
}

#[derive(Debug, Clone)]
pub struct CostTreeNode {
    pub name: String,
    pub kind: CostNodeKind,
    pub costs: HashMap<String, f64>,
    pub children: Vec<CostTreeNode>,
}

#[derive(Debug, Clone)]
pub enum CostNodeKind {
    Agent,
    Tool,
    Prompt,
    Sequence,
    Branch,
    Loop { iterations: Option<u64> },
    Condition,
}

#[derive(Debug, Clone)]
pub struct CostWarning {
    pub kind: CostWarningKind,
    pub span: corvid_ast::Span,
}

#[derive(Debug, Clone)]
pub enum CostWarningKind {
    UnboundedLoop {
        agent: String,
        message: String,
    },
}

impl EffectRegistry {
    /// Build the registry from a list of effect declarations.
    pub fn from_decls(decls: &[EffectDecl]) -> Self {
        Self::from_decls_with_config(decls, None)
    }

    /// Build the registry from effect declarations plus an optional
    /// `corvid.toml` configuration carrying user-defined dimensions.
    ///
    /// The config's dimensions are merged alongside the built-ins.
    /// Built-in names remain reserved — `CorvidConfig::into_dimension_schemas`
    /// rejects any collision before this function is called. A `None`
    /// config is equivalent to `from_decls`.
    pub fn from_decls_with_config(
        decls: &[EffectDecl],
        config: Option<&CorvidConfig>,
    ) -> Self {
        let mut registry = Self::default();

        // Register built-in dimension schemas with default composition rules.
        registry.register_builtin_dimensions();

        // Merge user-declared dimensions from corvid.toml, if any. A
        // malformed entry surfaces as a panic here — callers must have
        // pre-validated via CorvidConfig::into_dimension_schemas().
        // See `try_from_decls_with_config` for a fallible variant.
        if let Some(cfg) = config {
            if let Ok(schemas) = cfg.into_dimension_schemas() {
                for (schema, meta) in schemas {
                    registry
                        .custom_dimensions
                        .insert(schema.name.clone(), meta);
                    registry.dimensions.insert(schema.name.clone(), schema);
                }
            }
        }

        // Built-in `retrieval` effect with `data: grounded` so tools can
        // declare themselves as grounded sources for provenance tracking.
        registry.register_retrieval_effect();

        // Legacy bridge: the `dangerous` keyword on tools maps to a built-in
        // effect with `trust: human_required, reversible: false`. Existing
        // code using `dangerous` gets dimensional tracking for free.
        registry.register_dangerous_effect();

        for decl in decls {
            let mut profile = EffectProfile {
                name: decl.name.name.clone(),
                dimensions: HashMap::new(),
            };

            for dim in &decl.dimensions {
                let dim_name = canonical_dimension_name(&dim.name.name);
                profile.dimensions.insert(dim_name.clone(), dim.value.clone());

                // Infer schema from the dimension if not already registered.
                if !registry.dimensions.contains_key(&dim_name) {
                    let rule = infer_composition_rule(&dim_name, &dim.value);
                    let default = default_for_dimension(&dim_name, rule);
                    registry.dimensions.insert(
                        dim_name,
                        DimensionSchema {
                            name: canonical_dimension_name(&dim.name.name),
                            composition: rule,
                            default,
                        },
                    );
                }
            }

            registry.effects.insert(decl.name.name.clone(), profile);
        }

        registry
    }

    fn register_builtin_dimensions(&mut self) {
        self.dimensions.insert(
            "cost".into(),
            DimensionSchema {
                name: "cost".into(),
                composition: CompositionRule::Sum,
                default: DimensionValue::Cost(0.0),
            },
        );
        self.dimensions.insert(
            "trust".into(),
            DimensionSchema {
                name: "trust".into(),
                composition: CompositionRule::Max,
                default: DimensionValue::Name("autonomous".into()),
            },
        );
        self.dimensions.insert(
            "reversible".into(),
            DimensionSchema {
                name: "reversible".into(),
                composition: CompositionRule::LeastReversible,
                default: DimensionValue::Bool(true),
            },
        );
        self.dimensions.insert(
            "data".into(),
            DimensionSchema {
                name: "data".into(),
                composition: CompositionRule::Union,
                default: DimensionValue::Name("none".into()),
            },
        );
        self.dimensions.insert(
            "latency".into(),
            DimensionSchema {
                name: "latency".into(),
                composition: CompositionRule::Max,
                default: DimensionValue::Name("instant".into()),
            },
        );
        self.dimensions.insert(
            "tokens".into(),
            DimensionSchema {
                name: "tokens".into(),
                composition: CompositionRule::Sum,
                default: DimensionValue::Number(0.0),
            },
        );
        self.dimensions.insert(
            "latency_ms".into(),
            DimensionSchema {
                name: "latency_ms".into(),
                composition: CompositionRule::Max,
                default: DimensionValue::Number(0.0),
            },
        );
        self.dimensions.insert(
            "confidence".into(),
            DimensionSchema {
                name: "confidence".into(),
                composition: CompositionRule::Min,
                default: DimensionValue::Number(1.0),
            },
        );
        // Phase 20h: capability lattice for the typed model substrate.
        // basic < standard < expert. Max-composed — a call graph's
        // required capability is the strictest level any step requires.
        self.dimensions.insert(
            "capability".into(),
            DimensionSchema {
                name: "capability".into(),
                composition: CompositionRule::Max,
                default: DimensionValue::Name("basic".into()),
            },
        );
        // Phase 20h slice D: regulatory / compliance / privacy
        // dimensions for the typed model substrate. Each is a
        // declared-value dimension with its own archetype:
        //
        // * `jurisdiction` — Max-composed Name. Users pick their own
        //   jurisdiction tier names (e.g. `us_hosted`, `eu_hosted`,
        //   `us_hipaa_bva`). Max composition means a chain's
        //   jurisdiction is the strictest any step requires; unknown
        //   pairs fall back to lexicographic so composition is
        //   always deterministic.
        // * `compliance` — Union-composed Name set. A chain's
        //   compliance tags are the union of every step's tags.
        //   Fits the accumulative semantic — running through HIPAA
        //   AND SOC2-tagged models yields `hipaa, soc2`.
        // * `privacy_tier` — Max-composed Name. `standard < strict
        //   < air_gapped`. Identity `standard`.
        //
        // These dimensions are additive alongside the other built-ins;
        // a prompt's `requires: <capability>` clause only sets
        // capability, but a model's fields freely carry any declared
        // dimension. Slice B-rt's runtime dispatch reads model fields
        // and filters by all dimensions simultaneously.
        self.dimensions.insert(
            "jurisdiction".into(),
            DimensionSchema {
                name: "jurisdiction".into(),
                composition: CompositionRule::Max,
                default: DimensionValue::Name("none".into()),
            },
        );
        self.dimensions.insert(
            "compliance".into(),
            DimensionSchema {
                name: "compliance".into(),
                composition: CompositionRule::Union,
                default: DimensionValue::Name("none".into()),
            },
        );
        self.dimensions.insert(
            "privacy_tier".into(),
            DimensionSchema {
                name: "privacy_tier".into(),
                composition: CompositionRule::Max,
                default: DimensionValue::Name("standard".into()),
            },
        );
    }

    fn register_retrieval_effect(&mut self) {
        let mut dims = HashMap::new();
        dims.insert("data".into(), DimensionValue::Name("grounded".into()));
        self.effects.insert(
            "retrieval".into(),
            EffectProfile {
                name: "retrieval".into(),
                dimensions: dims,
            },
        );
    }

    fn register_dangerous_effect(&mut self) {
        let mut dims = HashMap::new();
        dims.insert("trust".into(), DimensionValue::Name("human_required".into()));
        dims.insert("reversible".into(), DimensionValue::Bool(false));
        self.effects.insert(
            "dangerous".into(),
            EffectProfile {
                name: "dangerous".into(),
                dimensions: dims,
            },
        );
    }

    /// Look up an effect by name and return its profile.
    pub fn get(&self, name: &str) -> Option<&EffectProfile> {
        self.effects.get(name)
    }

    /// Compose multiple effect names into a single dimensional profile
    /// by applying each dimension's composition rule.
    pub fn compose(&self, effect_names: &[&str]) -> ComposedProfile {
        let mut result = ComposedProfile {
            dimensions: HashMap::new(),
            effect_names: effect_names.iter().map(|s| s.to_string()).collect(),
        };

        // Start with defaults for all known dimensions.
        for (dim_name, schema) in &self.dimensions {
            result.dimensions.insert(dim_name.clone(), schema.default.clone());
        }

        // Layer each effect's dimensions using composition rules.
        for &effect_name in effect_names {
            let Some(profile) = self.effects.get(effect_name) else {
                continue;
            };
            for (dim_name, value) in &profile.dimensions {
                let Some(schema) = self.dimensions.get(dim_name) else {
                    continue;
                };
                let current = result
                    .dimensions
                    .entry(dim_name.clone())
                    .or_insert_with(|| schema.default.clone());
                *current = compose_dimension(schema.composition, current, value, &dim_name);
            }
        }

        result
    }

    /// Check a composed profile against a set of constraints. Returns
    /// a list of violations.
    pub fn check_constraints(
        &self,
        profile: &ComposedProfile,
        constraints: &[EffectConstraint],
    ) -> Vec<ConstraintViolation> {
        let mut violations = Vec::new();

        for constraint in constraints {
            let canonical_name = canonical_dimension_name(&constraint.dimension.name);
            let canonical_dim = canonical_name.as_str();
            let Some(actual) = profile.dimensions.get(canonical_dim) else {
                continue;
            };
            let expected = match (&constraint.value, actual) {
                (Some(expected), _) => Some(expected.clone()),
                // Bare boolean constraints like `@reversible` mean
                // "this dimension must stay true".
                (None, DimensionValue::Bool(_)) => Some(DimensionValue::Bool(true)),
                (None, _) => None,
            };
            if let Some(expected) = expected {
                if !dimension_satisfies(actual, &expected, canonical_dim) {
                    violations.push(ConstraintViolation {
                        dimension: canonical_dim.to_string(),
                        constraint: expected,
                        actual: actual.clone(),
                        span: constraint.span,
                    });
                }
            }
        }

        violations
    }
}

/// A constraint violation found during dimensional checking.
#[derive(Debug, Clone)]
pub struct ConstraintViolation {
    pub dimension: String,
    pub constraint: DimensionValue,
    pub actual: DimensionValue,
    pub span: corvid_ast::Span,
}

impl std::fmt::Display for ConstraintViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "dimension `{}`: constraint requires {}, but composed value is {}",
            self.dimension,
            format_dim_value(&self.constraint),
            format_dim_value(&self.actual),
        )
    }
}


mod analyze;
mod compose;
mod cost;
mod grounded;

pub use analyze::{analyze_effects, AgentEffectSummary};
pub use compose::{canonical_dimension_name, compose_dimension_public};
pub use cost::{
    compute_worst_case_cost, cost_path_for_dimension, format_numeric_dimension,
    numeric_constraint_value, render_cost_tree,
};
pub use grounded::{check_grounded_returns, ProvenanceResult, ProvenanceViolation};
use analyze::{find_agent, find_prompt, find_tool};
use compose::{
    backpressure_satisfies, capability_max, compose_dimension, default_for_dimension,
    dimension_satisfies, format_backpressure, format_dim_value, infer_composition_rule,
    latency_max, latency_rank, latency_streaming_rank, trust_max, trust_min,
};



#[cfg(test)]
mod tests {
    use super::{compose_dimension, dimension_satisfies};
    use corvid_ast::{BackpressurePolicy, CompositionRule, DimensionValue};

    #[test]
    fn streaming_latency_composes_above_named_classes() {
        let actual = compose_dimension(
            CompositionRule::Max,
            &DimensionValue::Name("fast".into()),
            &DimensionValue::Streaming {
                backpressure: BackpressurePolicy::Bounded(32),
            },
            "latency",
        );
        assert_eq!(
            actual,
            DimensionValue::Streaming {
                backpressure: BackpressurePolicy::Bounded(32),
            }
        );
    }

    #[test]
    fn streaming_latency_constraint_rejects_fast_floor() {
        assert!(!dimension_satisfies(
            &DimensionValue::Streaming {
                backpressure: BackpressurePolicy::Bounded(32),
            },
            &DimensionValue::Name("fast".into()),
            "latency",
        ));
        assert!(dimension_satisfies(
            &DimensionValue::Streaming {
                backpressure: BackpressurePolicy::Bounded(32),
            },
            &DimensionValue::Name("streaming".into()),
            "latency",
        ));
    }

    #[test]
    fn pull_backpressure_is_stricter_than_bounded_and_source_sensitive() {
        assert!(dimension_satisfies(
            &DimensionValue::Streaming {
                backpressure: BackpressurePolicy::PullsFrom("producer_rate".into()),
            },
            &DimensionValue::Streaming {
                backpressure: BackpressurePolicy::Bounded(8),
            },
            "latency",
        ));
        assert!(dimension_satisfies(
            &DimensionValue::Streaming {
                backpressure: BackpressurePolicy::PullsFrom("producer_rate".into()),
            },
            &DimensionValue::Streaming {
                backpressure: BackpressurePolicy::PullsFrom("producer_rate".into()),
            },
            "latency",
        ));
        assert!(!dimension_satisfies(
            &DimensionValue::Streaming {
                backpressure: BackpressurePolicy::PullsFrom("consumer_rate".into()),
            },
            &DimensionValue::Streaming {
                backpressure: BackpressurePolicy::PullsFrom("producer_rate".into()),
            },
            "latency",
        ));
    }
}
