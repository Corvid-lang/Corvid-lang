//! Algebraic-law verification for dimension composition rules.
//!
//! Every `CompositionRule` claims to be one of five algebraic
//! archetypes. Each archetype has laws that must hold — associativity,
//! commutativity, identity, and (for semilattices) idempotence +
//! monotonicity. This module runs 10,000 randomized cases per law per
//! archetype, reports the first counter-example, and — when a law
//! fails — tells the user exactly which law broke and which values
//! broke it.
//!
//! Two surfaces use this:
//!   * `corvid test dimensions` runs law checks on every custom
//!     dimension declared in `corvid.toml` and on the built-ins.
//!   * Unit tests at the bottom of this file pin the laws into CI so
//!     any future change to the composition rules that would break a
//!     law gets caught immediately.
//!
//! See `docs/effects-spec/02-composition-algebra.md` §3 for the
//! archetype classification these laws pin down.
//!
//! No external dependencies — the generator is a seeded xorshift PRNG
//! so the check is deterministic and reproducible across runs.

use corvid_ast::{CompositionRule, DimensionSchema, DimensionValue};

use crate::config::{CustomDimensionMeta, DimensionValueType};
use crate::effects::compose_dimension_public;

/// Default number of generated cases per law. Large enough to catch
/// most edge cases; small enough to run in well under a second.
pub const DEFAULT_SAMPLES: usize = 10_000;

/// A single law's verdict.
#[derive(Debug, Clone)]
pub struct LawCheckResult {
    pub dimension: String,
    pub rule: CompositionRule,
    pub law: Law,
    pub samples: usize,
    pub verdict: Verdict,
}

/// One of the algebraic laws that an archetype claims to satisfy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Law {
    /// `x ⊕ (y ⊕ z) == (x ⊕ y) ⊕ z`
    Associativity,
    /// `x ⊕ y == y ⊕ x`
    Commutativity,
    /// `x ⊕ identity == x`
    Identity,
    /// `x ⊕ x == x` — holds for every archetype except Cumulative (Sum).
    Idempotence,
    /// `x ⊕ y ≥ x` (Cumulative with non-negative values) or
    /// `x ⊕ y ≥ max(x, y)` (Dominant) — captured per-archetype.
    Monotonicity,
}

impl Law {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Associativity => "associativity",
            Self::Commutativity => "commutativity",
            Self::Identity => "identity",
            Self::Idempotence => "idempotence",
            Self::Monotonicity => "monotonicity",
        }
    }
}

/// Result of checking a single law.
#[derive(Debug, Clone)]
pub enum Verdict {
    /// All `samples` cases satisfied the law.
    Pass,
    /// One case falsified the law. The triple is `(x, y, z)` — unused
    /// components are `Nothing` so the renderer doesn't claim more
    /// than it knows.
    CounterExample {
        x: DimensionValue,
        y: DimensionValue,
        z: DimensionValue,
        note: String,
    },
    /// The law isn't checkable for this archetype+type combination
    /// (e.g. idempotence for Sum over floats). Skipping is an explicit
    /// outcome, not a pass.
    NotApplicable { reason: String },
}

/// Input to a law check — schema + optional metadata.
#[derive(Debug, Clone)]
pub struct DimensionUnderTest {
    pub schema: DimensionSchema,
    /// Declared value type. When absent (built-ins), inferred from the
    /// schema's default.
    pub ty: Option<DimensionValueType>,
}

impl DimensionUnderTest {
    pub fn from_schema(schema: DimensionSchema) -> Self {
        Self { schema, ty: None }
    }

    pub fn from_custom(schema: DimensionSchema, meta: &CustomDimensionMeta) -> Self {
        Self {
            schema,
            ty: Some(meta.ty),
        }
    }

    pub fn inferred_type(&self) -> DimensionValueType {
        if let Some(ty) = self.ty {
            return ty;
        }
        match &self.schema.default {
            DimensionValue::Bool(_) => DimensionValueType::Bool,
            DimensionValue::Name(_) => DimensionValueType::Name,
            DimensionValue::Cost(_) => DimensionValueType::Cost,
            DimensionValue::Number(_) => DimensionValueType::Number,
            // Streaming / ConfidenceGated fall back to Name since they
            // carry named levels at the law level.
            _ => DimensionValueType::Name,
        }
    }
}

