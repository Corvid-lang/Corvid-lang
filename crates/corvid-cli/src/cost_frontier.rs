//! `corvid cost-frontier` cost / quality Pareto analysis.
//!
//! Cost comes from `model_selected.cost_estimate` trace events. Quality comes
//! from explicit eval host events, so this command never fabricates a quality
//! score from routing usage alone.

use anyhow::{bail, Context, Result};
use corvid_runtime::TraceEvent;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use std::process::Command;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Debug, Clone)]
pub struct CostFrontierOptions<'a> {
    pub prompt: &'a str,
    pub trace_dir: &'a Path,
    pub since: Option<&'a str>,
    pub since_commit: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CostFrontierReport {
    pub prompt: String,
    pub trace_dir: String,
    pub since_ms: Option<u64>,
    pub has_quality_evidence: bool,
    pub candidates: Vec<FrontierCandidate>,
    pub pareto_optimal: Vec<String>,
    pub dominated: Vec<String>,
    pub unscored: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FrontierCandidate {
    pub model: String,
    pub calls: u64,
    pub eval_samples: u64,
    pub mean_cost: f64,
    pub quality: Option<f64>,
    pub status: FrontierStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrontierStatus {
    ParetoOptimal,
    Dominated,
    Unscored,
}

#[derive(Default)]
struct CandidateAgg {
    calls: u64,
    total_cost: f64,
    eval_samples: u64,
    quality_sum: f64,
}

pub fn build_frontier(opts: CostFrontierOptions<'_>) -> Result<CostFrontierReport> {
    let since_ms = resolve_since_ms(opts.since, opts.since_commit)?;
    let events = load_trace_events(opts.trace_dir, since_ms)?;
    let mut aggs: HashMap<String, CandidateAgg> = HashMap::new();

    for event in events {
        match event {
            TraceEvent::ModelSelected {
                prompt,
                model,
                model_version,
                cost_estimate,
                ..
            } if prompt == opts.prompt => {
                let row = aggs.entry(model_label(&model, model_version.as_deref())).or_default();
                row.calls += 1;
                if cost_estimate.is_finite() && cost_estimate >= 0.0 {
                    row.total_cost += cost_estimate;
                }
            }
            TraceEvent::HostEvent { name, payload, .. } if is_eval_quality_event(&name) => {
                let Some(prompt) = payload.get("prompt").and_then(|v| v.as_str()) else {
                    continue;
                };
                if prompt != opts.prompt {
                    continue;
                }
                let Some(model) = payload.get("model").and_then(|v| v.as_str()) else {
                    continue;
                };
                let version = payload.get("model_version").and_then(|v| v.as_str());
                let Some(quality) = quality_from_payload(&payload)? else {
                    continue;
                };
                let row = aggs.entry(model_label(model, version)).or_default();
                row.eval_samples += 1;
                row.quality_sum += quality;
                if let Some(cost) = payload.get("cost_usd").and_then(|v| v.as_f64()) {
                    if cost.is_finite() && cost >= 0.0 {
                        row.calls += 1;
                        row.total_cost += cost;
                    }
                }
            }
            _ => {}
        }
    }

    let scored_snapshot: BTreeMap<String, (f64, f64)> = aggs
        .iter()
        .filter_map(|(model, agg)| {
            if agg.eval_samples == 0 {
                return None;
            }
            let cost = mean_cost(agg);
            let quality = agg.quality_sum / agg.eval_samples as f64;
            Some((model.clone(), (cost, quality)))
        })
        .collect();

    let mut candidates: Vec<FrontierCandidate> = aggs
        .into_iter()
        .map(|(model, agg)| {
            let quality = (agg.eval_samples > 0)
                .then(|| agg.quality_sum / agg.eval_samples as f64);
            let status = match quality {
                None => FrontierStatus::Unscored,
                Some(_) if is_dominated(&model, &scored_snapshot) => FrontierStatus::Dominated,
                Some(_) => FrontierStatus::ParetoOptimal,
            };
            FrontierCandidate {
                model,
                calls: agg.calls,
                eval_samples: agg.eval_samples,
                mean_cost: mean_cost(&agg),
                quality,
                status,
            }
        })
        .collect();
    candidates.sort_by(|a, b| {
        a.mean_cost
            .partial_cmp(&b.mean_cost)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.quality.partial_cmp(&a.quality).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| a.model.cmp(&b.model))
    });

    let pareto_optimal = candidates
        .iter()
        .filter(|row| row.status == FrontierStatus::ParetoOptimal)
        .map(|row| row.model.clone())
        .collect::<Vec<_>>();
    let dominated = candidates
        .iter()
        .filter(|row| row.status == FrontierStatus::Dominated)
        .map(|row| row.model.clone())
        .collect::<Vec<_>>();
    let unscored = candidates
        .iter()
        .filter(|row| row.status == FrontierStatus::Unscored)
        .map(|row| row.model.clone())
        .collect::<Vec<_>>();

    Ok(CostFrontierReport {
        prompt: opts.prompt.to_string(),
        trace_dir: opts.trace_dir.display().to_string(),
        since_ms,
        has_quality_evidence: !pareto_optimal.is_empty() || !dominated.is_empty(),
        candidates,
        pareto_optimal,
        dominated,
        unscored,
    })
}

pub fn render_frontier(report: &CostFrontierReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("Cost-quality frontier for `{}`\n\n", report.prompt));
    out.push_str("model                calls  eval_n  mean_cost  quality  status\n");
    out.push_str("-------------------- ------ ------- ---------- -------- ----------------\n");
    for row in &report.candidates {
        out.push_str(&format!(
            "{:<20} {:>6} {:>7} {:>10.6} {:>8} {}\n",
            row.model,
            row.calls,
            row.eval_samples,
            row.mean_cost,
            row.quality
                .map(|q| format!("{:.1}%", q * 100.0))
                .unwrap_or_else(|| "n/a".to_string()),
            match row.status {
                FrontierStatus::ParetoOptimal => "pareto-optimal",
                FrontierStatus::Dominated => "dominated",
                FrontierStatus::Unscored => "unscored",
            }
        ));
    }

    if report.has_quality_evidence {
        out.push_str("\nPareto-optimal: ");
        out.push_str(&format_list(&report.pareto_optimal));
        out.push_str("\nDominated:      ");
        out.push_str(&format_list(&report.dominated));
        if !report.unscored.is_empty() {
            out.push_str("\nUnscored:       ");
            out.push_str(&format_list(&report.unscored));
            out.push_str(" (missing eval-quality host events)");
        }
        out.push('\n');
    } else {
        out.push_str(
            "\nNo quality evidence found. Emit host events named `corvid.eval.result` \
             with {prompt, model, passed|correct|score} to compute the frontier.\n",
        );
    }
    out
}

