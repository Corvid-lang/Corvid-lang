use anyhow::{Context, Result};
use corvid_runtime::{
    compute_lineage_drift_report, LineageDriftReport, LineageEvent, LineageKind, LineageStatus,
};
use std::collections::BTreeMap;
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

#[derive(Debug, Clone, PartialEq)]
pub struct ObserveShowReport {
    pub path: PathBuf,
    pub run: ObservedRun,
    pub events: Vec<ObservedEvent>,
    pub guarantee_groups: Vec<ObservedGroup>,
    pub effect_groups: Vec<ObservedGroup>,
    pub data_class_groups: Vec<ObservedGroup>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedEvent {
    pub kind: String,
    pub name: String,
    pub status: String,
    pub latency_ms: u64,
    pub cost_usd: String,
    pub guarantee_id: String,
    pub approval_id: String,
    pub replay_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedGroup {
    pub key: String,
    pub count: u64,
    pub failures: u64,
    pub denials: u64,
    pub cost_usd: String,
}

pub fn run_list(trace_dir: Option<&Path>) -> Result<u8> {
    let trace_dir = trace_dir.unwrap_or_else(|| Path::new("target/trace"));
    let report = build_observe_list(trace_dir)?;
    print!("{}", render_observe_list(&report));
    Ok(0)
}

pub fn run_show(id_or_path: &str, trace_dir: Option<&Path>) -> Result<u8> {
    let report = build_observe_show(id_or_path, trace_dir)?;
    print!("{}", render_observe_show(&report));
    Ok(0)
}

pub fn run_drift(baseline: &Path, candidate: &Path, json: bool) -> Result<u8> {
    let baseline_events = read_lineage_input(baseline)
        .with_context(|| format!("reading baseline lineage input `{}`", baseline.display()))?;
    let candidate_events = read_lineage_input(candidate)
        .with_context(|| format!("reading candidate lineage input `{}`", candidate.display()))?;
    let report = compute_lineage_drift_report(&baseline_events, &candidate_events);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).context("serializing drift report")?
        );
    } else {
        print!("{}", render_drift_report(&report));
    }
    Ok(drift_exit_code(&report))
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