/// Run the full law suite for a dimension. Returns one result per law
/// that applies to the dimension's archetype.
pub fn check_dimension(dim: &DimensionUnderTest, samples: usize) -> Vec<LawCheckResult> {
    let rule = dim.schema.composition;
    let name = dim.schema.name.clone();
    let laws = laws_for_rule(rule);
    let mut results = Vec::with_capacity(laws.len());
    let mut rng = SeededRng::from_dimension(&name);
    for law in laws {
        let law = *law;
        let verdict = run_law(dim, law, samples, &mut rng);
        results.push(LawCheckResult {
            dimension: name.clone(),
            rule,
            law,
            samples,
            verdict,
        });
    }
    results
}

/// The laws claimed by each archetype. Cumulative (Sum) skips
/// idempotence — repeating a cost call costs twice as much.
pub fn laws_for_rule(rule: CompositionRule) -> &'static [Law] {
    match rule {
        CompositionRule::Sum => &[
            Law::Associativity,
            Law::Commutativity,
            Law::Identity,
            Law::Monotonicity,
        ],
        CompositionRule::Max
        | CompositionRule::Min
        | CompositionRule::Union
        | CompositionRule::LeastReversible => &[
            Law::Associativity,
            Law::Commutativity,
            Law::Identity,
            Law::Idempotence,
            Law::Monotonicity,
        ],
    }
}

fn run_law(dim: &DimensionUnderTest, law: Law, samples: usize, rng: &mut SeededRng) -> Verdict {
    let rule = dim.schema.composition;
    let name = &dim.schema.name;
    for _ in 0..samples {
        let x = random_value(dim, rng);
        let y = random_value(dim, rng);
        let z = random_value(dim, rng);
        match law {
            Law::Associativity => {
                let xy = compose_dimension_public(rule, &x, &y, name);
                let yz = compose_dimension_public(rule, &y, &z, name);
                let left = compose_dimension_public(rule, &xy, &z, name);
                let right = compose_dimension_public(rule, &x, &yz, name);
                if !equal(&left, &right) {
                    return Verdict::CounterExample {
                        x,
                        y,
                        z,
                        note: format!("(x ⊕ y) ⊕ z = {left:?} but x ⊕ (y ⊕ z) = {right:?}"),
                    };
                }
            }
            Law::Commutativity => {
                let xy = compose_dimension_public(rule, &x, &y, name);
                let yx = compose_dimension_public(rule, &y, &x, name);
                if !equal(&xy, &yx) {
                    return Verdict::CounterExample {
                        x,
                        y,
                        z: DimensionValue::Bool(false),
                        note: format!("x ⊕ y = {xy:?} but y ⊕ x = {yx:?}"),
                    };
                }
            }
            Law::Identity => {
                let identity = dim.schema.default.clone();
                let composed = compose_dimension_public(rule, &x, &identity, name);
                if !equal(&composed, &x) {
                    return Verdict::CounterExample {
                        x,
                        y: identity.clone(),
                        z: DimensionValue::Bool(false),
                        note: format!(
                            "x ⊕ default = {composed:?} but expected x = {:?}",
                            &dim.schema.default
                        ),
                    };
                }
            }
            Law::Idempotence => {
                let xx = compose_dimension_public(rule, &x, &x, name);
                if !equal(&xx, &x) {
                    return Verdict::CounterExample {
                        x: x.clone(),
                        y: x,
                        z: DimensionValue::Bool(false),
                        note: format!("x ⊕ x = {xx:?} but expected x"),
                    };
                }
            }
            Law::Monotonicity => match monotonicity_check(rule, &x, &y, name) {
                Some(msg) => {
                    return Verdict::CounterExample {
                        x,
                        y,
                        z: DimensionValue::Bool(false),
                        note: msg,
                    }
                }
                None => {}
            },
        }
    }
    Verdict::Pass
}

