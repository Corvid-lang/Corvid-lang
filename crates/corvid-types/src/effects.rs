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

// ---- Composition rules ----

/// Merge two comma-separated category sets into a single canonical
/// form: deduplicated, sorted. The `"none"` sentinel is absorbed by
/// any non-empty category.
fn merge_comma_sets(a: &str, b: &str) -> String {
    let mut categories: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for part in a.split(',').chain(b.split(',')) {
        let trimmed = part.trim();
        if trimmed.is_empty() || trimmed == "none" {
            continue;
        }
        categories.insert(trimmed.to_string());
    }
    if categories.is_empty() {
        return "none".into();
    }
    categories.into_iter().collect::<Vec<_>>().join(", ")
}

/// Public re-export of `compose_dimension` for the law-check harness.
/// Naming keeps the private helper private while making the same
/// composition logic available to `corvid test dimensions`.
pub fn compose_dimension_public(
    rule: CompositionRule,
    current: &DimensionValue,
    incoming: &DimensionValue,
    dim_name: &str,
) -> DimensionValue {
    compose_dimension(rule, current, incoming, dim_name)
}

fn compose_dimension(
    rule: CompositionRule,
    current: &DimensionValue,
    incoming: &DimensionValue,
    dim_name: &str,
) -> DimensionValue {
    match rule {
        CompositionRule::Sum => match (current, incoming) {
            (DimensionValue::Cost(a), DimensionValue::Cost(b)) => DimensionValue::Cost(a + b),
            (DimensionValue::Number(a), DimensionValue::Number(b)) => {
                DimensionValue::Number(a + b)
            }
            _ => incoming.clone(),
        },
        CompositionRule::Max => compose_max_dimension(current, incoming, dim_name),
        CompositionRule::Min => match (current, incoming) {
            (DimensionValue::Number(a), DimensionValue::Number(b)) => {
                DimensionValue::Number(a.min(*b))
            }
            (DimensionValue::Cost(a), DimensionValue::Cost(b)) => {
                DimensionValue::Cost(a.min(*b))
            }
            (DimensionValue::Name(a), DimensionValue::Name(b)) => {
                // Min on Name is lattice-aware for the trust lattice.
                // For unknown Name values, fall back to lexicographic
                // so the result is at least deterministic (not
                // hash-order-dependent).
                DimensionValue::Name(trust_min(a, b).to_string())
            }
            _ => incoming.clone(),
        },
        CompositionRule::Union => match (current, incoming) {
            (DimensionValue::Name(a), DimensionValue::Name(b)) => {
                // Parse each side as a comma-separated set, union them,
                // then re-render in sorted order. Substring-based dedup
                // was not associative: "pii" ⊕ ("financial" ⊕ "pii")
                // diverged from ("pii" ⊕ "financial") ⊕ "pii" because
                // the substring check missed the already-present
                // category. Law-checking the Union archetype caught it.
                DimensionValue::Name(merge_comma_sets(a, b))
            }
            _ => incoming.clone(),
        },
        CompositionRule::LeastReversible => match (current, incoming) {
            (DimensionValue::Bool(a), DimensionValue::Bool(b)) => {
                DimensionValue::Bool(*a && *b)
            }
            _ => incoming.clone(),
        },
    }
}

fn compose_max_dimension(
    current: &DimensionValue,
    incoming: &DimensionValue,
    dim_name: &str,
) -> DimensionValue {
    if dim_name == "latency" {
        return compose_latency_dimension(current, incoming);
    }
    if dim_name == "capability" {
        return compose_capability_dimension(current, incoming);
    }
    if dim_name == "privacy_tier" {
        return compose_privacy_tier_dimension(current, incoming);
    }
    match (current, incoming) {
        (DimensionValue::Number(a), DimensionValue::Number(b)) => DimensionValue::Number(a.max(*b)),
        (DimensionValue::Cost(a), DimensionValue::Cost(b)) => DimensionValue::Cost(a.max(*b)),
        (DimensionValue::Name(a), DimensionValue::Name(b)) => {
            DimensionValue::Name(trust_max(a, b).to_string())
        }
        // ConfidenceGated composes by taking its `above` level
        // for static checking. The runtime gate handles the rest.
        (DimensionValue::Name(a), DimensionValue::ConfidenceGated { above, .. }) => {
            DimensionValue::Name(trust_max(a, above).to_string())
        }
        (DimensionValue::ConfidenceGated { above, .. }, DimensionValue::Name(b)) => {
            DimensionValue::Name(trust_max(above, b).to_string())
        }
        (
            DimensionValue::ConfidenceGated {
                threshold: t1,
                above: a1,
                below: b1,
            },
            DimensionValue::ConfidenceGated {
                threshold: t2,
                above: a2,
                ..
            },
        ) => DimensionValue::ConfidenceGated {
            threshold: t1.max(*t2),
            above: trust_max(a1, a2).to_string(),
            below: b1.clone(),
        },
        _ => incoming.clone(),
    }
}

fn compose_latency_dimension(current: &DimensionValue, incoming: &DimensionValue) -> DimensionValue {
    match (current, incoming) {
        (
            DimensionValue::Streaming {
                backpressure: current_bp,
            },
            DimensionValue::Streaming {
                backpressure: incoming_bp,
            },
        ) => DimensionValue::Streaming {
            backpressure: compose_backpressure(current_bp, incoming_bp),
        },
        (DimensionValue::Streaming { .. }, _) => current.clone(),
        (_, DimensionValue::Streaming { .. }) => incoming.clone(),
        (DimensionValue::Name(current_name), DimensionValue::Name(incoming_name)) => {
            DimensionValue::Name(latency_max(current_name, incoming_name).to_string())
        }
        _ => incoming.clone(),
    }
}

fn compose_backpressure(
    current: &BackpressurePolicy,
    incoming: &BackpressurePolicy,
) -> BackpressurePolicy {
    match (current, incoming) {
        (BackpressurePolicy::Unbounded, _) | (_, BackpressurePolicy::Unbounded) => {
            BackpressurePolicy::Unbounded
        }
        (BackpressurePolicy::Bounded(a), BackpressurePolicy::Bounded(b)) => {
            BackpressurePolicy::Bounded((*a).max(*b))
        }
    }
}

/// Capability lattice composer: basic < standard < expert.
/// Unknown names rank after `expert` so a user-declared tier like
/// `superhuman` composes as stricter than any built-in level.
fn compose_capability_dimension(
    current: &DimensionValue,
    incoming: &DimensionValue,
) -> DimensionValue {
    match (current, incoming) {
        (DimensionValue::Name(a), DimensionValue::Name(b)) => {
            DimensionValue::Name(capability_max(a, b).to_string())
        }
        _ => incoming.clone(),
    }
}

fn capability_rank(s: &str) -> u8 {
    match s {
        "basic" => 0,
        "standard" => 1,
        "expert" => 2,
        _ => 3,
    }
}

