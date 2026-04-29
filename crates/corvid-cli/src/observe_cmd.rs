use anyhow::{Context, Result};
use corvid_runtime::{LineageEvent, LineageKind, LineageStatus};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct ObserveListReport {
    pub trace_dir: PathBuf,
    pub runs: Vec<ObservedRun>,
    pub total_cost_usd: f64,
    pub total_failures: u64,
    pub total_denials: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObservedRun {
    pub trace_id: String,
    pub path: PathBuf,
    pub root_name: String,
    pub started_ms: u64,
    pub ended_ms: u64,
    pub duration_ms: u64,
    pub event_count: usize,
    pub failure_count: u64,
    pub approval_count: u64,
    pub approval_denied_count: u64,
    pub approval_pending_count: u64,
    pub cost_usd: f64,
    pub tokens: u64,
    pub hot_spot: String,
    pub hot_spot_latency_ms: u64,
}

pub fn run_list(trace_dir: Option<&Path>) -> Result<u8> {
    let trace_dir = trace_dir.unwrap_or_else(|| Path::new("target/trace"));
    let report = build_observe_list(trace_dir)?;
    print!("{}", render_observe_list(&report));
    Ok(0)
}

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

fn summarize_run(path: PathBuf, events: &[LineageEvent]) -> ObservedRun {
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

fn read_lineage_events(path: &Path) -> Result<Vec<LineageEvent>> {
    let text = fs::read_to_string(path)?;
    let mut events = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let event: LineageEvent = serde_json::from_str(trimmed)
            .with_context(|| format!("line {} is not a lineage event", index + 1))?;
        events.push(event);
    }
    Ok(events)
}

fn is_lineage_jsonl(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.ends_with(".lineage.jsonl"))
        .unwrap_or(false)
}

fn kind_name(kind: LineageKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| format!("{kind:?}").to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_lineage(path: &Path, events: &[LineageEvent]) {
        let body = events
            .iter()
            .map(|event| serde_json::to_string(event).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(path, format!("{body}\n")).unwrap();
    }

    #[test]
    fn list_summarizes_cost_failures_approvals_and_hot_spots() {
        let dir = tempfile::tempdir().unwrap();
        let mut route = LineageEvent::root("trace-1", LineageKind::Route, "POST /send", 10)
            .finish(LineageStatus::Ok, 100);
        route.cost_usd = 0.01;
        let mut prompt = LineageEvent::child(&route, LineageKind::Prompt, "draft", 0, 20)
            .finish(LineageStatus::Ok, 70);
        prompt.cost_usd = 0.04;
        prompt.tokens_in = 100;
        prompt.tokens_out = 20;
        let approval = LineageEvent::child(&route, LineageKind::Approval, "SendEmail", 1, 72)
            .finish(LineageStatus::Denied, 88);
        let tool = LineageEvent::child(&route, LineageKind::Tool, "send_email", 2, 89)
            .finish(LineageStatus::Failed, 96);
        write_lineage(
            &dir.path().join("trace-1.lineage.jsonl"),
            &[route, prompt, approval, tool],
        );

        let report = build_observe_list(dir.path()).unwrap();
        assert_eq!(report.runs.len(), 1);
        let run = &report.runs[0];
        assert_eq!(run.trace_id, "trace-1");
        assert_eq!(run.event_count, 4);
        assert_eq!(run.failure_count, 1);
        assert_eq!(run.approval_count, 1);
        assert_eq!(run.approval_denied_count, 1);
        assert_eq!(run.tokens, 120);
        assert_eq!(run.hot_spot, "route:POST /send");
        assert_eq!(run.hot_spot_latency_ms, 90);
        assert!((run.cost_usd - 0.05).abs() < f64::EPSILON);
        assert!(render_observe_list(&report).contains("failures=1"));
    }

    #[test]
    fn list_ignores_non_lineage_files_and_reports_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("plain.jsonl"), "{}\n").unwrap();

        let report = build_observe_list(dir.path()).unwrap();
        assert!(report.runs.is_empty());
        assert!(render_observe_list(&report).contains("no lineage traces found"));
    }
}