/// Monotonicity means: composing with `y` should not make the result
/// smaller than `x`. "Smaller" is defined per-archetype:
///   * Sum: `x + y ≥ x` whenever `y ≥ 0` (we only generate y ≥ 0).
///   * Max: `max(x, y) ≥ x` over the total order the archetype uses.
///   * Min: `min(x, y) ≤ x` — dual monotonicity.
///   * Union: `x ∪ y ⊇ x` over set inclusion.
///   * LeastReversible: `x ∧ y ≤ x` — the AND is conservative.
fn monotonicity_check(
    rule: CompositionRule,
    x: &DimensionValue,
    y: &DimensionValue,
    name: &str,
) -> Option<String> {
    let composed = compose_dimension_public(rule, x, y, name);
    match rule {
        CompositionRule::Sum => {
            if let (DimensionValue::Cost(x0), DimensionValue::Cost(c)) = (x, &composed) {
                if c + 1e-9 < *x0 {
                    return Some(format!("x + y = {c} decreased below x = {x0}"));
                }
            }
            if let (DimensionValue::Number(x0), DimensionValue::Number(c)) = (x, &composed) {
                if c + 1e-9 < *x0 {
                    return Some(format!("x + y = {c} decreased below x = {x0}"));
                }
            }
            None
        }
        CompositionRule::Max => match (x, &composed) {
            (DimensionValue::Number(x0), DimensionValue::Number(c)) if *c + 1e-9 < *x0 => {
                Some(format!("max(x, y) = {c} is less than x = {x0}"))
            }
            (DimensionValue::Cost(x0), DimensionValue::Cost(c)) if *c + 1e-9 < *x0 => {
                Some(format!("max(x, y) = ${c} is less than x = ${x0}"))
            }
            _ => None,
        },
        CompositionRule::Min => match (x, &composed) {
            (DimensionValue::Number(x0), DimensionValue::Number(c)) if *c > *x0 + 1e-9 => {
                Some(format!("min(x, y) = {c} is greater than x = {x0}"))
            }
            (DimensionValue::Cost(x0), DimensionValue::Cost(c)) if *c > *x0 + 1e-9 => {
                Some(format!("min(x, y) = ${c} is greater than x = ${x0}"))
            }
            _ => None,
        },
        CompositionRule::Union => match (x, &composed) {
            (DimensionValue::Name(x0), DimensionValue::Name(c)) => {
                if !union_includes(c, x0) {
                    return Some(format!("x ∪ y = `{c}` does not include x = `{x0}`"));
                }
                None
            }
            _ => None,
        },
        CompositionRule::LeastReversible => match (x, &composed) {
            (DimensionValue::Bool(x0), DimensionValue::Bool(c)) => {
                // Conservative: composition can only move true → false,
                // never false → true.
                if !x0 && *c {
                    return Some(format!(
                        "reversible composition promoted false → true for x = {x0}"
                    ));
                }
                None
            }
            _ => None,
        },
    }
}

fn union_includes(composed: &str, original: &str) -> bool {
    if original == "none" {
        return true;
    }
    for part in composed.split(',') {
        if part.trim() == original {
            return true;
        }
    }
    false
}

fn equal(a: &DimensionValue, b: &DimensionValue) -> bool {
    match (a, b) {
        (DimensionValue::Bool(x), DimensionValue::Bool(y)) => x == y,
        (DimensionValue::Name(x), DimensionValue::Name(y)) => x == y,
        (DimensionValue::Cost(x), DimensionValue::Cost(y)) => (x - y).abs() < 1e-9,
        (DimensionValue::Number(x), DimensionValue::Number(y)) => (x - y).abs() < 1e-9,
        _ => format!("{a:?}") == format!("{b:?}"),
    }
}

fn random_value(dim: &DimensionUnderTest, rng: &mut SeededRng) -> DimensionValue {
    let ty = dim.inferred_type();
    // For Max/Min archetypes, the declared default is the archetype's
    // bottom/top — the generator must respect that bound so the identity
    // law holds. A declared default of 0 under Max+Number means
    // "non-negative Number, 0 is the bottom"; producing a negative
    // would make max(x, 0) = 0 ≠ x.
    let (num_lo, num_hi) = numeric_range(dim);
    match (ty, dim.schema.composition) {
        (DimensionValueType::Bool, _) => DimensionValue::Bool(rng.bool()),
        (DimensionValueType::Cost, _) => DimensionValue::Cost(rng.float_in_range(num_lo, num_hi)),
        (DimensionValueType::Number, _) => {
            DimensionValue::Number(rng.float_in_range(num_lo, num_hi))
        }
        (DimensionValueType::Timestamp, _) => {
            DimensionValue::Number(rng.float_in_range(num_lo.max(0.0), num_hi.max(1e9)))
        }
        (DimensionValueType::Name, CompositionRule::Max) => {
            // Dispatch by dimension name so the generator stays in
            // the declared lattice. `capability`, `privacy_tier`,
            // `jurisdiction`, and `trust` are distinct ordered
            // lattices; other Max+Name dimensions fall back to a
            // generic placeholder set.
            let sample = match dim.schema.name.as_str() {
                "capability" => sample_capability_level(rng),
                "privacy_tier" => sample_privacy_tier(rng),
                "jurisdiction" => sample_jurisdiction(rng),
                _ => sample_trust_level(rng),
            };
            DimensionValue::Name(sample)
        }
        (DimensionValueType::Name, CompositionRule::Union) => {
            DimensionValue::Name(sample_data_category(rng))
        }
        (DimensionValueType::Name, _) => DimensionValue::Name(sample_generic_name(rng)),
    }
}