fn capability_max<'a>(a: &'a str, b: &'a str) -> &'a str {
    let ra = capability_rank(a);
    let rb = capability_rank(b);
    if ra == rb {
        // Two unknowns: pick lexicographic so the result is
        // deterministic across runs (HashMap iteration otherwise).
        if a >= b {
            a
        } else {
            b
        }
    } else if ra > rb {
        a
    } else {
        b
    }
}

/// Privacy-tier Max: standard < strict < air_gapped. Unknown names
/// fall through to lexicographic max for determinism.
fn compose_privacy_tier_dimension(
    current: &DimensionValue,
    incoming: &DimensionValue,
) -> DimensionValue {
    match (current, incoming) {
        (DimensionValue::Name(a), DimensionValue::Name(b)) => {
            DimensionValue::Name(privacy_tier_max(a, b).to_string())
        }
        _ => incoming.clone(),
    }
}

fn privacy_tier_rank(s: &str) -> u8 {
    match s {
        "standard" => 0,
        "strict" => 1,
        "air_gapped" => 2,
        _ => u8::MAX,
    }
}

fn privacy_tier_max<'a>(a: &'a str, b: &'a str) -> &'a str {
    let ra = privacy_tier_rank(a);
    let rb = privacy_tier_rank(b);
    if ra == u8::MAX || rb == u8::MAX {
        if a >= b {
            a
        } else {
            b
        }
    } else if ra > rb {
        a
    } else if rb > ra {
        b
    } else {
        a
    }
}

/// Trust level ordering: autonomous < supervisor_required < human_required.
fn trust_max<'a>(a: &'a str, b: &'a str) -> &'a str {
    // `none` is the universal identity for any Max-over-Name
    // dimension. Absorbing it first keeps the identity law true for
    // dimensions whose sampler includes "none" alongside other tags
    // (e.g. jurisdiction: `none` vs `eu_hosted` lex-ties wrong
    // without this short-circuit).
    if a == "none" {
        return b;
    }
    if b == "none" {
        return a;
    }
    let rank = |s: &str| -> u8 {
        match s {
            "autonomous" => 0,
            "supervisor_required" => 1,
            "human_required" => 2,
            _ => 3,
        }
    };
    let ra = rank(a);
    let rb = rank(b);
    if ra == rb {
        // Lex tie-break keeps composition commutative when two values
        // share a rank. The law-check harness caught this: without
        // the tie-break, `trust_max("us_hosted", "us_hipaa_bva")`
        // returned `a` unconditionally, violating commutativity.
        // Used by the generic Max-over-Name path that serves
        // `jurisdiction` and any other user-declared Name lattice
        // without a dedicated composer.
        if a >= b {
            a
        } else {
            b
        }
    } else if ra > rb {
        a
    } else {
        b
    }
}

fn trust_min<'a>(a: &'a str, b: &'a str) -> &'a str {
    let rank = |s: &str| -> u8 {
        match s {
            "autonomous" => 0,
            "supervisor_required" => 1,
            "human_required" => 2,
            _ => u8::MAX,
        }
    };
    let ra = rank(a);
    let rb = rank(b);
    // If either side isn't on the lattice, pick lexicographically so
    // the result is deterministic across runs (HashMap iteration
    // would otherwise make `a` vs `b` non-reproducible).
    if ra == u8::MAX || rb == u8::MAX {
        if a <= b {
            a
        } else {
            b
        }
    } else if ra <= rb {
        a
    } else {
        b
    }
}

fn dimension_satisfies(actual: &DimensionValue, constraint: &DimensionValue, dim_name: &str) -> bool {
    match (actual, constraint) {
        (DimensionValue::Cost(actual_cost), DimensionValue::Cost(budget)) => {
            actual_cost <= budget
        }
        (DimensionValue::Bool(actual_rev), DimensionValue::Bool(required_rev)) => {
            // If constraint requires reversible (true), actual must be true.
            !required_rev || *actual_rev
        }
        (DimensionValue::Name(actual_name), DimensionValue::Name(required_name)) => {
            if dim_name == "trust" {
                trust_rank(actual_name) <= trust_rank(required_name)
            } else if dim_name == "latency" {
                latency_rank(actual_name) <= latency_rank(required_name)
            } else {
                actual_name == required_name
            }
        }
        (DimensionValue::Streaming { .. }, DimensionValue::Name(required_name))
            if dim_name == "latency" =>
        {
            latency_streaming_rank() <= latency_rank(required_name)
        }
        (
            DimensionValue::Streaming {
                backpressure: actual_bp,
            },
            DimensionValue::Streaming {
                backpressure: required_bp,
            },
        ) if dim_name == "latency" => backpressure_satisfies(actual_bp, required_bp),
        (DimensionValue::Number(actual_num), DimensionValue::Number(limit)) => {
            if dim_name == "confidence" {
                // Confidence: higher is better. Actual must be >= required.
                actual_num >= limit
            } else {
                // Default: lower is better (latency, token count, etc.).
                actual_num <= limit
            }
        }
        // ConfidenceGated satisfies a trust constraint by checking the
        // `above` level (compile-time optimistic check — the runtime
        // gate handles the pessimistic case).
        (DimensionValue::ConfidenceGated { above, .. }, DimensionValue::Name(required)) => {
            if dim_name == "trust" {
                trust_rank(above) <= trust_rank(required)
            } else {
                true
            }
        }
        (DimensionValue::Name(actual_name), DimensionValue::ConfidenceGated { above, .. }) => {
            if dim_name == "trust" {
                trust_rank(actual_name) <= trust_rank(above)
            } else {
                true
            }
        }
        _ => true,
    }
}

fn trust_rank(s: &str) -> u8 {
    match s {
        "autonomous" => 0,
        "supervisor_required" => 1,
        "human_required" => 2,
        _ => 3,
    }
}

fn infer_composition_rule(name: &str, _value: &DimensionValue) -> CompositionRule {
    match name {
        "cost" => CompositionRule::Sum,
        "tokens" => CompositionRule::Sum,
        "trust" => CompositionRule::Max,
        "reversible" => CompositionRule::LeastReversible,
        "data" => CompositionRule::Union,
        "latency_ms" => CompositionRule::Max,
        "latency" => CompositionRule::Max,
        "confidence" => CompositionRule::Min,
        _ => CompositionRule::Max,
    }
}

fn default_for_dimension(name: &str, rule: CompositionRule) -> DimensionValue {
    match name {
        "cost" => return DimensionValue::Cost(0.0),
        "tokens" | "latency_ms" => return DimensionValue::Number(0.0),
        _ => {}
    }
    default_for_rule(rule)
}

fn default_for_rule(rule: CompositionRule) -> DimensionValue {
    match rule {
        CompositionRule::Sum => DimensionValue::Cost(0.0),
        CompositionRule::Max => DimensionValue::Name("none".into()),
        CompositionRule::Min => DimensionValue::Number(1.0),
        CompositionRule::Union => DimensionValue::Name("none".into()),
        CompositionRule::LeastReversible => DimensionValue::Bool(true),
    }
}

