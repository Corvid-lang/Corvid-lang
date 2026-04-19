//! Driver-level law-check runner.
//!
//! Wraps the `corvid_types::law_check` kernel: gathers every
//! built-in dimension plus the custom dimensions from `corvid.toml`,
//! runs each dimension against its archetype's algebraic laws, and
//! renders the per-law verdicts for CLI consumption. `corvid test
//! dimensions` is this module's entry point.
//!
//! Extracted from `lib.rs` as part of Phase 20i responsibility
//! decomposition (20i-audit-driver-b).

use corvid_ast::{
    CompositionRule as AstCompositionRule, DimensionSchema as AstDimensionSchema,
    DimensionValue as AstDimensionValue,
};
use corvid_types::{
    check_dimension, CorvidConfig, DimensionUnderTest, LawCheckResult, Verdict,
};

/// Run the archetype law-check suite against every built-in dimension
/// and every custom dimension declared in `corvid.toml`.
///
/// For each dimension, proptests the claimed composition archetype's
/// algebraic laws (associativity, commutativity, identity, plus
/// idempotence + monotonicity for semilattices) with `samples` cases
/// per law. Pass `DEFAULT_SAMPLES` (10,000) for the production run.
///
/// Returns one `LawCheckResult` per (dimension × law) pair. A single
/// counter-example short-circuits that law's check; the remaining
/// laws still run so users see the complete verdict in one report.
pub fn run_law_checks(
    config: Option<&CorvidConfig>,
    samples: usize,
) -> Vec<LawCheckResult> {
    let mut results = Vec::new();
    for dim in builtin_dimensions_under_test() {
        results.extend(check_dimension(&dim, samples));
    }
    if let Some(cfg) = config {
        if let Ok(schemas) = cfg.into_dimension_schemas() {
            for (schema, meta) in schemas {
                let dim = DimensionUnderTest::from_custom(schema, &meta);
                results.extend(check_dimension(&dim, samples));
            }
        }
    }
    results
}

/// Render a law-check report as human-readable text.
pub fn render_law_check_report(results: &[LawCheckResult]) -> String {
    use std::collections::BTreeMap;
    let mut by_dim: BTreeMap<String, Vec<&LawCheckResult>> = BTreeMap::new();
    for r in results {
        by_dim.entry(r.dimension.clone()).or_default().push(r);
    }
    let mut out = String::new();
    for (name, entries) in &by_dim {
        let rule = entries.first().map(|r| r.rule).unwrap_or(AstCompositionRule::Sum);
        out.push_str(&format!("\n  {name} ({rule:?})\n"));
        for r in entries {
            let status = match &r.verdict {
                Verdict::Pass => format!("ok ({} cases)", r.samples),
                Verdict::NotApplicable { reason } => format!("n/a — {reason}"),
                Verdict::CounterExample { note, .. } => format!("FAIL — {note}"),
            };
            out.push_str(&format!(
                "    {:<16} {status}\n",
                r.law.as_str(),
            ));
        }
    }
    let failures = results
        .iter()
        .filter(|r| matches!(r.verdict, Verdict::CounterExample { .. }))
        .count();
    if failures > 0 {
        out.push_str(&format!(
            "\n{failures} dimension(s) failed a law. See counter-examples above.\n"
        ));
    } else {
        out.push_str(&format!(
            "\nall {} dimensions satisfy their archetype's laws.\n",
            by_dim.len()
        ));
    }
    out
}

fn builtin_dimensions_under_test() -> Vec<DimensionUnderTest> {
    vec![
        DimensionUnderTest::from_schema(AstDimensionSchema {
            name: "cost".into(),
            composition: AstCompositionRule::Sum,
            default: AstDimensionValue::Cost(0.0),
        }),
        DimensionUnderTest::from_schema(AstDimensionSchema {
            name: "tokens".into(),
            composition: AstCompositionRule::Sum,
            default: AstDimensionValue::Number(0.0),
        }),
        DimensionUnderTest::from_schema(AstDimensionSchema {
            name: "latency_ms".into(),
            composition: AstCompositionRule::Sum,
            default: AstDimensionValue::Number(0.0),
        }),
        DimensionUnderTest::from_schema(AstDimensionSchema {
            name: "trust".into(),
            composition: AstCompositionRule::Max,
            default: AstDimensionValue::Name("autonomous".into()),
        }),
        DimensionUnderTest::from_schema(AstDimensionSchema {
            name: "confidence".into(),
            composition: AstCompositionRule::Min,
            default: AstDimensionValue::Number(f64::INFINITY),
        }),
        DimensionUnderTest::from_schema(AstDimensionSchema {
            name: "data".into(),
            composition: AstCompositionRule::Union,
            default: AstDimensionValue::Name("none".into()),
        }),
        DimensionUnderTest::from_schema(AstDimensionSchema {
            name: "reversible".into(),
            composition: AstCompositionRule::LeastReversible,
            default: AstDimensionValue::Bool(true),
        }),
        // Phase 20h: capability lattice (basic < standard < expert).
        DimensionUnderTest::from_schema(AstDimensionSchema {
            name: "capability".into(),
            composition: AstCompositionRule::Max,
            default: AstDimensionValue::Name("basic".into()),
        }),
        // Phase 20h slice D: regulatory/compliance/privacy dimensions.
        DimensionUnderTest::from_schema(AstDimensionSchema {
            name: "jurisdiction".into(),
            composition: AstCompositionRule::Max,
            default: AstDimensionValue::Name("none".into()),
        }),
        DimensionUnderTest::from_schema(AstDimensionSchema {
            name: "compliance".into(),
            composition: AstCompositionRule::Union,
            default: AstDimensionValue::Name("none".into()),
        }),
        DimensionUnderTest::from_schema(AstDimensionSchema {
            name: "privacy_tier".into(),
            composition: AstCompositionRule::Max,
            default: AstDimensionValue::Name("standard".into()),
        }),
    ]
}