pub fn build_observe_show(id_or_path: &str, trace_dir: Option<&Path>) -> Result<ObserveShowReport> {
    let path = resolve_lineage_path(id_or_path, trace_dir);
    let events = read_lineage_events(&path)
        .with_context(|| format!("reading lineage trace `{}`", path.display()))?;
    if events.is_empty() {
        anyhow::bail!("lineage trace `{}` is empty", path.display());
    }
    let run = summarize_run(path.clone(), &events);
    Ok(ObserveShowReport {
        path,
        run,
        events: events.iter().map(observed_event).collect(),
        guarantee_groups: build_groups(&events, GroupKind::Guarantee),
        effect_groups: build_groups(&events, GroupKind::Effect),
        data_class_groups: build_groups(&events, GroupKind::DataClass),
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

pub fn render_observe_show(report: &ObserveShowReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "observe show: {} | trace={} root={} duration_ms={} cost=${:.6} failures={} approvals={} denials={}\n",
        report.path.display(),
        report.run.trace_id,
        report.run.root_name,
        report.run.duration_ms,
        report.run.cost_usd,
        report.run.failure_count,
        report.run.approval_count,
        report.run.approval_denied_count
    ));
    out.push_str(&format!(
        "hot_spot: {}:{}ms | tokens={} | replayable_events={}\n",
        report.run.hot_spot,
        report.run.hot_spot_latency_ms,
        report.run.tokens,
        report
            .events
            .iter()
            .filter(|event| !event.replay_key.is_empty())
            .count()
    ));
    render_groups(&mut out, "guarantees", &report.guarantee_groups);
    render_groups(&mut out, "effects", &report.effect_groups);
    render_groups(&mut out, "data_classes", &report.data_class_groups);
    out.push_str("events:\n");
    for event in &report.events {
        out.push_str(&format!(
            "- {}:{} status={} latency_ms={} cost_usd={} guarantee={} approval={} replay={}\n",
            event.kind,
            event.name,
            event.status,
            event.latency_ms,
            event.cost_usd,
            display_optional(&event.guarantee_id),
            display_optional(&event.approval_id),
            display_optional(&event.replay_key)
        ));
    }
    out
}

pub fn render_drift_report(report: &LineageDriftReport) -> String {
    let mut out = String::new();
    out.push_str("corvid observe drift\n");
    out.push_str(&format!(
        "events: baseline={} candidate={} delta={}\n",
        report.baseline.event_count,
        report.candidate.event_count,
        report.candidate.event_count as i128 - report.baseline.event_count as i128
    ));
    out.push_str(&format!(
        "schema_violations: baseline={} candidate={} delta={}\n",
        report.baseline.schema_violation_count,
        report.candidate.schema_violation_count,
        report.schema_violation_delta
    ));
    out.push_str(&format!(
        "cost_usd: baseline={:.6} candidate={:.6} delta={:.6}\n",
        report.baseline.total_cost_usd, report.candidate.total_cost_usd, report.cost_delta_usd
    ));
    out.push_str(&format!(
        "latency_ms: baseline={} candidate={} delta={}\n",
        report.baseline.total_latency_ms,
        report.candidate.total_latency_ms,
        report.latency_delta_ms
    ));
    out.push_str(&format!(
        "denials: baseline={} candidate={} delta={}\n",
        report.baseline.denial_count, report.candidate.denial_count, report.denial_delta
    ));
    out.push_str(&format!(
        "tool_errors: baseline={} candidate={} delta={}\n",
        report.baseline.tool_error_count,
        report.candidate.tool_error_count,
        report.tool_error_delta
    ));
    out.push_str(&format!(
        "confidence: baseline={:.6} candidate={:.6} delta={:.6}\n",
        report.baseline.average_confidence,
        report.candidate.average_confidence,
        report.confidence_delta
    ));
    out.push_str(&format!(
        "verdict: {}\n",
        if drift_exit_code(report) == 0 {
            "stable"
        } else {
            "drift"
        }
    ));
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

fn observed_event(event: &LineageEvent) -> ObservedEvent {
    ObservedEvent {
        kind: kind_name(event.kind),
        name: event.name.clone(),
        status: status_name(event.status),
        latency_ms: event.latency_ms,
        cost_usd: format!("{:.6}", finite_cost(event.cost_usd)),
        guarantee_id: event.guarantee_id.clone(),
        approval_id: event.approval_id.clone(),
        replay_key: event.replay_key.clone(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupKind {
    Guarantee,
    Effect,
    DataClass,
}

#[derive(Debug, Clone, Default)]
struct GroupAccumulator {
    count: u64,
    failures: u64,
    denials: u64,
    cost_usd: f64,
}

fn build_groups(events: &[LineageEvent], kind: GroupKind) -> Vec<ObservedGroup> {
    let mut groups: BTreeMap<String, GroupAccumulator> = BTreeMap::new();
    for event in events {
        let keys = match kind {
            GroupKind::Guarantee => single_group_key(&event.guarantee_id),
            GroupKind::Effect => event.effect_ids.clone(),
            GroupKind::DataClass => event.data_classes.clone(),
        };
        for key in keys {
            let group = groups.entry(key).or_default();
            group.count += 1;
            if event.status == LineageStatus::Failed {
                group.failures += 1;
            }
            if event.status == LineageStatus::Denied {
                group.denials += 1;
            }
            group.cost_usd += finite_cost(event.cost_usd);
        }
    }
    groups
        .into_iter()
        .map(|(key, group)| ObservedGroup {
            key,
            count: group.count,
            failures: group.failures,
            denials: group.denials,
            cost_usd: format!("{:.6}", group.cost_usd),
        })
        .collect()
}

fn render_groups(out: &mut String, label: &str, groups: &[ObservedGroup]) {
    out.push_str(&format!("{label}:\n"));
    if groups.is_empty() {
        out.push_str("- none\n");
        return;
    }
    for group in groups {
        out.push_str(&format!(
            "- {} count={} failures={} denials={} cost_usd={}\n",
            group.key, group.count, group.failures, group.denials, group.cost_usd
        ));
    }
}

fn single_group_key(value: &str) -> Vec<String> {
    if value.is_empty() {
        Vec::new()
    } else {
        vec![value.to_string()]
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

fn read_lineage_input(path: &Path) -> Result<Vec<LineageEvent>> {
    if path.is_dir() {
        let mut events = Vec::new();
        let mut paths = fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        paths.sort();
        for path in paths {
            if is_lineage_jsonl(&path) {
                events.extend(read_lineage_events(&path)?);
            }
        }
        return Ok(events);
    }
    read_lineage_events(path)
}

fn resolve_lineage_path(id_or_path: &str, trace_dir: Option<&Path>) -> PathBuf {
    let direct = PathBuf::from(id_or_path);
    if direct.exists() || direct.extension().is_some() {
        return direct;
    }
    trace_dir
        .unwrap_or_else(|| Path::new("target/trace"))
        .join(format!("{id_or_path}.lineage.jsonl"))
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

fn status_name(status: LineageStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| format!("{status:?}").to_lowercase())
}

fn finite_cost(value: f64) -> f64 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}

fn display_optional(value: &str) -> &str {
    if value.is_empty() {
        "-"
    } else {
        value
    }
}

fn drift_exit_code(report: &LineageDriftReport) -> u8 {
    let regressed = report.schema_violation_delta > 0
        || report.denial_delta > 0
        || report.tool_error_delta > 0
        || report.cost_delta_usd > 0.0
        || report.latency_delta_ms > 0
        || report.confidence_delta < 0.0;
    u8::from(regressed)
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

    #[test]
    fn show_groups_one_run_by_guarantee_effect_and_data_class() {
        let dir = tempfile::tempdir().unwrap();
        let mut route = LineageEvent::root("trace-1", LineageKind::Route, "POST /send", 10)
            .finish(LineageStatus::Ok, 100);
        route.replay_key = "route:trace-1".to_string();
        let mut tool = LineageEvent::child(&route, LineageKind::Tool, "send_email", 0, 20)
            .finish(LineageStatus::Failed, 90);
        tool.guarantee_id = "approval.reachable_entrypoints_require_contract".to_string();
        tool.effect_ids = vec!["send_email".to_string()];
        tool.data_classes = vec!["private".to_string()];
        tool.approval_id = "approval-1".to_string();
        tool.cost_usd = 0.02;
        let mut approval = LineageEvent::child(&tool, LineageKind::Approval, "SendEmail", 0, 30)
            .finish(LineageStatus::Denied, 40);
        approval.guarantee_id = tool.guarantee_id.clone();
        approval.effect_ids = tool.effect_ids.clone();
        approval.data_classes = tool.data_classes.clone();
        approval.approval_id = tool.approval_id.clone();
        write_lineage(
            &dir.path().join("trace-1.lineage.jsonl"),
            &[route, tool, approval],
        );

        let report = build_observe_show("trace-1", Some(dir.path())).unwrap();
        assert_eq!(report.run.failure_count, 1);
        assert_eq!(report.guarantee_groups.len(), 1);
        assert_eq!(report.guarantee_groups[0].count, 2);
        assert_eq!(report.guarantee_groups[0].failures, 1);
        assert_eq!(report.guarantee_groups[0].denials, 1);
        assert_eq!(report.effect_groups[0].key, "send_email");
        assert_eq!(report.data_class_groups[0].key, "private");

        let rendered = render_observe_show(&report);
        assert!(rendered.contains("guarantees:"));
        assert!(rendered.contains("approval.reachable_entrypoints_require_contract"));
        assert!(rendered.contains("events:"));
        assert!(rendered.contains("tool:send_email status=failed"));
    }

    #[test]
    fn drift_report_is_stable_text_and_ci_exit_code() {
        let mut baseline = LineageEvent::root("trace-1", LineageKind::Route, "POST /send", 1)
            .finish(LineageStatus::Ok, 10);
        baseline.cost_usd = 0.01;
        baseline.confidence = 0.9;
        let mut candidate = LineageEvent::root("trace-2", LineageKind::Route, "POST /send", 1)
            .finish(LineageStatus::Denied, 30);
        candidate.cost_usd = 0.04;
        candidate.confidence = 0.5;

        let report = compute_lineage_drift_report(&[baseline], &[candidate]);
        let rendered = render_drift_report(&report);

        assert!(rendered.contains("schema_violations:"));
        assert!(rendered.contains("cost_usd: baseline=0.010000 candidate=0.040000"));
        assert!(rendered.contains("denials: baseline=0 candidate=1 delta=1"));
        assert!(rendered.contains("verdict: drift"));
        assert_eq!(drift_exit_code(&report), 1);
    }

    #[test]
    fn drift_reads_lineage_directories() {
        let dir = tempfile::tempdir().unwrap();
        let baseline_dir = dir.path().join("baseline");
        let candidate_dir = dir.path().join("candidate");
        fs::create_dir_all(&baseline_dir).unwrap();
        fs::create_dir_all(&candidate_dir).unwrap();
        let baseline = LineageEvent::root("trace-1", LineageKind::Route, "GET /", 1)
            .finish(LineageStatus::Ok, 2);
        let candidate = LineageEvent::root("trace-2", LineageKind::Route, "GET /", 1)
            .finish(LineageStatus::Ok, 3);
        write_lineage(&baseline_dir.join("a.lineage.jsonl"), &[baseline]);
        write_lineage(&candidate_dir.join("b.lineage.jsonl"), &[candidate]);

        assert_eq!(read_lineage_input(&baseline_dir).unwrap().len(), 1);
        assert_eq!(read_lineage_input(&candidate_dir).unwrap().len(), 1);
    }
}