fn format_dim_value(v: &DimensionValue) -> String {
    match v {
        DimensionValue::Bool(b) => b.to_string(),
        DimensionValue::Name(n) => n.clone(),
        DimensionValue::Cost(c) => format!("${c:.4}"),
        DimensionValue::Number(n) => format!("{n}"),
        DimensionValue::Streaming { backpressure } => {
            format!("streaming(backpressure: {})", format_backpressure(backpressure))
        }
        DimensionValue::ConfidenceGated { threshold, above, below } => {
            format!("{above}_if_confident({threshold}) (below: {below})")
        }
    }
}

pub fn canonical_dimension_name(name: &str) -> String {
    match name {
        "budget" => "cost".into(),
        "min_confidence" => "confidence".into(),
        other => other.to_string(),
    }
}

fn format_backpressure(policy: &BackpressurePolicy) -> String {
    match policy {
        BackpressurePolicy::Bounded(size) => format!("bounded({size})"),
        BackpressurePolicy::Unbounded => "unbounded".into(),
    }
}

fn latency_max<'a>(a: &'a str, b: &'a str) -> &'a str {
    if latency_rank(a) >= latency_rank(b) {
        a
    } else {
        b
    }
}

fn latency_rank(name: &str) -> u8 {
    match name {
        "instant" => 0,
        "fast" => 1,
        "medium" => 2,
        "slow" => 3,
        "streaming" => latency_streaming_rank(),
        _ => latency_streaming_rank() + 1,
    }
}

fn latency_streaming_rank() -> u8 {
    4
}

fn backpressure_satisfies(actual: &BackpressurePolicy, required: &BackpressurePolicy) -> bool {
    match (actual, required) {
        (_, BackpressurePolicy::Unbounded) => true,
        (BackpressurePolicy::Unbounded, BackpressurePolicy::Bounded(_)) => false,
        (BackpressurePolicy::Bounded(actual), BackpressurePolicy::Bounded(required)) => {
            actual <= required
        }
    }
}

// ---- Call-graph effect analyzer ----

/// Per-agent inferred effect profile: the union of all effects used
/// by tools/prompts/agents called in the agent's body.
#[derive(Debug, Clone)]
pub struct AgentEffectSummary {
    pub agent_def_id: DefId,
    pub agent_name: String,
    pub declared_effects: Vec<String>,
    pub inferred_effects: Vec<String>,
    pub composed: ComposedProfile,
    pub violations: Vec<ConstraintViolation>,
}

/// Analyze all agents in the file and produce per-agent effect summaries.
pub fn analyze_effects(
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    registry: &EffectRegistry,
) -> Vec<AgentEffectSummary> {
    let mut summaries = Vec::new();

    for decl in &file.decls {
        let corvid_ast::Decl::Agent(agent) = decl else {
            continue;
        };
        let Some(def_id) = resolved.symbols.lookup_def(&agent.name.name) else {
            continue;
        };

        // Collect all effect names used by calls in this agent's body.
        let mut effect_names: Vec<String> = Vec::new();
        collect_body_effects(&agent.body, file, resolved, registry, &mut effect_names);

        // Deduplicate.
        effect_names.sort();
        effect_names.dedup();

        // Compose the dimensional profile.
        let refs: Vec<&str> = effect_names.iter().map(|s| s.as_str()).collect();
        let mut composed = registry.compose(&refs);

        // Phase 20h: collect capability requirements from every
        // prompt call in the body and Max-compose them into the
        // agent's `capability` dimension. A prompt's `requires:
        // <level>` clause sets the minimum model capability its
        // dispatch needs; the agent-level composed requirement is
        // the strictest capability any call needs.
        let mut capabilities: Vec<String> = Vec::new();
        collect_body_capabilities(&agent.body, file, resolved, &mut capabilities);
        if !capabilities.is_empty() {
            let mut iter = capabilities.into_iter();
            let mut acc = iter.next().unwrap();
            for next in iter {
                acc = capability_max(&acc, &next).to_string();
            }
            composed
                .dimensions
                .insert("capability".into(), DimensionValue::Name(acc));
        }

        // Check constraints.
        let violations = registry.check_constraints(&composed, &agent.constraints);

        // Declared effects from the agent's `uses` clause.
        let declared: Vec<String> = agent.effect_row.effects.iter()
            .map(|e| e.name.name.clone())
            .collect();

        summaries.push(AgentEffectSummary {
            agent_def_id: def_id,
            agent_name: agent.name.name.clone(),
            declared_effects: declared,
            inferred_effects: effect_names,
            composed,
            violations,
        });
    }

    summaries
}

fn collect_body_effects(
    block: &corvid_ast::Block,
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    registry: &EffectRegistry,
    effects: &mut Vec<String>,
) {
    for stmt in &block.stmts {
        collect_stmt_effects(stmt, file, resolved, registry, effects);
    }
}

/// Phase 20h: walk a block and collect the `capability_required`
/// string from every prompt call. Mirrors `collect_body_effects`'s
/// recursion pattern but looks at prompt-level attributes instead of
/// effect-row references.
fn collect_body_capabilities(
    block: &corvid_ast::Block,
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    caps: &mut Vec<String>,
) {
    for stmt in &block.stmts {
        collect_stmt_capabilities(stmt, file, resolved, caps);
    }
}

fn collect_stmt_capabilities(
    stmt: &corvid_ast::Stmt,
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    caps: &mut Vec<String>,
) {
    match stmt {
        corvid_ast::Stmt::Let { value, .. } => {
            collect_expr_capabilities(value, file, resolved, caps);
        }
        corvid_ast::Stmt::Return { value: Some(v), .. } => {
            collect_expr_capabilities(v, file, resolved, caps);
        }
        corvid_ast::Stmt::Return { value: None, .. } => {}
        corvid_ast::Stmt::Yield { value, .. } => {
            collect_expr_capabilities(value, file, resolved, caps);
        }
        corvid_ast::Stmt::If { cond, then_block, else_block, .. } => {
            collect_expr_capabilities(cond, file, resolved, caps);
            collect_body_capabilities(then_block, file, resolved, caps);
            if let Some(eb) = else_block {
                collect_body_capabilities(eb, file, resolved, caps);
            }
        }
        corvid_ast::Stmt::For { iter, body, .. } => {
            collect_expr_capabilities(iter, file, resolved, caps);
            collect_body_capabilities(body, file, resolved, caps);
        }
        corvid_ast::Stmt::Expr { expr, .. } => {
            collect_expr_capabilities(expr, file, resolved, caps);
        }
        _ => {}
    }
}

