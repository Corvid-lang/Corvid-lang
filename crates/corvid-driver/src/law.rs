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
use std::path::{Path, PathBuf};

use crate::proof_replay::{replay_dimension_proof, ProofReplayResult};

#[derive(Debug, Clone)]
pub struct DimensionVerificationReport {
    pub laws: Vec<LawCheckResult>,
    pub proofs: Vec<ProofReplayResult>,
}

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

/// Run both mandatory property-law checks and optional Lean/Coq proof
/// replay for custom dimensions that declare a proof path.
pub fn run_dimension_verification(
    config: Option<&CorvidConfig>,
    config_dir: Option<&Path>,
    samples: usize,
) -> DimensionVerificationReport {
    let laws = run_law_checks(config, samples);
    let mut proofs = Vec::new();
    if let Some(cfg) = config {
        if let Ok(schemas) = cfg.into_dimension_schemas() {
            for (schema, meta) in schemas {
                if let Some(proof) = meta.proof_path {
                    let proof_path = resolve_proof_path(config_dir, &proof);
                    proofs.push(replay_dimension_proof(&schema.name, &proof_path));
                }
            }
        }
    }
    DimensionVerificationReport { laws, proofs }
}

fn resolve_proof_path(config_dir: Option<&Path>, proof: &str) -> PathBuf {
    let path = PathBuf::from(proof);
    if path.is_absolute() {
        path
    } else {
        config_dir.unwrap_or_else(|| Path::new(".")).join(path)
    }
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

pub fn render_dimension_verification_report(report: &DimensionVerificationReport) -> String {
    let mut out = render_law_check_report(&report.laws);
    if report.proofs.is_empty() {
        out.push_str("no custom dimension proofs declared; proof replay skipped.\n");
        return out;
    }
    out.push_str("\nproof replay\n");
    for proof in &report.proofs {
        let status = if proof.failed() { "FAIL" } else { "ok" };
        out.push_str(&format!(
            "  {:<20} {:<4} {} ({}) — {}\n",
            proof.dimension,
            status,
            proof.proof_path.display(),
            proof.assistant,
            proof.message
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
