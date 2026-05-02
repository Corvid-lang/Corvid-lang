//! `corvid observe show` build + render.
//!
//! `build_observe_show` resolves a run id (or path) to the
//! corresponding lineage trace, builds the per-event records,
//! and groups events by guarantee / effect / data-class
//! fingerprint plus the lineage incident clusters. The
//! `render_observe_show` text formatter emits a per-event line
//! plus the four group sections.
//!
//! `build_groups` (with its `GroupKind` and private
//! `GroupAccumulator`) is the per-axis bucket-and-roll-up
//! helper. It also serves the `build_observe_list` path —
//! both list and show fingerprint events the same way.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};

use corvid_runtime::{
    group_lineage_incidents, LineageEvent, LineageIncidentGroup, LineageStatus,
};

use super::list::summarize_run;
use super::{
    display_optional, finite_cost, kind_name, read_lineage_events, resolve_lineage_path,
    status_name, ObserveShowReport, ObservedEvent, ObservedGroup,
};

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