fn is_eval_quality_event(name: &str) -> bool {
    matches!(
        name,
        "corvid.eval.result" | "eval_result" | "eval.result" | "quality_result"
    )
}

fn quality_from_payload(payload: &serde_json::Value) -> Result<Option<f64>> {
    if let Some(passed) = payload
        .get("passed")
        .or_else(|| payload.get("correct"))
        .and_then(|v| v.as_bool())
    {
        return Ok(Some(if passed { 1.0 } else { 0.0 }));
    }
    if let Some(score) = payload
        .get("score")
        .or_else(|| payload.get("quality"))
        .and_then(|v| v.as_f64())
    {
        if !(0.0..=1.0).contains(&score) || !score.is_finite() {
            bail!("eval quality score must be finite and in [0.0, 1.0]");
        }
        return Ok(Some(score));
    }
    Ok(None)
}

fn is_dominated(model: &str, scored: &BTreeMap<String, (f64, f64)>) -> bool {
    let Some((cost, quality)) = scored.get(model) else {
        return false;
    };
    scored.iter().any(|(other, (other_cost, other_quality))| {
        other != model
            && *other_cost <= *cost
            && *other_quality >= *quality
            && (*other_cost < *cost || *other_quality > *quality)
    })
}

fn mean_cost(agg: &CandidateAgg) -> f64 {
    if agg.calls == 0 {
        0.0
    } else {
        agg.total_cost / agg.calls as f64
    }
}

fn format_list(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(", ")
    }
}

fn model_label(model: &str, version: Option<&str>) -> String {
    match version {
        Some(version) if !version.is_empty() => format!("{model}@{version}"),
        _ => model.to_string(),
    }
}

fn load_trace_events(trace_dir: &Path, since_ms: Option<u64>) -> Result<Vec<TraceEvent>> {
    let mut events = Vec::new();
    for entry in fs::read_dir(trace_dir)
        .with_context(|| format!("failed to read trace dir `{}`", trace_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let body = fs::read_to_string(&path)
            .with_context(|| format!("failed to read trace file `{}`", path.display()))?;
        for (idx, line) in body.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let event: TraceEvent = serde_json::from_str(line).with_context(|| {
                format!("invalid trace event at {}:{}", path.display(), idx + 1)
            })?;
            if since_ms.map(|cutoff| event_ts_ms(&event) >= cutoff).unwrap_or(true) {
                events.push(event);
            }
        }
    }
    events.sort_by_key(event_ts_ms);
    Ok(events)
}

fn resolve_since_ms(since: Option<&str>, since_commit: Option<&str>) -> Result<Option<u64>> {
    let mut cutoffs = Vec::new();
    if let Some(since) = since {
        let ts = OffsetDateTime::parse(since, &Rfc3339)
            .with_context(|| format!("invalid --since timestamp `{since}`; expected RFC3339"))?;
        cutoffs.push((ts.unix_timestamp_nanos() / 1_000_000) as u64);
    }
    if let Some(sha) = since_commit {
        let output = Command::new("git")
            .args(["show", "-s", "--format=%ct", sha])
            .output()
            .with_context(|| format!("failed to run `git show` for `{sha}`"))?;
        if !output.status.success() {
            bail!("failed to resolve commit `{sha}`");
        }
        let secs = String::from_utf8(output.stdout)
            .context("git returned non-utf8 commit timestamp")?
            .trim()
            .parse::<u64>()
            .with_context(|| format!("invalid git timestamp for `{sha}`"))?;
        cutoffs.push(secs * 1000);
    }
    Ok(cutoffs.into_iter().max())
}

