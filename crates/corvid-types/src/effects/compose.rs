//! Dimensional composition algebra.
//!
//! Per-dimension composers — every built-in dimension (cost, trust,
//! reversible, data, latency, confidence, tokens, latency_ms,
//! capability, jurisdiction, compliance, privacy_tier) plus custom
//! dimensions declared in corvid.toml go through one of these:
//!
//!   Sum / Max / Min / Union / LeastReversible / Mean
//!
//! Lattice-aware composers (trust, capability, privacy_tier,
//! latency, jurisdiction) use ordered rank tables with lex tie-break
//! for unknown-tag commutativity.
//!
//! Extracted from `effects.rs` as part of Phase 20i responsibility
//! decomposition.

use corvid_ast::{BackpressurePolicy, CompositionRule, DimensionValue};

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

pub(super) fn compose_dimension(
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

pub(super) fn capability_max<'a>(a: &'a str, b: &'a str) -> &'a str {
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
pub(super) fn trust_max<'a>(a: &'a str, b: &'a str) -> &'a str {
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

pub(super) fn trust_min<'a>(a: &'a str, b: &'a str) -> &'a str {
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

pub(super) fn dimension_satisfies(actual: &DimensionValue, constraint: &DimensionValue, dim_name: &str) -> bool {
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

pub(super) fn infer_composition_rule(name: &str, _value: &DimensionValue) -> CompositionRule {
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

pub(super) fn default_for_dimension(name: &str, rule: CompositionRule) -> DimensionValue {
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

pub(super) fn format_dim_value(v: &DimensionValue) -> String {
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

pub(super) fn format_backpressure(policy: &BackpressurePolicy) -> String {
    match policy {
        BackpressurePolicy::Bounded(size) => format!("bounded({size})"),
        BackpressurePolicy::Unbounded => "unbounded".into(),
    }
}

pub(super) fn latency_max<'a>(a: &'a str, b: &'a str) -> &'a str {
    if latency_rank(a) >= latency_rank(b) {
        a
    } else {
        b
    }
}

pub(super) fn latency_rank(name: &str) -> u8 {
    match name {
        "instant" => 0,
        "fast" => 1,
        "medium" => 2,
        "slow" => 3,
        "streaming" => latency_streaming_rank(),
        _ => latency_streaming_rank() + 1,
    }
}

pub(super) fn latency_streaming_rank() -> u8 {
    4
}

pub(super) fn backpressure_satisfies(actual: &BackpressurePolicy, required: &BackpressurePolicy) -> bool {
    match (actual, required) {
        (_, BackpressurePolicy::Unbounded) => true,
        (BackpressurePolicy::Unbounded, BackpressurePolicy::Bounded(_)) => false,
        (BackpressurePolicy::Bounded(actual), BackpressurePolicy::Bounded(required)) => {
            actual <= required
        }
    }
}
