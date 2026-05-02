use anyhow::{Context, Result};
use corvid_runtime::{
    compute_lineage_drift_report, group_lineage_incidents, LineageDriftReport, LineageEvent,
    LineageIncidentGroup, LineageKind, LineageStatus,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

mod drift;
mod list;
pub use drift::render_drift_report;
pub use list::{build_observe_list, render_observe_list};
use list::summarize_run;
use drift::drift_exit_code;

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
    pub incident_groups: Vec<LineageIncidentGroup>,
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
        incident_groups: group_lineage_incidents(&events),
    })
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
    render_incident_groups(&mut out, &report.incident_groups);
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

fn render_incident_groups(out: &mut String, groups: &[LineageIncidentGroup]) {
    out.push_str("incidents:\n");
    if groups.is_empty() {
        out.push_str("- none\n");
        return;
    }
    for group in groups {
        out.push_str(&format!(
            "- {:?}:{} count={} failed={} denied={} pending={} schema={} cost_usd={:.6}\n",
            group.kind,
            group.key,
            group.count,
            group.failed,
            group.denied,
            group.pending_review,
            group.schema_violations,
            group.cost_usd
        ));
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
pub(super) enum GroupKind {
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

pub(super) fn build_groups(events: &[LineageEvent], kind: GroupKind) -> Vec<ObservedGroup> {
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

pub(super) fn render_groups(out: &mut String, label: &str, groups: &[ObservedGroup]) {
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

pub(super) fn single_group_key(value: &str) -> Vec<String> {
    if value.is_empty() {
        Vec::new()
    } else {
        vec![value.to_string()]
    }
}

pub(super) fn read_lineage_events(path: &Path) -> Result<Vec<LineageEvent>> {
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

pub(super) fn read_lineage_input(path: &Path) -> Result<Vec<LineageEvent>> {
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

pub(super) fn resolve_lineage_path(id_or_path: &str, trace_dir: Option<&Path>) -> PathBuf {
    let direct = PathBuf::from(id_or_path);
    if direct.exists() || direct.extension().is_some() {
        return direct;
    }
    trace_dir
        .unwrap_or_else(|| Path::new("target/trace"))
        .join(format!("{id_or_path}.lineage.jsonl"))
}

pub(super) fn is_lineage_jsonl(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.ends_with(".lineage.jsonl"))
        .unwrap_or(false)
}

pub(super) fn kind_name(kind: LineageKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| format!("{kind:?}").to_lowercase())
}

pub(super) fn status_name(status: LineageStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| format!("{status:?}").to_lowercase())
}

pub(super) fn finite_cost(value: f64) -> f64 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}

pub(super) fn display_optional(value: &str) -> &str {
    if value.is_empty() {
        "-"
    } else {
        value
    }
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
        assert!(!report.incident_groups.is_empty());

        let rendered = render_observe_show(&report);
        assert!(rendered.contains("guarantees:"));
        assert!(rendered.contains("incidents:"));
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
