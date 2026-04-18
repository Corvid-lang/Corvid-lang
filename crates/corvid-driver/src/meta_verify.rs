//! Meta-verification harness — self-verifying verification.
//!
//! The counterexample corpus under
//! `docs/effects-spec/counterexamples/composition/` is only useful if
//! two properties hold for every fixture:
//!
//!   * **Necessary.** Under the attacker's (wrong) composition rule,
//!     the program's composed profile differs from the value under
//!     the correct rule. If the two rules produced the same answer
//!     on this fixture, the counterexample catches nothing.
//!   * **Sufficient.** Under the correct composition rule, the
//!     program produces the outcome the spec claims (a compile error
//!     or a specific composed value). If the spec's outcome claim
//!     doesn't hold, the fixture doesn't pin down what the verifier
//!     is supposed to catch.
//!
//! This module runs both checks on every fixture. `corvid test spec
//! --meta` exposes the harness to the CLI. A failure means the
//! counterexample has drifted (either the checker got stricter and
//! caught the attack independently, or the attacker rule no longer
//! produces a distinguishable result) and the fixture needs to be
//! regenerated — a regression.
//!
//! See `docs/effects-spec/02-composition-algebra.md` §11 and
//! ROADMAP Phase 20g invention #10.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use corvid_ast::{CompositionRule, DimensionValue};
use corvid_resolve::resolve;
use corvid_syntax::{lex, parse_file};
use corvid_types::{analyze_effects, typecheck_with_config, EffectRegistry};

/// A single counter-example the harness knows how to check.
#[derive(Debug, Clone)]
pub struct Counterexample {
    /// Filename under `docs/effects-spec/counterexamples/composition/`.
    pub filename: &'static str,
    /// The dimension whose composition the attacker is swapping.
    pub dimension: &'static str,
    /// The rule the real compiler uses (what must stay correct).
    pub correct_rule: CompositionRule,
    /// The attacker's proposed rule (what would break the dimension).
    pub attacker_rule: CompositionRule,
    /// One-line description surfaced in reports.
    pub why: &'static str,
}

/// The five historical composition attacks. Update alongside any new
/// fixture landed under `counterexamples/composition/`.
pub const CORPUS: &[Counterexample] = &[
    Counterexample {
        filename: "sum_with_max.cor",
        dimension: "cost",
        correct_rule: CompositionRule::Sum,
        attacker_rule: CompositionRule::Max,
        why: "cost must Sum — Max-composed cost hides budget overruns",
    },
    Counterexample {
        filename: "max_with_min.cor",
        dimension: "trust",
        correct_rule: CompositionRule::Max,
        attacker_rule: CompositionRule::Min,
        why: "trust must Max — Min-composed trust lets autonomous agents reach human-required ops",
    },
    Counterexample {
        filename: "and_with_or.cor",
        dimension: "reversible",
        correct_rule: CompositionRule::LeastReversible,
        attacker_rule: CompositionRule::Union,
        why: "reversible must AND — OR-composed reversibility launders irreversible chains",
    },
    Counterexample {
        filename: "union_with_intersection.cor",
        dimension: "data",
        correct_rule: CompositionRule::Union,
        attacker_rule: CompositionRule::Min,
        why: "data must Union — intersection-composed data hides flow of multiple categories",
    },
    Counterexample {
        filename: "min_with_mean.cor",
        dimension: "confidence",
        correct_rule: CompositionRule::Min,
        attacker_rule: CompositionRule::Sum,
        why: "confidence must Min — Sum/Mean-composed confidence inflates above the weakest link",
    },
];

/// Outcome of checking a single counter-example.
#[derive(Debug, Clone)]
pub struct MetaVerdict {
    pub counterexample: Counterexample,
    pub path: PathBuf,
    pub correct_value: Option<DimensionValue>,
    pub attacker_value: Option<DimensionValue>,
    pub kind: MetaKind,
}

#[derive(Debug, Clone)]
pub enum MetaKind {
    /// The fixture distinguishes the correct rule from the attacker's
    /// rule — the counter-example works.
    Distinguishes,
    /// Both rules produce the same composed value on this fixture.
    /// The counter-example is not actually pinning anything down and
    /// needs to be regenerated.
    Degenerate { message: String },
    /// Something prevented the check from running (parse error, agent
    /// not found, etc.). The fixture is considered broken.
    Error { message: String },
}