/// Derive the numeric generator's [lo, hi] range from the dimension's
/// declared default and archetype. The default is the archetype's
/// identity, which for Max is the bottom and for Min is the top —
/// the generator must stay above/below that boundary so the identity
/// law holds.
fn numeric_range(dim: &DimensionUnderTest) -> (f64, f64) {
    let default_value = match &dim.schema.default {
        DimensionValue::Cost(v) => Some(*v),
        DimensionValue::Number(v) => Some(*v),
        _ => None,
    };
    match (dim.schema.composition, default_value) {
        (CompositionRule::Sum, _) => (0.0, 1e6),
        (CompositionRule::Max, Some(floor)) if floor.is_finite() => (floor, floor + 1e6),
        (CompositionRule::Max, _) => (-1e6, 1e6),
        (CompositionRule::Min, Some(ceil)) if ceil.is_finite() => (ceil - 1e6, ceil),
        (CompositionRule::Min, _) => (-1e6, 1e6),
        _ => (-1e6, 1e6),
    }
}

fn sample_trust_level(rng: &mut SeededRng) -> String {
    const LEVELS: &[&str] = &["autonomous", "supervisor_required", "human_required"];
    LEVELS[rng.bounded(LEVELS.len())].into()
}

fn sample_capability_level(rng: &mut SeededRng) -> String {
    const LEVELS: &[&str] = &["basic", "standard", "expert"];
    LEVELS[rng.bounded(LEVELS.len())].into()
}

fn sample_privacy_tier(rng: &mut SeededRng) -> String {
    const TIERS: &[&str] = &["standard", "strict", "air_gapped"];
    TIERS[rng.bounded(TIERS.len())].into()
}

fn sample_jurisdiction(rng: &mut SeededRng) -> String {
    // Jurisdiction has no canonical ordering — users pick tier names
    // for their regulatory landscape. The generator samples from a
    // small placeholder set so law checks actually exercise the
    // lexicographic fallback deterministically.
    const TAGS: &[&str] = &["none", "us_hosted", "eu_hosted", "us_hipaa_bva"];
    TAGS[rng.bounded(TAGS.len())].into()
}

fn sample_data_category(rng: &mut SeededRng) -> String {
    const CATS: &[&str] = &["none", "public", "pii", "financial", "medical"];
    CATS[rng.bounded(CATS.len())].into()
}

fn sample_generic_name(rng: &mut SeededRng) -> String {
    const NAMES: &[&str] = &["none", "low", "medium", "high"];
    NAMES[rng.bounded(NAMES.len())].into()
}

/// A seeded xorshift64* PRNG. Deterministic across runs — reproducible
/// counter-examples are essential for law verification. Seed derives
/// from the dimension name so every dimension gets its own sequence
/// but the sequence doesn't depend on wall-clock time.
struct SeededRng {
    state: u64,
}

impl SeededRng {
    fn from_dimension(name: &str) -> Self {
        let mut state: u64 = 0xCBF29CE484222325; // FNV offset basis
        for byte in name.bytes() {
            state = state.wrapping_mul(0x100000001B3);
            state ^= u64::from(byte);
        }
        if state == 0 {
            state = 0x9E3779B97F4A7C15;
        }
        Self { state }
    }

    fn next_u64(&mut self) -> u64 {
        // xorshift64*
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.state.wrapping_mul(0x2545F4914F6CDD1D)
    }

    fn bool(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }

