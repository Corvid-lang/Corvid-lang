//! `corvid observe list` build + render.
//!
//! Walks `target/trace/*.lineage.jsonl`, summarizes each run
//! into an `ObservedRun` (trace id, root, event/failure/denial
//! counts, total cost, hot-spot latency), and renders a
//! pipe-separated table sorted by recency.
//!
//! `summarize_run` is the per-file digest the build pass calls
//! once for each lineage trace it finds.

use std::fs;
use std::path::Path;
use std::path::PathBuf;

use anyhow::{Context, Result};

use corvid_runtime::{LineageEvent, LineageKind, LineageStatus};

use super::{
    is_lineage_jsonl, kind_name, read_lineage_events, ObserveListReport, ObservedRun,
};

pub fn build_observe_list(trace_dir: &Path) -> Result<ObserveListReport> {
    let mut runs = Vec::new();
    if trace_dir.exists() {
        for entry in fs::read_dir(trace_dir)
            .with_context(|| format!("reading trace directory `{}`", trace_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if !is_lineage_jsonl(&path) {
                continue;
            }
            let events = read_lineage_events(&path)
                .with_context(|| format!("reading lineage trace `{}`", path.display()))?;
            if events.is_empty() {
                continue;
            }
            runs.push(summarize_run(path, &events));
        }
    }

    runs.sort_by(|left, right| {
        right
            .started_ms
            .cmp(&left.started_ms)
            .then_with(|| left.trace_id.cmp(&right.trace_id))
    });
    let total_cost_usd = runs.iter().map(|run| run.cost_usd).sum();
    let total_failures = runs.iter().map(|run| run.failure_count).sum();
    let total_denials = runs.iter().map(|run| run.approval_denied_count).sum();
    Ok(ObserveListReport {
        trace_dir: trace_dir.to_path_buf(),
        runs,
        total_cost_usd,
        total_failures,
        total_denials,
    })
}

pub fn render_observe_list(report: &ObserveListReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "observe traces: {} | runs={} cost=${:.6} failures={} denials={}\n",
        report.trace_dir.display(),
        report.runs.len(),
        report.total_cost_usd,
        report.total_failures,
        report.total_denials
    ));
    if report.runs.is_empty() {
        out.push_str("no lineage traces found\n");
        return out;
    }

    out.push_str(
        "trace_id | root | events | duration_ms | cost_usd | tokens | failures | approvals | hot_spot\n",
    );
    for run in &report.runs {
        out.push_str(&format!(
            "{} | {} | {} | {} | {:.6} | {} | {} | {}:{}:{} | {}:{}ms\n",
            run.trace_id,
            run.root_name,
            run.event_count,
            run.duration_ms,
            run.cost_usd,
            run.tokens,
            run.failure_count,
            run.approval_count,
            run.approval_denied_count,
            run.approval_pending_count,
            run.hot_spot,
            run.hot_spot_latency_ms
        ));
    }
    out
}

pub(super) fn summarize_run(path: PathBuf, events: &[LineageEvent]) -> ObservedRun {
    let root = events
        .iter()
        .find(|event| event.parent_span_id.is_empty())
        .unwrap_or(&events[0]);
    let started_ms = events
        .iter()
        .map(|event| event.started_ms)
        .min()
        .unwrap_or(root.started_ms);
    let ended_ms = events
        .iter()
        .map(|event| event.ended_ms)
        .max()
        .unwrap_or(root.ended_ms);
    let hot_spot_event = events
        .iter()
        .max_by(|left, right| {
            left.latency_ms
                .cmp(&right.latency_ms)
                .then_with(|| left.name.cmp(&right.name))
        })
        .unwrap_or(root);
    let failure_count = events
        .iter()
        .filter(|event| event.status == LineageStatus::Failed)
        .count() as u64;
    let approval_count = events
        .iter()
        .filter(|event| event.kind == LineageKind::Approval)
        .count() as u64;
    let approval_denied_count = events
        .iter()
        .filter(|event| {
            event.kind == LineageKind::Approval && event.status == LineageStatus::Denied
        })
        .count() as u64;
    let approval_pending_count = events
        .iter()
        .filter(|event| {
            event.kind == LineageKind::Approval && event.status == LineageStatus::PendingReview
        })
        .count() as u64;
    ObservedRun {
        trace_id: root.trace_id.clone(),
        path,
        root_name: root.name.clone(),
        started_ms,
        ended_ms,
        duration_ms: ended_ms.saturating_sub(started_ms),
        event_count: events.len(),
        failure_count,
        approval_count,
        approval_denied_count,
        approval_pending_count,
        cost_usd: events
            .iter()
            .map(|event| event.cost_usd)
            .filter(|cost| cost.is_finite() && *cost > 0.0)
            .sum(),
        tokens: events
            .iter()
            .map(|event| event.tokens_in + event.tokens_out)
            .sum(),
        hot_spot: format!("{}:{}", kind_name(hot_spot_event.kind), hot_spot_event.name),
        hot_spot_latency_ms: hot_spot_event.latency_ms,
    }
}