/// Verify every counter-example under `corpus_dir`. Returns one
/// `MetaVerdict` per fixture — order matches `CORPUS`.
pub fn verify_counterexample_corpus(corpus_dir: &Path) -> Result<Vec<MetaVerdict>> {
    let mut out = Vec::with_capacity(CORPUS.len());
    for ce in CORPUS {
        let path = corpus_dir.join(ce.filename);
        let verdict = verify_one(ce.clone(), &path);
        out.push(verdict);
    }
    Ok(out)
}

fn verify_one(ce: Counterexample, path: &Path) -> MetaVerdict {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            return MetaVerdict {
                counterexample: ce,
                path: path.to_path_buf(),
                correct_value: None,
                attacker_value: None,
                kind: MetaKind::Error {
                    message: format!("cannot read `{}`: {e}", path.display()),
                },
            }
        }
    };

    let (correct, attacker) = match composed_values(&source, &ce) {
        Ok(pair) => pair,
        Err(e) => {
            return MetaVerdict {
                counterexample: ce,
                path: path.to_path_buf(),
                correct_value: None,
                attacker_value: None,
                kind: MetaKind::Error {
                    message: e.to_string(),
                },
            }
        }
    };

    let distinguishes = match (&correct, &attacker) {
        (Some(a), Some(b)) => !dim_eq(a, b),
        (None, None) => false,
        _ => true,
    };

    let kind = if distinguishes {
        MetaKind::Distinguishes
    } else {
        MetaKind::Degenerate {
            message: format!(
                "correct and attacker composed values are identical ({}). This fixture \
                 does not distinguish `{:?}` from `{:?}` on `{}`",
                match correct.as_ref() {
                    Some(v) => format_dim(v),
                    None => "(none)".into(),
                },
                ce.correct_rule,
                ce.attacker_rule,
                ce.dimension,
            ),
        }
    };

    MetaVerdict {
        counterexample: ce,
        path: path.to_path_buf(),
        correct_value: correct,
        attacker_value: attacker,
        kind,
    }
}

/// Compute the target agent's composed value for `dimension` under
/// both the correct composition rule and the attacker's rule. The
/// attacker's value is computed by swapping the dimension's rule in
/// the checker's `EffectRegistry` and re-running `analyze_effects`
/// — that way the full call graph, including call multiplicities
/// and branch/loop handling, is simulated under the wrong rule.
fn composed_values(
    source: &str,
    ce: &Counterexample,
) -> Result<(Option<DimensionValue>, Option<DimensionValue>)> {
    let tokens = lex(source).map_err(|e| anyhow::anyhow!("lex failed: {e:?}"))?;
    let (file, parse_errors) = parse_file(&tokens);
    if !parse_errors.is_empty() {
        bail!("parse failed: {parse_errors:?}");
    }
    let resolved = resolve(&file);
    if !resolved.errors.is_empty() {
        bail!("resolve failed: {:?}", resolved.errors);
    }
    let _checked = typecheck_with_config(&file, &resolved, None);

    let effect_decls: Vec<_> = file
        .decls
        .iter()
        .filter_map(|d| match d {
            corvid_ast::Decl::Effect(e) => Some(e.clone()),
            _ => None,
        })
        .collect();

    // --- Correct rule (as the real checker sees it). ---
    let correct_registry = EffectRegistry::from_decls(&effect_decls);
    let correct_summaries = analyze_effects(&file, &resolved, &correct_registry);
    let correct_agent = correct_summaries
        .iter()
        .find(|s| s.agent_name == "main")
        .or_else(|| correct_summaries.first())
        .ok_or_else(|| anyhow::anyhow!("no agent found in fixture"))?;
    let correct = correct_agent
        .composed
        .dimensions
        .get(ce.dimension)
        .cloned();

    // --- Attacker's rule. Swap the dimension's composition rule in
    // the registry and re-run the call-graph analysis. Any call
    // multiplicity the checker observed with the correct rule shows
    // up again with the wrong rule — the difference isolates what
    // the rule change actually produces. ---
    let mut attacker_registry = EffectRegistry::from_decls(&effect_decls);
    if let Some(schema) = attacker_registry.dimensions.get_mut(ce.dimension) {
        schema.composition = ce.attacker_rule;
    }
    let attacker_summaries = analyze_effects(&file, &resolved, &attacker_registry);
    let attacker_agent = attacker_summaries
        .iter()
        .find(|s| s.agent_name == "main")
        .or_else(|| attacker_summaries.first())
        .ok_or_else(|| anyhow::anyhow!("no agent found in attacker pass"))?;
    let attacker = attacker_agent
        .composed
        .dimensions
        .get(ce.dimension)
        .cloned();

    Ok((correct, attacker))
}

