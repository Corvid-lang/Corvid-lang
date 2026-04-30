//! `corvid observe explain` / `corvid observe cost-optimise` /
//! `corvid eval drift` / `corvid eval generate-from-feedback`
//! CLI dispatch — slice 40K AI-helper surface, decomposed in
//! Phase 20j-A1.
//!
//! Each entry point is a thin wrapper around the matching
//! `crate::observe_helpers_cmd::run_*` heuristic; this module
//! owns only the JSON rendering of the typed output records.

use anyhow::Result;
use std::path::PathBuf;

use crate::observe_helpers_cmd;

pub(crate) fn cmd_observe_explain(trace_id: String, trace_dir: PathBuf) -> Result<u8> {
    let report = observe_helpers_cmd::run_observe_explain(
        observe_helpers_cmd::ObserveExplainArgs {
            trace_dir,
            trace_id,
        },
    )?;
    let body = serde_json::json!({
        "trace_id": report.trace_id,
        "root_cause_kind": report.root_cause_kind,
        "first_failed_event": report.first_failed_event.map(|e| serde_json::json!({
            "name": e.name,
            "kind": e.kind,
            "status": e.status,
            "guarantee_id": e.guarantee_id,
            "latency_ms": e.latency_ms,
            "cost_usd": e.cost_usd,
            "trace_id": e.trace_id,
            "span_id": e.span_id,
        })),
        "affected_guarantees": report.affected_guarantees,
        "suggested_next_steps": report.suggested_next_steps,
        "sources": report.sources,
    });
    println!("{}", serde_json::to_string_pretty(&body)?);
    Ok(0)
}

pub(crate) fn cmd_observe_cost_optimise(agent: String, trace_dir: PathBuf, top_n: usize) -> Result<u8> {
    let report = observe_helpers_cmd::run_observe_cost_optimise(
        observe_helpers_cmd::ObserveCostOptimiseArgs {
            trace_dir,
            agent,
            top_n,
        },
    )?;
    let body = serde_json::json!({
        "agent": report.agent,
        "trace_count": report.trace_count,
        "total_cost_usd": report.total_cost_usd,
        "top_cost_centers": report.top_cost_centers.iter().map(|c| serde_json::json!({
            "name": c.name,
            "kind": c.kind,
            "total_cost_usd": c.total_cost_usd,
            "call_count": c.call_count,
            "percent_of_total": c.percent_of_total,
        })).collect::<Vec<_>>(),
        "suggestions": report.suggestions.iter().map(|s| serde_json::json!({
            "kind": s.kind,
            "description": s.description,
            "estimated_savings_usd": s.estimated_savings_usd,
            "sources": s.sources,
        })).collect::<Vec<_>>(),
        "sources": report.sources,
    });
    println!("{}", serde_json::to_string_pretty(&body)?);
    Ok(0)
}

pub(crate) fn cmd_eval_drift(baseline: PathBuf, candidate: PathBuf, _explain: bool) -> Result<u8> {
    // The `--explain` flag is documented for parity with the
    // developer-flow doc; the helper output is always the
    // structured attribution, so the flag has no behavioural
    // change today.
    let report = observe_helpers_cmd::run_eval_drift_explain(
        observe_helpers_cmd::EvalDriftExplainArgs {
            baseline,
            candidate,
        },
    )?;
    let dim = |d: &observe_helpers_cmd::DriftDimension| -> serde_json::Value {
        serde_json::json!({
            "name": d.name,
            "changed_event_count": d.changed_event_count,
            "contribution_percent": d.contribution_percent,
            "example_changes": d.example_changes.iter().map(|x| serde_json::json!({
                "event_name": x.event_name,
                "baseline_value": x.baseline_value,
                "candidate_value": x.candidate_value,
                "baseline_source": x.baseline_source,
                "candidate_source": x.candidate_source,
            })).collect::<Vec<_>>(),
        })
    };
    let body = serde_json::json!({
        "baseline": report.baseline,
        "candidate": report.candidate,
        "events_compared": report.events_compared,
        "model_drift": dim(&report.model_drift),
        "prompt_drift": dim(&report.prompt_drift),
        "retrieval_index_drift": dim(&report.retrieval_index_drift),
        "input_drift": dim(&report.input_drift),
        "residual_percent": report.residual_percent,
        "sources": report.sources,
    });
    println!("{}", serde_json::to_string_pretty(&body)?);
    Ok(0)
}

pub(crate) fn cmd_eval_from_feedback(
    feedback: PathBuf,
    trace_dir: PathBuf,
    out: Option<PathBuf>,
) -> Result<u8> {
    let fixture = observe_helpers_cmd::run_eval_generate_from_feedback(
        observe_helpers_cmd::EvalFromFeedbackArgs {
            trace_dir,
            feedback_file: feedback,
            out,
        },
    )?;
    let body = serde_json::json!({
        "fixture_id": fixture.fixture_id,
        "trace_id": fixture.trace_id,
        "feedback_kind": fixture.feedback_kind,
        "user_correction": fixture.user_correction,
        "redacted_lineage_count": fixture.redacted_lineage_count,
        "sources": fixture.sources,
        "redaction_policy": fixture.redaction_policy,
        "fixture_path": fixture.fixture_path,
    });
    println!("{}", serde_json::to_string_pretty(&body)?);
    Ok(0)
}