fn event_ts_ms(event: &TraceEvent) -> u64 {
    match event {
        TraceEvent::SchemaHeader { ts_ms, .. }
        | TraceEvent::RunStarted { ts_ms, .. }
        | TraceEvent::RunCompleted { ts_ms, .. }
        | TraceEvent::ToolCall { ts_ms, .. }
        | TraceEvent::ToolResult { ts_ms, .. }
        | TraceEvent::LlmCall { ts_ms, .. }
        | TraceEvent::LlmResult { ts_ms, .. }
        | TraceEvent::PromptCache { ts_ms, .. }
        | TraceEvent::ApprovalRequest { ts_ms, .. }
        | TraceEvent::ApprovalDecision { ts_ms, .. }
        | TraceEvent::ApprovalResponse { ts_ms, .. }
        | TraceEvent::HostEvent { ts_ms, .. }
        | TraceEvent::SeedRead { ts_ms, .. }
        | TraceEvent::ClockRead { ts_ms, .. }
        | TraceEvent::ModelSelected { ts_ms, .. }
        | TraceEvent::ProgressiveEscalation { ts_ms, .. }
        | TraceEvent::ProgressiveExhausted { ts_ms, .. }
        | TraceEvent::StreamUpgrade { ts_ms, .. }
        | TraceEvent::AbVariantChosen { ts_ms, .. }
        | TraceEvent::EnsembleVote { ts_ms, .. }
        | TraceEvent::AdversarialPipelineCompleted { ts_ms, .. }
        | TraceEvent::AdversarialContradiction { ts_ms, .. }
        | TraceEvent::ProvenanceEdge { ts_ms, .. } => *ts_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::{build_frontier, render_frontier, CostFrontierOptions, FrontierStatus};
    use std::fs;

    #[test]
    fn computes_pareto_frontier_from_cost_and_eval_events() {
        let temp = tempfile::tempdir().expect("tempdir");
        let trace = temp.path().join("run.jsonl");
        fs::write(
            &trace,
            r#"{"kind":"model_selected","ts_ms":1,"run_id":"r","prompt":"answer","model":"cheap","cost_estimate":0.01}
{"kind":"host_event","ts_ms":2,"run_id":"r","name":"corvid.eval.result","payload":{"prompt":"answer","model":"cheap","passed":true}}
{"kind":"host_event","ts_ms":3,"run_id":"r","name":"corvid.eval.result","payload":{"prompt":"answer","model":"cheap","passed":false}}
{"kind":"model_selected","ts_ms":4,"run_id":"r","prompt":"answer","model":"strong","cost_estimate":0.10}
{"kind":"host_event","ts_ms":5,"run_id":"r","name":"corvid.eval.result","payload":{"prompt":"answer","model":"strong","score":0.95}}
{"kind":"model_selected","ts_ms":6,"run_id":"r","prompt":"answer","model":"wasteful","cost_estimate":0.20}
{"kind":"host_event","ts_ms":7,"run_id":"r","name":"corvid.eval.result","payload":{"prompt":"answer","model":"wasteful","score":0.60}}
"#,
        )
        .expect("write trace");

        let report = build_frontier(CostFrontierOptions {
            prompt: "answer",
            trace_dir: temp.path(),
            since: None,
            since_commit: None,
        })
        .expect("frontier");

        assert!(report.has_quality_evidence);
        assert_eq!(report.pareto_optimal, vec!["cheap", "strong"]);
        assert_eq!(report.dominated, vec!["wasteful"]);
        assert_eq!(
            report
                .candidates
                .iter()
                .find(|row| row.model == "wasteful")
                .map(|row| row.status),
            Some(FrontierStatus::Dominated)
        );
        let rendered = render_frontier(&report);
        assert!(rendered.contains("Pareto-optimal: cheap, strong"));
        assert!(rendered.contains("Dominated:      wasteful"));
    }

    #[test]
    fn reports_missing_quality_without_inventing_scores() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(
            temp.path().join("run.jsonl"),
            r#"{"kind":"model_selected","ts_ms":1,"run_id":"r","prompt":"answer","model":"cheap","cost_estimate":0.01}
"#,
        )
        .expect("write trace");

        let report = build_frontier(CostFrontierOptions {
            prompt: "answer",
            trace_dir: temp.path(),
            since: None,
            since_commit: None,
        })
        .expect("frontier");

        assert!(!report.has_quality_evidence);
        assert_eq!(report.unscored, vec!["cheap"]);
        assert!(render_frontier(&report).contains("No quality evidence found"));
    }
}
