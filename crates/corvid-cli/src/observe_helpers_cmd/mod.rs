//! AI-assisted observability + eval helpers — slice 40K,
//! decomposed in Phase 20j-S3.
//!
//! The Phase 40 phase-done checklist names four helper
//! subcommands the developer-flow doc shows operators running:
//!
//!   `corvid observe explain <trace-id>`         — RAG-grounded
//!     incident root cause from a typed lineage trace.
//!   `corvid observe cost-optimise <agent>`       — generative
//!     route/escalate/cache suggestions from cost rollup.
//!   `corvid eval drift --explain`                — decompose
//!     drift between two trace runs into model / input /
//!     prompt / retrieval contributions.
//!   `corvid eval generate-from-feedback <id>`    — eval fixture
//!     synthesised from a "wrong answer" feedback record.
//!
//! Each ships in two layers:
//!
//!   1. A deterministic Rust handler that produces the structured
//!      output (`*Report`/`*Plan`/`*Attribution`/`EvalFixture`)
//!      from the lineage store. This is the always-available path
//!      — no LLM key required.
//!
//!   2. A paired `.cor` source under
//!      `examples/observe_helpers/` documenting the
//!      `Grounded<T>`-shaped LLM-grounded version: typed effects,
//!      `@budget`, the prompt's `cites strictly` clause, and the
//!      `Grounded<…>` return type. Production deployments wire the
//!      `.cor` program through the project's configured LLM
//!      adapter; the heuristic stays as the deterministic
//!      fallback so the helpers remain useful in CI and offline.
//!
//! Each output carries a `sources` array (the `Grounded<T>` shape
//! at the JSON layer) listing the trace_id + span_id of every
//! lineage event the helper consulted. A downstream consumer can
//! `JOIN` against the trace store to reconstruct the evidence
//! the analysis rests on.
//!
//! The module is split per CLI surface (Phase 20j-S3):
//!
//! - [`observe_explain`] — `corvid observe explain <trace-id>`.
//! - The cost-optimise / drift / from-feedback surfaces stay in
//!   this file mid-refactor; commits 20j-S3 #2/#3/#4 relocate
//!   them.

pub mod observe_explain;
#[allow(unused_imports)]
pub use observe_explain::*;

#[cfg(test)]
mod test_support;

use anyhow::{anyhow, Context, Result};
use corvid_runtime::lineage::{LineageEvent, LineageKind, LineageStatus};
use corvid_runtime::lineage_redact::{redact_lineage_events, LineageRedactionPolicy};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------

/// Read a lineage JSONL file or a directory of them. Mirrors the
/// existing `observe_cmd::read_lineage_input` shape.
pub fn read_lineage_input(path: &Path) -> Result<Vec<LineageEvent>> {
    if path.is_dir() {
        let mut events = Vec::new();
        let mut entries = fs::read_dir(path)
            .with_context(|| format!("read_dir `{}`", path.display()))?
            .map(|e| e.map(|e| e.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        entries.sort();
        for entry in entries {
            if entry.extension().and_then(|s| s.to_str()) == Some("jsonl")
                || entry
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.ends_with(".lineage.jsonl"))
                    .unwrap_or(false)
            {
                events.extend(read_lineage_file(&entry)?);
            }
        }
        return Ok(events);
    }
    read_lineage_file(path)
}

fn read_lineage_file(path: &Path) -> Result<Vec<LineageEvent>> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("reading lineage from `{}`", path.display()))?;
    let mut events = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let event: LineageEvent = serde_json::from_str(trimmed)
            .with_context(|| format!("line {} is not a lineage event", i + 1))?;
        events.push(event);
    }
    Ok(events)
}

pub(crate) fn source_descriptor(event: &LineageEvent) -> Value {
    json!({
        "trace_id": event.trace_id,
        "span_id": event.span_id,
        "kind": event.kind,
        "name": event.name,
    })
}

pub(crate) fn select_run(events: &[LineageEvent], trace_id: &str) -> Vec<LineageEvent> {
    events
        .iter()
        .filter(|e| e.trace_id == trace_id)
        .cloned()
        .collect()
}

