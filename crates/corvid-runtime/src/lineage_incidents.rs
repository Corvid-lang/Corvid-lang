//! Contract-aware grouping for lineage incidents.

use crate::lineage::{LineageEvent, LineageStatus, LINEAGE_SCHEMA};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineageIncidentGroupKind {
    Guarantee,
    Effect,
    Budget,
    Provenance,
    Approval,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineageIncidentGroup {
    pub kind: LineageIncidentGroupKind,
    pub key: String,
    pub count: u64,
    pub failed: u64,
    pub denied: u64,
    pub pending_review: u64,
    pub schema_violations: u64,
    pub cost_usd: f64,
    pub span_ids: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct IncidentAccumulator {
    count: u64,
    failed: u64,
    denied: u64,
    pending_review: u64,
    schema_violations: u64,
    cost_usd: f64,
    span_ids: Vec<String>,
}

pub fn group_lineage_incidents(events: &[LineageEvent]) -> Vec<LineageIncidentGroup> {
    let mut groups: BTreeMap<(LineageIncidentGroupKind, String), IncidentAccumulator> =
        BTreeMap::new();
    for event in events {
        if !is_incident(event) {
            continue;
        }
        for (kind, key) in incident_keys(event) {
            let group = groups.entry((kind, key)).or_default();
            group.count += 1;
            if event.status == LineageStatus::Failed {
                group.failed += 1;
            }
            if event.status == LineageStatus::Denied {
                group.denied += 1;
            }
            if event.status == LineageStatus::PendingReview {
                group.pending_review += 1;
            }
            if event.schema != LINEAGE_SCHEMA {
                group.schema_violations += 1;
            }
            if event.cost_usd.is_finite() && event.cost_usd > 0.0 {
                group.cost_usd += event.cost_usd;
            }
            if !group.span_ids.contains(&event.span_id) {
                group.span_ids.push(event.span_id.clone());
            }
        }
    }
    groups
        .into_iter()
        .map(|((kind, key), group)| LineageIncidentGroup {
            kind,
            key,
            count: group.count,
            failed: group.failed,
            denied: group.denied,
            pending_review: group.pending_review,
            schema_violations: group.schema_violations,
            cost_usd: group.cost_usd,
            span_ids: group.span_ids,
        })
        .collect()
}

fn is_incident(event: &LineageEvent) -> bool {
    event.schema != LINEAGE_SCHEMA
        || matches!(
            event.status,
            LineageStatus::Failed | LineageStatus::Denied | LineageStatus::PendingReview
        )
}

fn incident_keys(event: &LineageEvent) -> Vec<(LineageIncidentGroupKind, String)> {
    let mut keys = Vec::new();
    if !event.guarantee_id.is_empty() {
        keys.push((
            LineageIncidentGroupKind::Guarantee,
            event.guarantee_id.clone(),
        ));
    }
    for effect_id in &event.effect_ids {
        if !effect_id.is_empty() {
            keys.push((LineageIncidentGroupKind::Effect, effect_id.clone()));
        }
    }
    if event.cost_usd.is_finite() && event.cost_usd > 0.0 {
        keys.push((
            LineageIncidentGroupKind::Budget,
            event
                .guarantee_id
                .clone()
                .if_empty_then(|| event.name.clone()),
        ));
    }
    if !event.retrieval_index_hash.is_empty() {
        keys.push((
            LineageIncidentGroupKind::Provenance,
            event.retrieval_index_hash.clone(),
        ));
    } else if !event.prompt_hash.is_empty() {
        keys.push((
            LineageIncidentGroupKind::Provenance,
            event.prompt_hash.clone(),
        ));
    }
    if !event.approval_id.is_empty() {
        keys.push((
            LineageIncidentGroupKind::Approval,
            event.approval_id.clone(),
        ));
    }
    if keys.is_empty() {
        keys.push((
            LineageIncidentGroupKind::Guarantee,
            "unclassified".to_string(),
        ));
    }
    keys
}

trait IfEmpty {
    fn if_empty_then(self, fallback: impl FnOnce() -> String) -> String;
}

impl IfEmpty for String {
    fn if_empty_then(self, fallback: impl FnOnce() -> String) -> String {
        if self.is_empty() {
            fallback()
        } else {
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lineage::{LineageEvent, LineageKind};

    #[test]
    fn incidents_group_by_guarantee_effect_budget_provenance_and_approval() {
        let mut event = LineageEvent::root("trace-1", LineageKind::Tool, "send_email", 1)
            .finish(LineageStatus::Failed, 20);
        event.guarantee_id = "approval.reachable_entrypoints_require_contract".to_string();
        event.effect_ids = vec!["send_email".to_string()];
        event.approval_id = "approval-1".to_string();
        event.prompt_hash = "prompt-sha".to_string();
        event.cost_usd = 0.25;

        let groups = group_lineage_incidents(&[event]);
        let keys = groups
            .iter()
            .map(|group| (group.kind, group.key.as_str()))
            .collect::<Vec<_>>();

        assert!(keys.contains(&(
            LineageIncidentGroupKind::Guarantee,
            "approval.reachable_entrypoints_require_contract"
        )));
        assert!(keys.contains(&(LineageIncidentGroupKind::Effect, "send_email")));
        assert!(keys.contains(&(
            LineageIncidentGroupKind::Budget,
            "approval.reachable_entrypoints_require_contract"
        )));
        assert!(keys.contains(&(LineageIncidentGroupKind::Provenance, "prompt-sha")));
        assert!(keys.contains(&(LineageIncidentGroupKind::Approval, "approval-1")));
        assert!(groups.iter().all(|group| group.failed == 1));
    }

    #[test]
    fn non_incident_ok_events_are_not_grouped() {
        let event = LineageEvent::root("trace-1", LineageKind::Route, "GET /", 1)
            .finish(LineageStatus::Ok, 2);
        assert!(group_lineage_incidents(&[event]).is_empty());
    }
}