fn collect_expr_capabilities(
    expr: &corvid_ast::Expr,
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    caps: &mut Vec<String>,
) {
    match expr {
        corvid_ast::Expr::Call { callee, args, .. } => {
            if let corvid_ast::Expr::Ident { name: ident, .. } = callee.as_ref() {
                if let Some(corvid_resolve::Binding::Decl(def_id)) =
                    resolved.bindings.get(&ident.span)
                {
                    let entry = resolved.symbols.get(*def_id);
                    if matches!(entry.kind, corvid_resolve::DeclKind::Prompt) {
                        if let Some(prompt) = find_prompt(file, &entry.name) {
                            if let Some(req) = &prompt.capability_required {
                                caps.push(req.name.clone());
                            }
                        }
                    }
                    if matches!(entry.kind, corvid_resolve::DeclKind::Agent) {
                        if let Some(agent) = find_agent(file, &entry.name) {
                            collect_body_capabilities(&agent.body, file, resolved, caps);
                        }
                    }
                }
            }
            collect_expr_capabilities(callee, file, resolved, caps);
            for arg in args {
                collect_expr_capabilities(arg, file, resolved, caps);
            }
        }
        corvid_ast::Expr::FieldAccess { target, .. } => {
            collect_expr_capabilities(target, file, resolved, caps);
        }
        corvid_ast::Expr::BinOp { left, right, .. } => {
            collect_expr_capabilities(left, file, resolved, caps);
            collect_expr_capabilities(right, file, resolved, caps);
        }
        corvid_ast::Expr::UnOp { operand, .. } => {
            collect_expr_capabilities(operand, file, resolved, caps);
        }
        _ => {}
    }
}

fn collect_stmt_effects(
    stmt: &corvid_ast::Stmt,
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    registry: &EffectRegistry,
    effects: &mut Vec<String>,
) {
    match stmt {
        corvid_ast::Stmt::Let { value, .. } => {
            collect_expr_effects(value, file, resolved, registry, effects);
        }
        corvid_ast::Stmt::Return { value: Some(v), .. } => {
            collect_expr_effects(v, file, resolved, registry, effects);
        }
        corvid_ast::Stmt::Return { value: None, .. } => {}
        corvid_ast::Stmt::Yield { value, .. } => {
            collect_expr_effects(value, file, resolved, registry, effects);
        }
        corvid_ast::Stmt::If { cond, then_block, else_block, .. } => {
            collect_expr_effects(cond, file, resolved, registry, effects);
            collect_body_effects(then_block, file, resolved, registry, effects);
            if let Some(eb) = else_block {
                collect_body_effects(eb, file, resolved, registry, effects);
            }
        }
        corvid_ast::Stmt::For { iter, body, .. } => {
            collect_expr_effects(iter, file, resolved, registry, effects);
            collect_body_effects(body, file, resolved, registry, effects);
        }
        corvid_ast::Stmt::Approve { action, .. } => {
            collect_expr_effects(action, file, resolved, registry, effects);
        }
        corvid_ast::Stmt::Expr { expr, .. } => {
            collect_expr_effects(expr, file, resolved, registry, effects);
        }
    }
}

fn collect_expr_effects(
    expr: &corvid_ast::Expr,
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    registry: &EffectRegistry,
    effects: &mut Vec<String>,
) {
    match expr {
        corvid_ast::Expr::Call { callee, args, .. } => {
            // Check if callee resolves to a tool/prompt/agent with effects.
            if let corvid_ast::Expr::Ident { span, .. } = &**callee {
                if let Some(corvid_resolve::Binding::Decl(def_id)) = resolved.bindings.get(span) {
                    let entry = resolved.symbols.get(*def_id);
                    match entry.kind {
                        corvid_resolve::DeclKind::Tool => {
                            // Find the tool declaration and collect its effect row.
                            if let Some(tool) = find_tool(file, &entry.name) {
                                for eff in &tool.effect_row.effects {
                                    effects.push(eff.name.name.clone());
                                }
                                // Legacy: dangerous → implicit "dangerous" effect
                                if matches!(tool.effect, corvid_ast::Effect::Dangerous) {
                                    effects.push("dangerous".into());
                                }
                            }
                        }
                        corvid_resolve::DeclKind::Prompt => {
                            if let Some(prompt) = find_prompt(file, &entry.name) {
                                for eff in &prompt.effect_row.effects {
                                    effects.push(eff.name.name.clone());
                                }
                            }
                        }
                        corvid_resolve::DeclKind::Agent => {
                            if let Some(agent) = find_agent(file, &entry.name) {
                                // If the agent declares effects, use those.
                                // Otherwise, this would need recursive inference.
                                for eff in &agent.effect_row.effects {
                                    effects.push(eff.name.name.clone());
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            collect_expr_effects(callee, file, resolved, registry, effects);
            for arg in args {
                collect_expr_effects(arg, file, resolved, registry, effects);
            }
        }
        corvid_ast::Expr::FieldAccess { target, .. } => {
            collect_expr_effects(target, file, resolved, registry, effects);
        }
        corvid_ast::Expr::Index { target, index, .. } => {
            collect_expr_effects(target, file, resolved, registry, effects);
            collect_expr_effects(index, file, resolved, registry, effects);
        }
        corvid_ast::Expr::BinOp { left, right, .. } => {
            collect_expr_effects(left, file, resolved, registry, effects);
            collect_expr_effects(right, file, resolved, registry, effects);
        }
        corvid_ast::Expr::UnOp { operand, .. } => {
            collect_expr_effects(operand, file, resolved, registry, effects);
        }
        corvid_ast::Expr::List { items, .. } => {
            for item in items {
                collect_expr_effects(item, file, resolved, registry, effects);
            }
        }
        corvid_ast::Expr::TryPropagate { inner, .. } => {
            collect_expr_effects(inner, file, resolved, registry, effects);
        }
        corvid_ast::Expr::TryRetry { body, .. } => {
            collect_expr_effects(body, file, resolved, registry, effects);
        }
        _ => {}
    }
}

fn find_tool<'a>(file: &'a corvid_ast::File, name: &str) -> Option<&'a corvid_ast::ToolDecl> {
    file.decls.iter().find_map(|d| match d {
        corvid_ast::Decl::Tool(t) if t.name.name == name => Some(t),
        _ => None,
    })
}

fn find_prompt<'a>(file: &'a corvid_ast::File, name: &str) -> Option<&'a corvid_ast::PromptDecl> {
    file.decls.iter().find_map(|d| match d {
        corvid_ast::Decl::Prompt(p) if p.name.name == name => Some(p),
        _ => None,
    })
}

fn find_agent<'a>(file: &'a corvid_ast::File, name: &str) -> Option<&'a corvid_ast::AgentDecl> {
    file.decls.iter().find_map(|d| match d {
        corvid_ast::Decl::Agent(a) if a.name.name == name => Some(a),
        _ => None,
    })
}

// ---- Worst-case cost analysis ----

const COST_DIMENSIONS: [&str; 3] = ["cost", "tokens", "latency_ms"];

pub fn compute_worst_case_cost(
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    registry: &EffectRegistry,
    agent_name: &str,
) -> Option<CostEstimate> {
    let mut analyzer = CostAnalyzer::new(file, resolved, registry);
    analyzer.analyze_agent(agent_name)
}

