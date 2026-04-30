//! `corvid eval drift --explain` — model/input/prompt/index drift
//! attribution.
//!
//! Compares two lineage runs event-by-event (matched by event
//! name + kind) and decomposes the drift between them into four
//! named dimensions: model_id change, prompt_hash change,
//! retrieval_index_hash change, input_fingerprint change. Each
//! dimension's contribution is its (changed-event-count /
//! total-event-count) × 100. Any change the four dimensions don't
//! account for is "residual" — usually a status flip or cost
//! shift the prompt/model didn't drive.

use anyhow::{anyhow, Result};
use serde_json::Value;
use std::path::PathBuf;

use super::{read_lineage_input, source_descriptor};

#[derive(Debug, Clone)]
pub struct EvalDriftExplainArgs {
    pub baseline: PathBuf,
    pub candidate: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DriftAttributionReport {
    pub baseline: PathBuf,
    pub candidate: PathBuf,
    pub events_compared: usize,
    pub model_drift: DriftDimension,
    pub prompt_drift: DriftDimension,
    pub retrieval_index_drift: DriftDimension,
    pub input_drift: DriftDimension,
    pub residual_percent: f64,
    pub sources: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DriftDimension {
    pub name: String,
    pub changed_event_count: usize,
    pub contribution_percent: f64,
    pub example_changes: Vec<DriftExample>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DriftExample {
    pub event_name: String,
    pub baseline_value: String,
    pub candidate_value: String,
    pub baseline_source: Value,
    pub candidate_source: Value,
}

/// Compare two lineage runs event-by-event (matched by event
/// name + kind, same-position) and decompose the drift between
/// them into four named dimensions: model_id change, prompt_hash
/// change, retrieval_index_hash change, input_fingerprint
/// change. Each dimension's contribution is its
/// (changed-event-count / total-event-count) × 100. Any change
/// the four dimensions don't account for is "residual" — usually
/// a status flip or cost shift the prompt/model didn't drive.
pub fn run_eval_drift_explain(
    args: EvalDriftExplainArgs,
) -> Result<DriftAttributionReport> {
    let baseline = read_lineage_input(&args.baseline)?;
    let candidate = read_lineage_input(&args.candidate)?;
    if baseline.is_empty() || candidate.is_empty() {
        return Err(anyhow!(
            "drift comparison requires non-empty baseline + candidate inputs"
        ));
    }

    // Match events by (kind, name) — same logical event across
    // the two runs. We process every match once.
    let mut model_changes = 0usize;
    let mut prompt_changes = 0usize;
    let mut index_changes = 0usize;
    let mut input_changes = 0usize;
    let mut total_compared = 0usize;
    let mut residual = 0usize;
    let mut model_examples: Vec<DriftExample> = Vec::new();
    let mut prompt_examples: Vec<DriftExample> = Vec::new();
    let mut index_examples: Vec<DriftExample> = Vec::new();
    let mut input_examples: Vec<DriftExample> = Vec::new();
    let mut sources: Vec<Value> = Vec::new();

    for base in &baseline {
        if let Some(cand) = candidate
            .iter()
            .find(|c| c.kind == base.kind && c.name == base.name)
        {
            total_compared += 1;
            if base.model_id != cand.model_id {
                model_changes += 1;
                if model_examples.len() < 3 {
                    model_examples.push(DriftExample {
                        event_name: base.name.clone(),
                        baseline_value: base.model_id.clone(),
                        candidate_value: cand.model_id.clone(),
                        baseline_source: source_descriptor(base),
                        candidate_source: source_descriptor(cand),
                    });
                }
            }
            if base.prompt_hash != cand.prompt_hash {
                prompt_changes += 1;
                if prompt_examples.len() < 3 {
                    prompt_examples.push(DriftExample {
                        event_name: base.name.clone(),
                        baseline_value: base.prompt_hash.clone(),
                        candidate_value: cand.prompt_hash.clone(),
                        baseline_source: source_descriptor(base),
                        candidate_source: source_descriptor(cand),
                    });
                }
            }
            if base.retrieval_index_hash != cand.retrieval_index_hash {
                index_changes += 1;
                if index_examples.len() < 3 {
                    index_examples.push(DriftExample {
                        event_name: base.name.clone(),
                        baseline_value: base.retrieval_index_hash.clone(),
                        candidate_value: cand.retrieval_index_hash.clone(),
                        baseline_source: source_descriptor(base),
                        candidate_source: source_descriptor(cand),
                    });
                }
            }
            if base.input_fingerprint != cand.input_fingerprint {
                input_changes += 1;
                if input_examples.len() < 3 {
                    input_examples.push(DriftExample {
                        event_name: base.name.clone(),
                        baseline_value: base.input_fingerprint.clone(),
                        candidate_value: cand.input_fingerprint.clone(),
                        baseline_source: source_descriptor(base),
                        candidate_source: source_descriptor(cand),
                    });
                }
            }
            // Residual: outputs differ without any of the four
            // dimensions changing. That shows up as a status flip
            // or cost shift not driven by the named axes.
            let dimensions_changed = (base.model_id != cand.model_id) as i32
                + (base.prompt_hash != cand.prompt_hash) as i32
                + (base.retrieval_index_hash != cand.retrieval_index_hash) as i32
                + (base.input_fingerprint != cand.input_fingerprint) as i32;
            if dimensions_changed == 0
                && (base.status != cand.status
                    || (base.cost_usd - cand.cost_usd).abs() > f64::EPSILON)
            {
                residual += 1;
            }
            if sources.len() < 20 {
                sources.push(source_descriptor(base));
            }
        }
    }

    let pct = |n: usize| -> f64 {
        if total_compared == 0 {
            0.0
        } else {
            (n as f64 / total_compared as f64) * 100.0
        }
    };

    Ok(DriftAttributionReport {
        baseline: args.baseline,
        candidate: args.candidate,
        events_compared: total_compared,
        model_drift: DriftDimension {
            name: "model".to_string(),
            changed_event_count: model_changes,
            contribution_percent: pct(model_changes),
            example_changes: model_examples,
        },
        prompt_drift: DriftDimension {
            name: "prompt".to_string(),
            changed_event_count: prompt_changes,
            contribution_percent: pct(prompt_changes),
            example_changes: prompt_examples,
        },
        retrieval_index_drift: DriftDimension {
            name: "retrieval_index".to_string(),
            changed_event_count: index_changes,
            contribution_percent: pct(index_changes),
            example_changes: index_examples,
        },
        input_drift: DriftDimension {
            name: "input".to_string(),
            changed_event_count: input_changes,
            contribution_percent: pct(input_changes),
            example_changes: input_examples,
        },
        residual_percent: pct(residual),
        sources,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe_helpers_cmd::test_support::{ev, write_lineage};
    use corvid_runtime::lineage::{LineageKind, LineageStatus};

    /// Slice 40K: drift attribution decomposes a model swap as
    /// 100% model-drift contribution.
    #[test]
    fn drift_explain_attributes_model_swap() {
        let dir = tempfile::tempdir().unwrap();
        let baseline = dir.path().join("baseline.lineage.jsonl");
        let candidate = dir.path().join("candidate.lineage.jsonl");
        let mut be = ev(
            LineageKind::Prompt,
            "decide_refund",
            "t1",
            "s1",
            LineageStatus::Ok,
            "",
            0.10,
        );
        be.model_id = "gpt-old".to_string();
        be.prompt_hash = "ph1".to_string();
        be.retrieval_index_hash = "ri1".to_string();
        be.input_fingerprint = "in1".to_string();
        let mut ce = be.clone();
        ce.model_id = "gpt-new".to_string(); // only model changes
        write_lineage(&baseline, &[be]);
        write_lineage(&candidate, &[ce]);
        let report = run_eval_drift_explain(EvalDriftExplainArgs {
            baseline,
            candidate,
        })
        .unwrap();
        assert_eq!(report.events_compared, 1);
        assert_eq!(report.model_drift.changed_event_count, 1);
        assert_eq!(report.prompt_drift.changed_event_count, 0);
        assert_eq!(report.retrieval_index_drift.changed_event_count, 0);
        assert_eq!(report.input_drift.changed_event_count, 0);
        assert!((report.model_drift.contribution_percent - 100.0).abs() < f64::EPSILON);
        assert_eq!(report.model_drift.example_changes.len(), 1);
        assert_eq!(
            report.model_drift.example_changes[0].baseline_value,
            "gpt-old"
        );
    }

    /// Slice 40K: drift attribution surfaces residual when the
    /// status flips without any of the four named dimensions
    /// changing.
    #[test]
    fn drift_explain_surfaces_residual_when_status_flips_alone() {
        let dir = tempfile::tempdir().unwrap();
        let baseline = dir.path().join("baseline.lineage.jsonl");
        let candidate = dir.path().join("candidate.lineage.jsonl");
        let be = ev(
            LineageKind::Tool,
            "issue_refund",
            "t1",
            "s1",
            LineageStatus::Ok,
            "",
            0.05,
        );
        let mut ce = be.clone();
        ce.status = LineageStatus::Failed; // status flip with no dim change
        write_lineage(&baseline, &[be]);
        write_lineage(&candidate, &[ce]);
        let report = run_eval_drift_explain(EvalDriftExplainArgs {
            baseline,
            candidate,
        })
        .unwrap();
        assert!(report.residual_percent > 0.0);
        assert_eq!(report.model_drift.changed_event_count, 0);
    }
}