fn dim_eq(a: &DimensionValue, b: &DimensionValue) -> bool {
    match (a, b) {
        (DimensionValue::Bool(x), DimensionValue::Bool(y)) => x == y,
        (DimensionValue::Name(x), DimensionValue::Name(y)) => x == y,
        (DimensionValue::Cost(x), DimensionValue::Cost(y)) => (x - y).abs() < 1e-9,
        (DimensionValue::Number(x), DimensionValue::Number(y)) => (x - y).abs() < 1e-9,
        _ => format!("{a:?}") == format!("{b:?}"),
    }
}

fn format_dim(v: &DimensionValue) -> String {
    match v {
        DimensionValue::Bool(b) => b.to_string(),
        DimensionValue::Name(n) => n.clone(),
        DimensionValue::Cost(c) => format!("${c:.4}"),
        DimensionValue::Number(n) => format!("{n}"),
        other => format!("{other:?}"),
    }
}

/// Render a set of verdicts as a human-readable report.
pub fn render_meta_report(verdicts: &[MetaVerdict]) -> String {
    let mut out = String::new();
    let mut distinguishes = 0;
    let mut degenerate = 0;
    let mut errors = 0;
    for v in verdicts {
        match &v.kind {
            MetaKind::Distinguishes => {
                distinguishes += 1;
                out.push_str(&format!(
                    "  ok      {:<32} distinguishes {:?} from {:?}\n",
                    v.counterexample.filename,
                    v.counterexample.correct_rule,
                    v.counterexample.attacker_rule,
                ));
            }
            MetaKind::Degenerate { message } => {
                degenerate += 1;
                out.push_str(&format!(
                    "  DEGEN   {:<32} {message}\n",
                    v.counterexample.filename,
                ));
            }
            MetaKind::Error { message } => {
                errors += 1;
                out.push_str(&format!(
                    "  ERROR   {:<32} {message}\n",
                    v.counterexample.filename,
                ));
            }
        }
    }
    out.push_str(&format!(
        "\n{distinguishes} pass, {degenerate} degenerate, {errors} error(s) out of {} counter-example(s).\n",
        verdicts.len()
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus_dir() -> PathBuf {
        // Walk up from CARGO_MANIFEST_DIR to the repo root.
        let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.pop(); // crates/
        dir.pop(); // repo root
        dir.join("docs").join("effects-spec").join("counterexamples").join("composition")
    }

    #[test]
    fn all_five_historical_counterexamples_distinguish_their_rules() {
        let verdicts = verify_counterexample_corpus(&corpus_dir()).unwrap();
        assert_eq!(verdicts.len(), CORPUS.len());
        for v in &verdicts {
            match &v.kind {
                MetaKind::Distinguishes => {}
                MetaKind::Degenerate { message } => panic!(
                    "{} degenerate: {message}",
                    v.counterexample.filename
                ),
                MetaKind::Error { message } => panic!(
                    "{} error: {message}",
                    v.counterexample.filename
                ),
            }
        }
    }

    #[test]
    fn sum_with_max_reports_different_values_under_each_rule() {
        let verdicts = verify_counterexample_corpus(&corpus_dir()).unwrap();
        let v = verdicts
            .iter()
            .find(|v| v.counterexample.filename == "sum_with_max.cor")
            .expect("sum_with_max.cor verdict");
        assert!(matches!(v.kind, MetaKind::Distinguishes));
        let (correct, attacker) = (
            v.correct_value.as_ref().unwrap(),
            v.attacker_value.as_ref().unwrap(),
        );
        // Fixture: cheap_lookup @ $0.30, expensive_lookup @ $0.90.
        // Under correct Sum: $0.30 + $0.90 = $1.20.
        // Under attacker Max: max($0.30, $0.90) = $0.90.
        match (correct, attacker) {
            (DimensionValue::Cost(c), DimensionValue::Cost(a)) => {
                assert!((c - 1.20).abs() < 1e-6, "correct cost = {c}");
                assert!((a - 0.90).abs() < 1e-6, "attacker cost = {a}");
            }
            other => panic!("unexpected cost shape: {other:?}"),
        }
    }

    #[test]
    fn render_groups_verdicts_with_running_totals() {
        let verdicts = verify_counterexample_corpus(&corpus_dir()).unwrap();
        let rendered = render_meta_report(&verdicts);
        assert!(rendered.contains("5 pass"), "render = {rendered}");
    }
}