pub fn render_cost_tree(
    tree: &CostTreeNode,
    budget_constraints: Option<&[EffectConstraint]>,
) -> String {
    let mut lines = Vec::new();
    render_cost_tree_lines(tree, "", true, &mut lines);

    if let Some(constraints) = budget_constraints {
        if !constraints.is_empty() {
            lines.push(String::new());
            for constraint in constraints {
                let dim = canonical_dimension_name(&constraint.dimension.name);
                let used = tree.costs.get(&dim).copied().unwrap_or(0.0);
                if let Some(limit) = numeric_constraint_value(constraint) {
                    let pct = if limit > 0.0 { (used / limit) * 100.0 } else { 0.0 };
                    let status = if used <= limit { "✓" } else { "✗" };
                    lines.push(format!(
                        "{:8} budget: {:<10} used: {:<10} ({:.1}%) {status}",
                        dim,
                        format_numeric_dimension(&dim, limit),
                        format_numeric_dimension(&dim, used),
                        pct,
                    ));
                }
            }
        }
    }

    lines.join("\n")
}

struct CostAnalyzer<'a> {
    file: &'a corvid_ast::File,
    resolved: &'a corvid_resolve::Resolved,
    registry: &'a EffectRegistry,
    memo: HashMap<String, CostEstimate>,
    visiting: HashSet<String>,
}

impl<'a> CostAnalyzer<'a> {
    fn new(
        file: &'a corvid_ast::File,
        resolved: &'a corvid_resolve::Resolved,
        registry: &'a EffectRegistry,
    ) -> Self {
        Self {
            file,
            resolved,
            registry,
            memo: HashMap::new(),
            visiting: HashSet::new(),
        }
    }

    fn analyze_agent(&mut self, agent_name: &str) -> Option<CostEstimate> {
        if let Some(cached) = self.memo.get(agent_name) {
            return Some(cached.clone());
        }

        let agent = find_agent(self.file, agent_name)?;
        if !self.visiting.insert(agent_name.to_string()) {
            return Some(zero_estimate(
                agent_name,
                CostNodeKind::Agent,
                agent.span,
            ));
        }

        let body_estimate = self.analyze_block(&agent.body, agent_name);
        self.visiting.remove(agent_name);

        let result = CostEstimate {
            dimensions: body_estimate.dimensions.clone(),
            tree: CostTreeNode {
                name: agent_name.to_string(),
                kind: CostNodeKind::Agent,
                costs: body_estimate.dimensions,
                children: body_estimate.tree.children,
            },
            warnings: body_estimate.warnings,
            bounded: body_estimate.bounded,
        };
        self.memo.insert(agent_name.to_string(), result.clone());
        Some(result)
    }

    fn analyze_block(&mut self, block: &corvid_ast::Block, agent_name: &str) -> CostEstimate {
        let mut children = Vec::new();
        let mut warnings = Vec::new();
        let mut bounded = true;

        for stmt in &block.stmts {
            let estimate = self.analyze_stmt(stmt, agent_name);
            if !tree_is_zero(&estimate.tree) {
                children.push(estimate.tree);
            }
            warnings.extend(estimate.warnings);
            bounded &= estimate.bounded;
        }

        let tree = sequence_tree("block", CostNodeKind::Sequence, children, block.span);
        CostEstimate {
            dimensions: tree.costs.clone(),
            tree,
            warnings,
            bounded,
        }
    }

    fn analyze_stmt(&mut self, stmt: &corvid_ast::Stmt, agent_name: &str) -> CostEstimate {
        match stmt {
            corvid_ast::Stmt::Let { value, span, .. }
            | corvid_ast::Stmt::Yield { value, span }
            | corvid_ast::Stmt::Expr { expr: value, span }
            | corvid_ast::Stmt::Approve { action: value, span } => {
                let mut estimate = self.analyze_expr(value, agent_name);
                estimate.tree.name = "stmt".into();
                estimate.tree.kind = CostNodeKind::Sequence;
                estimate.tree.costs = estimate.dimensions.clone();
                estimate.tree.children = prune_children(std::mem::take(&mut estimate.tree.children));
                estimate.tree = wrap_if_needed("stmt", estimate.tree, *span);
                estimate
            }
            corvid_ast::Stmt::Return { value: Some(value), span } => {
                let mut estimate = self.analyze_expr(value, agent_name);
                estimate.tree = wrap_if_needed("return", estimate.tree, *span);
                estimate
            }
            corvid_ast::Stmt::Return { value: None, span } => {
                zero_estimate("return", CostNodeKind::Sequence, *span)
            }
            corvid_ast::Stmt::If {
                cond,
                then_block,
                else_block,
                span,
            } => {
                let cond_est = self.analyze_expr(cond, agent_name);
                let then_est = self.analyze_block(then_block, agent_name);
                let else_est = else_block
                    .as_ref()
                    .map(|block| self.analyze_block(block, agent_name))
                    .unwrap_or_else(|| zero_estimate("else", CostNodeKind::Sequence, *span));
                let branch = branch_tree("if", &then_est.tree, &else_est.tree, *span);
                let sequence = sequence_tree(
                    "if",
                    CostNodeKind::Sequence,
                    vec![wrap_if_needed("condition", cond_est.tree, cond.span()), branch],
                    *span,
                );
                CostEstimate {
                    dimensions: sequence.costs.clone(),
                    tree: sequence,
                    warnings: collect_warnings(&[cond_est.warnings, then_est.warnings, else_est.warnings]),
                    bounded: cond_est.bounded && then_est.bounded && else_est.bounded,
                }
            }
            corvid_ast::Stmt::For { iter, body, span, .. } => {
                let iter_est = self.analyze_expr(iter, agent_name);
                if let Some(iterations) = static_loop_bound(iter) {
                    let body_est = self.analyze_block(body, agent_name);
                    let loop_tree = scale_tree(body_est.tree.clone(), iterations, *span);
                    let sequence = sequence_tree(
                        "for",
                        CostNodeKind::Sequence,
                        vec![wrap_if_needed("iter", iter_est.tree, iter.span()), loop_tree.clone()],
                        *span,
                    );
                    let mut warnings = iter_est.warnings;
                    warnings.extend(body_est.warnings);
                    CostEstimate {
                        dimensions: sequence.costs.clone(),
                        tree: sequence,
                        warnings,
                        bounded: iter_est.bounded && body_est.bounded,
                    }
                } else {
                    let warning = CostWarning {
                        kind: CostWarningKind::UnboundedLoop {
                            agent: agent_name.to_string(),
                            message: format!(
                                "unbounded loop at {}..{} — static iteration count unknown",
                                span.start, span.end
                            ),
                        },
                        span: *span,
                    };
                    let tree = sequence_tree(
                        "for",
                        CostNodeKind::Sequence,
                        vec![wrap_if_needed("iter", iter_est.tree, iter.span())],
                        *span,
                    );
                    let mut warnings = iter_est.warnings;
                    warnings.push(warning);
                    CostEstimate {
                        dimensions: tree.costs.clone(),
                        tree,
                        warnings,
                        bounded: false,
                    }
                }
            }
        }
    }