    fn bounded(&mut self, n: usize) -> usize {
        (self.next_u64() as usize) % n.max(1)
    }

    fn float_in_range(&mut self, lo: f64, hi: f64) -> f64 {
        let u = (self.next_u64() >> 11) as f64 / ((1u64 << 53) as f64);
        lo + u * (hi - lo)
    }

    fn nonneg_float_capped(&mut self, hi: f64) -> f64 {
        self.float_in_range(0.0, hi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin(name: &str, rule: CompositionRule, default: DimensionValue) -> DimensionUnderTest {
        DimensionUnderTest::from_schema(DimensionSchema {
            name: name.into(),
            composition: rule,
            default,
        })
    }

    fn assert_all_pass(results: &[LawCheckResult]) {
        for r in results {
            match &r.verdict {
                Verdict::Pass => {}
                Verdict::NotApplicable { .. } => {}
                Verdict::CounterExample { x, y, z, note } => panic!(
                    "dimension `{}` rule `{:?}` law `{}` found counter-example: \
                     x={x:?} y={y:?} z={z:?} — {note}",
                    r.dimension,
                    r.rule,
                    r.law.as_str()
                ),
            }
        }
    }

    #[test]
    fn sum_cost_satisfies_monoid_laws() {
        let dim = builtin("cost", CompositionRule::Sum, DimensionValue::Cost(0.0));
        let results = check_dimension(&dim, 1000);
        assert_all_pass(&results);
        // Sum skips idempotence by design — verify the law suite agrees.
        assert!(!results.iter().any(|r| matches!(r.law, Law::Idempotence)));
    }

    #[test]
    fn sum_tokens_number_satisfies_monoid_laws() {
        let dim = builtin("tokens", CompositionRule::Sum, DimensionValue::Number(0.0));
        let results = check_dimension(&dim, 1000);
        assert_all_pass(&results);
    }

    #[test]
    fn max_trust_satisfies_semilattice_laws() {
        let dim = builtin(
            "trust",
            CompositionRule::Max,
            DimensionValue::Name("autonomous".into()),
        );
        let results = check_dimension(&dim, 1000);
        assert_all_pass(&results);
        // Max claims idempotence — must be checked.
        assert!(results.iter().any(|r| matches!(r.law, Law::Idempotence)));
    }

    #[test]
    fn min_confidence_satisfies_semilattice_laws() {
        let dim = builtin(
            "confidence",
            CompositionRule::Min,
            DimensionValue::Number(f64::INFINITY),
        );
        let results = check_dimension(&dim, 1000);
        assert_all_pass(&results);
    }

    #[test]
    fn union_data_satisfies_semilattice_laws() {
        let dim = builtin(
            "data",
            CompositionRule::Union,
            DimensionValue::Name("none".into()),
        );
        let results = check_dimension(&dim, 1000);
        assert_all_pass(&results);
    }

    #[test]
    fn least_reversible_satisfies_semilattice_laws() {
        let dim = builtin(
            "reversible",
            CompositionRule::LeastReversible,
            DimensionValue::Bool(true),
        );
        let results = check_dimension(&dim, 1000);
        assert_all_pass(&results);
    }

    #[test]
    fn seeded_rng_is_deterministic() {
        let mut a = SeededRng::from_dimension("freshness");
        let mut b = SeededRng::from_dimension("freshness");
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_dimensions_get_different_sequences() {
        let mut a = SeededRng::from_dimension("freshness");
        let mut b = SeededRng::from_dimension("fairness");
        // Two different seeds → two different 64-bit sequences. The
        // probability of collision on the first sample is 2^-64.
        assert_ne!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn law_suite_returns_counter_example_when_law_is_broken() {
        // A mis-declared Sum dimension whose default is non-zero —
        // Sum's identity must be 0 (the additive identity). A default
        // of 5 breaks identity because x + 5 ≠ x for any x.
        let dim = builtin(
            "broken_sum",
            CompositionRule::Sum,
            DimensionValue::Number(5.0),
        );
        let results = check_dimension(&dim, 1000);
        let identity = results
            .iter()
            .find(|r| matches!(r.law, Law::Identity))
            .expect("identity law was checked");
        assert!(
            matches!(identity.verdict, Verdict::CounterExample { .. }),
            "Sum with default 5 must fail identity; got {:?}",
            identity.verdict
        );
    }
}