// ---------------------------------------------------------------
// 2. observe cost-optimise — generative route/cache suggestions
// ---------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ObserveCostOptimiseArgs {
    pub trace_dir: PathBuf,
    pub agent: String,
    pub top_n: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CostOptimisationReport {
    pub agent: String,
    pub trace_count: usize,
    pub total_cost_usd: f64,
    pub top_cost_centers: Vec<CostCenter>,
    pub suggestions: Vec<CostSuggestion>,
    pub sources: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CostCenter {
    pub name: String,
    pub kind: String,
    pub total_cost_usd: f64,
    pub call_count: u64,
    pub percent_of_total: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CostSuggestion {
    pub kind: String,
    pub description: String,
    pub estimated_savings_usd: f64,
    pub sources: Vec<Value>,
}

/// Aggregate cost-by-event-name across all traces under
/// `trace_dir`, identify the top-N cost centres, and propose
/// concrete optimisations. The heuristic suggests cache when an
/// event name with the same input fingerprint fired multiple
/// times, model-swap when a single prompt is the dominant cost
/// centre, and skip when a Failed event consumed budget without
/// producing a successful outcome.
pub fn run_observe_cost_optimise(args: ObserveCostOptimiseArgs) -> Result<CostOptimisationReport> {
    let events = read_lineage_input(&args.trace_dir)?;
    let agent_events: Vec<&LineageEvent> = events
        .iter()
        .filter(|e| {
            (e.kind == LineageKind::Agent && e.name == args.agent)
                || matches!(
                    e.kind,
                    LineageKind::Prompt | LineageKind::Tool | LineageKind::Db
                )
        })
        .collect();
    let total_cost: f64 = agent_events.iter().map(|e| e.cost_usd).sum();

    // Group by (kind, name).
    let mut groups: BTreeMap<(String, String), (f64, u64, BTreeMap<String, u64>)> = BTreeMap::new();
    for event in &agent_events {
        let key = (
            format!("{:?}", event.kind).to_lowercase(),
            event.name.clone(),
        );
        let entry = groups.entry(key).or_insert((0.0, 0, BTreeMap::new()));
        entry.0 += event.cost_usd;
        entry.1 += 1;
        if !event.input_fingerprint.is_empty() {
            *entry
                .2
                .entry(event.input_fingerprint.clone())
                .or_insert(0) += 1;
        }
    }

    let mut centers: Vec<CostCenter> = groups
        .iter()
        .map(|((kind, name), (cost, count, _))| CostCenter {
            name: name.clone(),
            kind: kind.clone(),
            total_cost_usd: *cost,
            call_count: *count,
            percent_of_total: if total_cost > 0.0 {
                (cost / total_cost) * 100.0
            } else {
                0.0
            },
        })
        .collect();
    centers.sort_by(|a, b| {
        b.total_cost_usd
            .partial_cmp(&a.total_cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let top_n = args.top_n.max(1).min(centers.len().max(1));
    let top: Vec<CostCenter> = centers.into_iter().take(top_n).collect();

    // Suggestions: cache for repeated input fingerprints; skip for
    // failed events consuming cost; model-swap for the top center
    // if it dwarfs the rest.
    let mut suggestions = Vec::new();
    for ((kind, name), (cost, count, fingerprints)) in &groups {
        let max_repeat = fingerprints.values().max().copied().unwrap_or(0);
        if max_repeat >= 2 {
            let saved = cost * (max_repeat as f64 - 1.0) / max_repeat.max(1) as f64;
            suggestions.push(CostSuggestion {
                kind: "cache".to_string(),
                description: format!(
                    "`{}` (kind={}) fired {} times with the same input fingerprint; \
                     caching the response would save approximately ${saved:.4} of the \
                     ${cost:.4} this {} spent total",
                    name, kind, max_repeat, kind
                ),
                estimated_savings_usd: saved,
                sources: agent_events
                    .iter()
                    .filter(|e| {
                        e.name == *name
                            && format!("{:?}", e.kind).to_lowercase() == *kind
                    })
                    .take(3)
                    .map(|e| source_descriptor(e))
                    .collect(),
            });
        }
        if *count >= 1 && *cost > 0.0 {
            let failed_cost: f64 = agent_events
                .iter()
                .filter(|e| {
                    e.name == *name
                        && format!("{:?}", e.kind).to_lowercase() == *kind
                        && matches!(e.status, LineageStatus::Failed | LineageStatus::Denied)
                })
                .map(|e| e.cost_usd)
                .sum();
            if failed_cost > 0.0 {
                suggestions.push(CostSuggestion {
                    kind: "skip_pre_validate".to_string(),
                    description: format!(
                        "`{}` consumed ${failed_cost:.4} on failed/denied attempts; \
                         add a pre-validate step (cheap check) before invoking the \
                         expensive {kind}",
                        name
                    ),
                    estimated_savings_usd: failed_cost,
                    sources: agent_events
                        .iter()
                        .filter(|e| {
                            e.name == *name
                                && matches!(
                                    e.status,
                                    LineageStatus::Failed | LineageStatus::Denied
                                )
                        })
                        .take(3)
                        .map(|e| source_descriptor(e))
                        .collect(),
                });
            }
        }
    }
    if let Some(first) = top.first() {
        if first.percent_of_total > 50.0 && first.kind == "prompt" {
            suggestions.push(CostSuggestion {
                kind: "model_swap".to_string(),
                description: format!(
                    "`{}` is the dominant cost centre ({:.1}% of total); a smaller-model \
                     route via `@progressive(escalate_to: ...)` or a per-call \
                     `min_confidence` threshold could reduce spend",
                    first.name, first.percent_of_total
                ),
                estimated_savings_usd: first.total_cost_usd * 0.30,
                sources: agent_events
                    .iter()
                    .filter(|e| {
                        e.name == first.name
                            && format!("{:?}", e.kind).to_lowercase() == first.kind
                    })
                    .take(3)
                    .map(|e| source_descriptor(e))
                    .collect(),
            });
        }
    }

    let trace_count = agent_events
        .iter()
        .map(|e| e.trace_id.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let sources: Vec<Value> = agent_events
        .iter()
        .take(20)
        .map(|e| source_descriptor(e))
        .collect();

    Ok(CostOptimisationReport {
        agent: args.agent,
        trace_count,
        total_cost_usd: total_cost,
        top_cost_centers: top,
        suggestions,
        sources,
    })
}

// ---------------------------------------------------------------
// 3. eval drift --explain — model/input/prompt/index attribution
// ---------------------------------------------------------------

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

// ---------------------------------------------------------------
// 4. eval generate-from-feedback — synthesised eval fixture
// ---------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct EvalFromFeedbackArgs {
    pub trace_dir: PathBuf,
    pub feedback_file: PathBuf,
    pub out: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvalFixture {
    pub fixture_id: String,
    pub trace_id: String,
    pub feedback_kind: String,
    pub user_correction: String,
    pub redacted_lineage_count: usize,
    pub sources: Vec<Value>,
    pub redaction_policy: String,
    pub fixture_path: Option<PathBuf>,
}

/// Read a feedback record (a JSON file with `trace_id`,
/// `feedback_kind` ∈ {`wrong_answer`, `unsafe_action`,
/// `low_confidence`, …}, `user_correction`), look up the named
/// trace, redact PII via the production redaction policy, write a
/// `corvid eval promote`-shaped fixture (`.eval.json`).
pub fn run_eval_generate_from_feedback(
    args: EvalFromFeedbackArgs,
) -> Result<EvalFixture> {
    let feedback_text = fs::read_to_string(&args.feedback_file).with_context(|| {
        format!(
            "reading feedback record from `{}`",
            args.feedback_file.display()
        )
    })?;
    let feedback: Value = serde_json::from_str(&feedback_text)
        .with_context(|| "feedback file is not JSON")?;
    let trace_id = feedback
        .get("trace_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("feedback record must include `trace_id`"))?
        .to_string();
    let feedback_kind = feedback
        .get("feedback_kind")
        .and_then(|v| v.as_str())
        .unwrap_or("wrong_answer")
        .to_string();
    let user_correction = feedback
        .get("user_correction")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let events = read_lineage_input(&args.trace_dir)?;
    let run = select_run(&events, &trace_id);
    if run.is_empty() {
        return Err(anyhow!(
            "no lineage events found for trace `{}` referenced by feedback",
            trace_id
        ));
    }
    let policy = LineageRedactionPolicy::production_default();
    let redacted = redact_lineage_events(&run, &policy);

    let fixture_id = format!(
        "eval-from-feedback-{}-{}",
        feedback_kind,
        &trace_id.chars().take(12).collect::<String>()
    );
    let sources: Vec<Value> = redacted.iter().map(source_descriptor).collect();

    let fixture_path = if let Some(path) = &args.out {
        let body = json!({
            "fixture_id": fixture_id,
            "kind": "eval_from_feedback",
            "trace_id": trace_id,
            "feedback_kind": feedback_kind,
            "user_correction": user_correction,
            "redaction_policy": policy.name,
            "lineage_events": redacted,
            "sources": sources,
        });
        fs::write(path, serde_json::to_string_pretty(&body)?)
            .with_context(|| format!("writing eval fixture to `{}`", path.display()))?;
        Some(path.clone())
    } else {
        None
    };

    Ok(EvalFixture {
        fixture_id,
        trace_id,
        feedback_kind,
        user_correction,
        redacted_lineage_count: redacted.len(),
        sources,
        redaction_policy: policy.name,
        fixture_path,
    })
}

// ---------------------------------------------------------------
// Tests
// ---------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe_helpers_cmd::test_support::{ev, write_lineage};

    /// Slice 40K: cost-optimise computes correct percentages and
    /// suggests caching for repeated input fingerprints.
    #[test]
    fn cost_optimise_suggests_cache_for_repeated_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.lineage.jsonl");
        let mut e1 = ev(
            LineageKind::Prompt,
            "summarise",
            "t1",
            "s1",
            LineageStatus::Ok,
            "",
            0.20,
        );
        e1.input_fingerprint = "fp-1".to_string();
        let mut e2 = ev(
            LineageKind::Prompt,
            "summarise",
            "t2",
            "s2",
            LineageStatus::Ok,
            "",
            0.20,
        );
        e2.input_fingerprint = "fp-1".to_string(); // same input
        let mut e3 = ev(
            LineageKind::Tool,
            "fetch",
            "t1",
            "s3",
            LineageStatus::Ok,
            "",
            0.05,
        );
        e3.input_fingerprint = "fp-2".to_string();
        write_lineage(&path, &[e1, e2, e3]);
        let report = run_observe_cost_optimise(ObserveCostOptimiseArgs {
            trace_dir: dir.path().to_path_buf(),
            agent: "any".to_string(),
            top_n: 3,
        })
        .unwrap();
        assert!(report.total_cost_usd > 0.0);
        let cache_suggestions: Vec<_> = report
            .suggestions
            .iter()
            .filter(|s| s.kind == "cache")
            .collect();
        assert!(!cache_suggestions.is_empty());
        // The summarise prompt has the highest cost.
        assert_eq!(report.top_cost_centers[0].name, "summarise");
    }

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

    /// Slice 40K: eval generate-from-feedback reads a feedback
    /// record, redacts the matching trace, writes a typed fixture
    /// to disk, and the fixture's `sources` array carries the
    /// `(trace_id, span_id)` pairs of every redacted event.
    #[test]
    fn eval_generate_from_feedback_writes_redacted_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("trace.lineage.jsonl");
        write_lineage(
            &trace_path,
            &[ev(
                LineageKind::Prompt,
                "decide",
                "t1",
                "s1",
                LineageStatus::Ok,
                "",
                0.01,
            )],
        );
        let feedback_path = dir.path().join("feedback.json");
        fs::write(
            &feedback_path,
            r#"{"trace_id":"t1","feedback_kind":"wrong_answer","user_correction":"refund the order"}"#,
        )
        .unwrap();
        let out_path = dir.path().join("fixture.eval.json");
        let fixture = run_eval_generate_from_feedback(EvalFromFeedbackArgs {
            trace_dir: dir.path().to_path_buf(),
            feedback_file: feedback_path,
            out: Some(out_path.clone()),
        })
        .unwrap();
        assert_eq!(fixture.feedback_kind, "wrong_answer");
        assert_eq!(fixture.redacted_lineage_count, 1);
        assert!(out_path.exists());
        let written = fs::read_to_string(&out_path).unwrap();
        let parsed: Value = serde_json::from_str(&written).unwrap();
        assert_eq!(parsed["fixture_id"], fixture.fixture_id);
        assert_eq!(parsed["trace_id"], "t1");
        assert!(parsed["sources"].as_array().unwrap().len() == 1);
        // The redacted lineage must NOT contain the raw tenant id —
        // the production redaction policy hashes it.
        assert!(!written.contains("\"tenant_id\":\"t1\""));
    }

    /// Slice 40K adversarial: missing `trace_id` in the feedback
    /// record is refused with a clear diagnostic.
    #[test]
    fn eval_generate_from_feedback_missing_trace_id_refused() {
        let dir = tempfile::tempdir().unwrap();
        let feedback_path = dir.path().join("feedback.json");
        fs::write(&feedback_path, r#"{"feedback_kind":"wrong_answer"}"#).unwrap();
        let err = run_eval_generate_from_feedback(EvalFromFeedbackArgs {
            trace_dir: dir.path().to_path_buf(),
            feedback_file: feedback_path,
            out: None,
        })
        .unwrap_err();
        assert!(err.to_string().contains("trace_id"));
    }
}