    fn analyze_expr(&mut self, expr: &corvid_ast::Expr, agent_name: &str) -> CostEstimate {
        match expr {
            corvid_ast::Expr::Call { callee, args, span } => {
                let mut children = Vec::new();
                let callee_est = self.analyze_expr(callee, agent_name);
                if !tree_is_zero(&callee_est.tree) {
                    children.push(callee_est.tree);
                }
                let mut warnings = callee_est.warnings;
                let mut bounded = callee_est.bounded;
                for arg in args {
                    let arg_est = self.analyze_expr(arg, agent_name);
                    if !tree_is_zero(&arg_est.tree) {
                        children.push(arg_est.tree);
                    }
                    warnings.extend(arg_est.warnings);
                    bounded &= arg_est.bounded;
                }
                if let Some(call_tree) = self.call_cost_tree(callee, agent_name, *span) {
                    children.push(call_tree);
                }
                let tree = sequence_tree("call", CostNodeKind::Sequence, children, *span);
                CostEstimate {
                    dimensions: tree.costs.clone(),
                    tree,
                    warnings,
                    bounded,
                }
            }
            corvid_ast::Expr::FieldAccess { target, .. } => self.analyze_expr(target, agent_name),
            corvid_ast::Expr::Index { target, index, span } => {
                let target_est = self.analyze_expr(target, agent_name);
                let index_est = self.analyze_expr(index, agent_name);
                let tree = sequence_tree(
                    "index",
                    CostNodeKind::Sequence,
                    vec![target_est.tree, index_est.tree],
                    *span,
                );
                CostEstimate {
                    dimensions: tree.costs.clone(),
                    tree,
                    warnings: collect_warnings(&[target_est.warnings, index_est.warnings]),
                    bounded: target_est.bounded && index_est.bounded,
                }
            }
            corvid_ast::Expr::BinOp { left, right, span, .. } => {
                let left_est = self.analyze_expr(left, agent_name);
                let right_est = self.analyze_expr(right, agent_name);
                let tree = sequence_tree(
                    "binop",
                    CostNodeKind::Sequence,
                    vec![left_est.tree, right_est.tree],
                    *span,
                );
                CostEstimate {
                    dimensions: tree.costs.clone(),
                    tree,
                    warnings: collect_warnings(&[left_est.warnings, right_est.warnings]),
                    bounded: left_est.bounded && right_est.bounded,
                }
            }
            corvid_ast::Expr::UnOp { operand, span, .. } => {
                let estimate = self.analyze_expr(operand, agent_name);
                let tree = wrap_if_needed("unop", estimate.tree, *span);
                CostEstimate {
                    dimensions: tree.costs.clone(),
                    tree,
                    warnings: estimate.warnings,
                    bounded: estimate.bounded,
                }
            }
            corvid_ast::Expr::List { items, span } => {
                let mut children = Vec::new();
                let mut warnings = Vec::new();
                let mut bounded = true;
                for item in items {
                    let estimate = self.analyze_expr(item, agent_name);
                    if !tree_is_zero(&estimate.tree) {
                        children.push(estimate.tree);
                    }
                    warnings.extend(estimate.warnings);
                    bounded &= estimate.bounded;
                }
                let tree = sequence_tree("list", CostNodeKind::Sequence, children, *span);
                CostEstimate {
                    dimensions: tree.costs.clone(),
                    tree,
                    warnings,
                    bounded,
                }
            }
            corvid_ast::Expr::TryPropagate { inner, .. } => self.analyze_expr(inner, agent_name),
            corvid_ast::Expr::TryRetry { body, span, .. } => {
                let estimate = self.analyze_expr(body, agent_name);
                let tree = wrap_if_needed("retry", estimate.tree, *span);
                CostEstimate {
                    dimensions: tree.costs.clone(),
                    tree,
                    warnings: estimate.warnings,
                    bounded: estimate.bounded,
                }
            }
            corvid_ast::Expr::Literal { .. } | corvid_ast::Expr::Ident { .. } => {
                zero_estimate("expr", CostNodeKind::Sequence, expr.span())
            }
        }
    }

    fn call_cost_tree(
        &mut self,
        callee: &corvid_ast::Expr,
        agent_name: &str,
        span: corvid_ast::Span,
    ) -> Option<CostTreeNode> {
        let corvid_ast::Expr::Ident { name, .. } = callee else {
            return None;
        };
        let binding = self.resolved.bindings.get(&name.span)?;
        let corvid_resolve::Binding::Decl(def_id) = binding else {
            return None;
        };
        let entry = self.resolved.symbols.get(*def_id);
        match entry.kind {
            corvid_resolve::DeclKind::Tool => {
                let tool = find_tool(self.file, &entry.name)?;
                Some(effect_node_for_decl(
                    &tool.name.name,
                    CostNodeKind::Tool,
                    &tool.effect_row,
                    tool.effect,
                    self.registry,
                    span,
                ))
            }
            corvid_resolve::DeclKind::Prompt => {
                let prompt = find_prompt(self.file, &entry.name)?;
                Some(effect_node_for_prompt(prompt, self.registry))
            }
            corvid_resolve::DeclKind::Agent => self
                .analyze_agent(&entry.name)
                .map(|estimate| rename_tree(estimate.tree, &entry.name)),
            _ => {
                let _ = agent_name;
                None
            }
        }
    }
}

fn zero_estimate(name: &str, kind: CostNodeKind, _span: corvid_ast::Span) -> CostEstimate {
    let costs = zero_costs();
    CostEstimate {
        dimensions: costs.clone(),
        tree: CostTreeNode {
            name: name.to_string(),
            kind,
            costs,
            children: Vec::new(),
        },
        warnings: Vec::new(),
        bounded: true,
    }
}

fn zero_costs() -> HashMap<String, f64> {
    COST_DIMENSIONS
        .iter()
        .map(|dim| ((*dim).to_string(), 0.0))
        .collect()
}

fn sequence_tree(
    name: &str,
    kind: CostNodeKind,
    children: Vec<CostTreeNode>,
    _span: corvid_ast::Span,
) -> CostTreeNode {
    let children = prune_children(children);
    let mut costs = zero_costs();
    for child in &children {
        add_costs(&mut costs, &child.costs);
    }
    CostTreeNode {
        name: name.to_string(),
        kind,
        costs,
        children,
    }
}

fn branch_tree(
    name: &str,
    then_tree: &CostTreeNode,
    else_tree: &CostTreeNode,
    _span: corvid_ast::Span,
) -> CostTreeNode {
    let mut costs = zero_costs();
    for dim in COST_DIMENSIONS {
        let then_cost = then_tree.costs.get(dim).copied().unwrap_or(0.0);
        let else_cost = else_tree.costs.get(dim).copied().unwrap_or(0.0);
        costs.insert(dim.to_string(), then_cost.max(else_cost));
    }
    CostTreeNode {
        name: name.to_string(),
        kind: CostNodeKind::Branch,
        costs,
        children: prune_children(vec![then_tree.clone(), else_tree.clone()]),
    }
}

fn wrap_if_needed(name: &str, tree: CostTreeNode, span: corvid_ast::Span) -> CostTreeNode {
    if tree.name == name && matches!(tree.kind, CostNodeKind::Sequence) {
        tree
    } else {
        sequence_tree(name, CostNodeKind::Sequence, vec![tree], span)
    }
}

fn tree_is_zero(tree: &CostTreeNode) -> bool {
    tree.costs.values().all(|value| *value <= f64::EPSILON) && tree.children.is_empty()
}

fn prune_children(children: Vec<CostTreeNode>) -> Vec<CostTreeNode> {
    children
        .into_iter()
        .filter(|child| !tree_is_zero(child))
        .collect()
}

fn add_costs(target: &mut HashMap<String, f64>, source: &HashMap<String, f64>) {
    for dim in COST_DIMENSIONS {
        let next = target.get(dim).copied().unwrap_or(0.0) + source.get(dim).copied().unwrap_or(0.0);
        target.insert(dim.to_string(), next);
    }
}

fn scale_tree(tree: CostTreeNode, iterations: u64, _span: corvid_ast::Span) -> CostTreeNode {
    let name = tree.name.clone();
    let costs = tree
        .costs
        .iter()
        .map(|(dim, value)| (dim.clone(), value * iterations as f64))
        .collect();
    CostTreeNode {
        name,
        kind: CostNodeKind::Loop {
            iterations: Some(iterations),
        },
        costs,
        children: vec![tree],
    }
}

fn collect_warnings(chunks: &[Vec<CostWarning>]) -> Vec<CostWarning> {
    let mut warnings = Vec::new();
    for chunk in chunks {
        warnings.extend(chunk.clone());
    }
    warnings
}

fn static_loop_bound(expr: &corvid_ast::Expr) -> Option<u64> {
    match expr {
        corvid_ast::Expr::List { items, .. } => Some(items.len() as u64),
        _ => None,
    }
}

pub fn numeric_constraint_value(constraint: &EffectConstraint) -> Option<f64> {
    match constraint.value.as_ref()? {
        DimensionValue::Cost(value) => Some(*value),
        DimensionValue::Number(value) => Some(*value),
        _ => None,
    }
}

fn effect_node_for_decl(
    name: &str,
    kind: CostNodeKind,
    effect_row: &corvid_ast::EffectRow,
    legacy_effect: Effect,
    registry: &EffectRegistry,
    _span: corvid_ast::Span,
) -> CostTreeNode {
    let mut effect_names: Vec<&str> = effect_row.effects.iter().map(|effect| effect.name.name.as_str()).collect();
    if matches!(legacy_effect, Effect::Dangerous) {
        effect_names.push("dangerous");
    }
    let profile = registry.compose(&effect_names);
    CostTreeNode {
        name: name.to_string(),
        kind,
        costs: numeric_dimensions_from_profile(&profile),
        children: Vec::new(),
    }
}

fn effect_node_for_prompt(prompt: &corvid_ast::PromptDecl, registry: &EffectRegistry) -> CostTreeNode {
    let effect_names: Vec<&str> = prompt.effect_row.effects.iter().map(|effect| effect.name.name.as_str()).collect();
    let profile = registry.compose(&effect_names);
    CostTreeNode {
        name: prompt.name.name.clone(),
        kind: CostNodeKind::Prompt,
        costs: numeric_dimensions_from_profile(&profile),
        children: Vec::new(),
    }
}

fn numeric_dimensions_from_profile(profile: &ComposedProfile) -> HashMap<String, f64> {
    let mut costs = zero_costs();
    for dim in COST_DIMENSIONS {
        let value = match profile.dimensions.get(dim) {
            Some(DimensionValue::Cost(value)) => *value,
            Some(DimensionValue::Number(value)) => *value,
            _ => 0.0,
        };
        costs.insert(dim.to_string(), value);
    }
    costs
}

fn rename_tree(mut tree: CostTreeNode, name: &str) -> CostTreeNode {
    tree.name = name.to_string();
    tree
}

fn render_cost_tree_lines(
    node: &CostTreeNode,
    prefix: &str,
    is_last: bool,
    lines: &mut Vec<String>,
) {
    let branch = if prefix.is_empty() {
        ""
    } else if is_last {
        "└── "
    } else {
        "├── "
    };
    lines.push(format!(
        "{prefix}{branch}{:<28} total: {:<10} tokens: {:<10} latency: {}",
        node.name,
        format_numeric_dimension("cost", node.costs.get("cost").copied().unwrap_or(0.0)),
        format_numeric_dimension("tokens", node.costs.get("tokens").copied().unwrap_or(0.0)),
        format_numeric_dimension("latency_ms", node.costs.get("latency_ms").copied().unwrap_or(0.0)),
    ));

    let next_prefix = if prefix.is_empty() {
        String::new()
    } else if is_last {
        format!("{prefix}    ")
    } else {
        format!("{prefix}│   ")
    };

    for (index, child) in node.children.iter().enumerate() {
        render_cost_tree_lines(
            child,
            &next_prefix,
            index + 1 == node.children.len(),
            lines,
        );
    }
}

pub fn cost_path_for_dimension(tree: &CostTreeNode, dimension: &str) -> Vec<String> {
    match tree.kind {
        CostNodeKind::Agent | CostNodeKind::Sequence | CostNodeKind::Condition => tree
            .children
            .iter()
            .filter(|child| child.costs.get(dimension).copied().unwrap_or(0.0) > 0.0)
            .flat_map(|child| cost_path_for_dimension(child, dimension))
            .collect(),
        CostNodeKind::Branch => tree
            .children
            .iter()
            .max_by(|left, right| {
                left.costs
                    .get(dimension)
                    .copied()
                    .unwrap_or(0.0)
                    .partial_cmp(&right.costs.get(dimension).copied().unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|child| cost_path_for_dimension(child, dimension))
            .unwrap_or_default(),
        CostNodeKind::Loop { iterations } => {
            let mut path = tree
                .children
                .get(0)
                .map(|child| cost_path_for_dimension(child, dimension))
                .unwrap_or_default();
            if let Some(iterations) = iterations {
                if let Some(last) = path.last_mut() {
                    *last = format!("{last} × {iterations} iterations");
                }
            }
            path
        }
        CostNodeKind::Tool | CostNodeKind::Prompt => vec![format!(
            "{} ({})",
            tree.name,
            format_numeric_dimension(dimension, tree.costs.get(dimension).copied().unwrap_or(0.0))
        )],
    }
}

pub fn format_numeric_dimension(dimension: &str, value: f64) -> String {
    match dimension {
        "cost" => format!("${value:.3}"),
        "tokens" => format!("{:.0}", value),
        "latency_ms" => format!("{:.1}s", value / 1000.0),
        _ => format!("{value:.3}"),
    }
}

// ---- Provenance analyzer for Grounded<T> ----

/// Result of provenance analysis for one agent.
#[derive(Debug, Clone)]
pub struct ProvenanceResult {
    pub agent_name: String,
    pub return_is_grounded: bool,
    pub grounded_locals: Vec<String>,
    pub ungrounded_return_path: Option<String>,
}

/// Check whether an agent returning `Grounded<T>` has provenance from
/// a `data: grounded` source feeding into its return value. Returns
/// violations for agents that fail the check.
pub fn check_grounded_returns(
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    registry: &EffectRegistry,
) -> Vec<ProvenanceViolation> {
    let mut violations = Vec::new();

    for decl in &file.decls {
        let corvid_ast::Decl::Agent(agent) = decl else {
            continue;
        };

        // Check if the return type is Grounded<T>.
        let return_type_name = format_type_ref(&agent.return_ty);
        if !return_type_name.starts_with("Grounded") {
            continue;
        }

        // Analyze provenance: which locals are grounded?
        let grounded_locals = analyze_agent_provenance(agent, file, resolved, registry);

        // Check if any return statement returns a grounded value.
        let return_is_grounded = check_return_grounded(
            &agent.body,
            &grounded_locals,
            file,
            resolved,
            registry,
        );

        if !return_is_grounded {
            violations.push(ProvenanceViolation {
                agent_name: agent.name.name.clone(),
                span: agent.return_ty.span(),
                message: format!(
                    "agent `{}` returns `{}` but no provenance path from a `data: grounded` \
                     source feeds into the return value. Call a tool with `uses retrieval` \
                     and pass its result (directly or through a prompt) to the return.",
                    agent.name.name, return_type_name,
                ),
            });
        }
    }

    violations
}

/// A provenance violation: an agent returns Grounded<T> without proof.
#[derive(Debug, Clone)]
pub struct ProvenanceViolation {
    pub agent_name: String,
    pub span: corvid_ast::Span,
    pub message: String,
}

/// Analyze which local variables in an agent body are "grounded" —
/// i.e., their value chain includes at least one `data: grounded` tool.
fn analyze_agent_provenance(
    agent: &corvid_ast::AgentDecl,
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    registry: &EffectRegistry,
) -> std::collections::HashSet<String> {
    let mut grounded: std::collections::HashSet<String> = std::collections::HashSet::new();

    for stmt in &agent.body.stmts {
        analyze_stmt_provenance(stmt, file, resolved, registry, &mut grounded);
    }

    grounded
}

fn analyze_stmt_provenance(
    stmt: &corvid_ast::Stmt,
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    registry: &EffectRegistry,
    grounded: &mut std::collections::HashSet<String>,
) {
    match stmt {
        corvid_ast::Stmt::Let { name, value, .. } => {
            if expr_is_grounded(value, file, resolved, registry, grounded) {
                grounded.insert(name.name.clone());
            }
        }
        corvid_ast::Stmt::Yield { .. } => {}
        corvid_ast::Stmt::If { then_block, else_block, .. } => {
            for s in &then_block.stmts {
                analyze_stmt_provenance(s, file, resolved, registry, grounded);
            }
            if let Some(eb) = else_block {
                for s in &eb.stmts {
                    analyze_stmt_provenance(s, file, resolved, registry, grounded);
                }
            }
        }
        corvid_ast::Stmt::For { body, .. } => {
            for s in &body.stmts {
                analyze_stmt_provenance(s, file, resolved, registry, grounded);
            }
        }
        _ => {}
    }
}

/// Determine if an expression produces a grounded value.
fn expr_is_grounded(
    expr: &corvid_ast::Expr,
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    registry: &EffectRegistry,
    grounded: &std::collections::HashSet<String>,
) -> bool {
    match expr {
        corvid_ast::Expr::Call { callee, args, .. } => {
            // Check if callee is a tool/prompt/agent with grounded effects.
            if let corvid_ast::Expr::Ident { span, .. } = &**callee {
                if let Some(corvid_resolve::Binding::Decl(def_id)) = resolved.bindings.get(span) {
                    let entry = resolved.symbols.get(*def_id);
                    match entry.kind {
                        corvid_resolve::DeclKind::Tool => {
                            if let Some(tool) = find_tool(file, &entry.name) {
                                // Check if the tool has a grounded effect.
                                if tool_is_grounded(tool, registry) {
                                    return true;
                                }
                            }
                        }
                        corvid_resolve::DeclKind::Prompt => {
                            // A prompt is grounded if ANY of its args are grounded.
                            // This is the key provenance flow: grounded input → grounded output.
                            for arg in args {
                                if expr_is_grounded(arg, file, resolved, registry, grounded) {
                                    return true;
                                }
                            }
                        }
                        corvid_resolve::DeclKind::Agent => {
                            // Check if the called agent returns Grounded<T>.
                            if let Some(agent) = find_agent(file, &entry.name) {
                                let ret_name = format_type_ref(&agent.return_ty);
                                if ret_name.starts_with("Grounded") {
                                    return true;
                                }
                                // Also check if the agent has grounded effects.
                                for eff in &agent.effect_row.effects {
                                    if let Some(profile) = registry.get(&eff.name.name) {
                                        if profile.dimensions.get("data")
                                            == Some(&DimensionValue::Name("grounded".into()))
                                        {
                                            return true;
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            false
        }
        corvid_ast::Expr::Ident { name, .. } => {
            // A local variable is grounded if it was previously assigned a grounded value.
            grounded.contains(&name.name)
        }
        corvid_ast::Expr::FieldAccess { target, .. } => {
            // Field access on a grounded struct is grounded.
            expr_is_grounded(target, file, resolved, registry, grounded)
        }
        _ => false,
    }
}

fn tool_is_grounded(tool: &corvid_ast::ToolDecl, registry: &EffectRegistry) -> bool {
    for eff in &tool.effect_row.effects {
        if let Some(profile) = registry.get(&eff.name.name) {
            if profile.dimensions.get("data")
                == Some(&DimensionValue::Name("grounded".into()))
            {
                return true;
            }
        }
        // Built-in: "retrieval" effect has data: grounded.
        if eff.name.name == "retrieval" {
            return true;
        }
    }
    false
}

fn check_return_grounded(
    block: &corvid_ast::Block,
    grounded: &std::collections::HashSet<String>,
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    registry: &EffectRegistry,
) -> bool {
    for stmt in &block.stmts {
        match stmt {
            corvid_ast::Stmt::Return { value: Some(expr), .. } => {
                if expr_is_grounded(expr, file, resolved, registry, grounded) {
                    return true;
                }
            }
            corvid_ast::Stmt::Yield { .. } => {}
            corvid_ast::Stmt::If { then_block, else_block, .. } => {
                let then_grounded = check_return_grounded(then_block, grounded, file, resolved, registry);
                let else_grounded = else_block.as_ref().map_or(false, |eb| {
                    check_return_grounded(eb, grounded, file, resolved, registry)
                });
                if then_grounded || else_grounded {
                    return true;
                }
            }
            corvid_ast::Stmt::For { body, .. } => {
                if check_return_grounded(body, grounded, file, resolved, registry) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn format_type_ref(ty: &corvid_ast::TypeRef) -> String {
    match ty {
        corvid_ast::TypeRef::Named { name, .. } => name.name.clone(),
        corvid_ast::TypeRef::Generic { name, args, .. } => {
            let inner: Vec<String> = args.iter().map(format_type_ref).collect();
            format!("{}<{}>", name.name, inner.join(", "))
        }
        corvid_ast::TypeRef::Weak { inner, .. } => format!("Weak<{}>", format_type_ref(inner)),
        corvid_ast::TypeRef::Function { params, ret, .. } => {
            let ps: Vec<String> = params.iter().map(format_type_ref).collect();
            format!("({}) -> {}", ps.join(", "), format_type_ref(ret))
        }
    }
}

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
}
