//! Drift metrics computed from lineage trace sets.

use crate::lineage::{LineageEvent, LineageKind, LineageStatus, LINEAGE_SCHEMA};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineageDriftSummary {
    pub trace_count: u64,
    pub event_count: u64,
    pub schema_violation_count: u64,
    pub total_cost_usd: f64,
    pub total_latency_ms: u64,
    pub denial_count: u64,
    pub tool_error_count: u64,
    pub confidence_sample_count: u64,
    pub average_confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineageDriftReport {
    pub baseline: LineageDriftSummary,
    pub candidate: LineageDriftSummary,
    pub cost_delta_usd: f64,
    pub latency_delta_ms: i128,
    pub denial_delta: i128,
    pub tool_error_delta: i128,
    pub confidence_delta: f64,
    pub schema_violation_delta: i128,
}

pub fn summarize_lineage_drift_metrics(events: &[LineageEvent]) -> LineageDriftSummary {
    let mut trace_ids = std::collections::BTreeSet::new();
    let mut confidence_sum = 0.0;
    let mut confidence_sample_count = 0u64;
    for event in events {
        trace_ids.insert(event.trace_id.clone());
        if event.confidence.is_finite() && event.confidence > 0.0 {
            confidence_sum += event.confidence;
            confidence_sample_count += 1;
        }
    }

    LineageDriftSummary {
        trace_count: trace_ids.len() as u64,
        event_count: events.len() as u64,
        schema_violation_count: events
            .iter()
            .filter(|event| event.schema != LINEAGE_SCHEMA)
            .count() as u64,
        total_cost_usd: events
            .iter()
            .map(|event| event.cost_usd)
            .filter(|cost| cost.is_finite() && *cost > 0.0)
            .sum(),
        total_latency_ms: events.iter().map(|event| event.latency_ms).sum(),
        denial_count: events
            .iter()
            .filter(|event| event.status == LineageStatus::Denied)
            .count() as u64,
        tool_error_count: events
            .iter()
            .filter(|event| {
                event.kind == LineageKind::Tool && event.status == LineageStatus::Failed
            })
            .count() as u64,
        confidence_sample_count,
        average_confidence: if confidence_sample_count == 0 {
            0.0
        } else {
            confidence_sum / confidence_sample_count as f64
        },
    }
}

pub fn compute_lineage_drift_report(
    baseline_events: &[LineageEvent],
    candidate_events: &[LineageEvent],
) -> LineageDriftReport {
    let baseline = summarize_lineage_drift_metrics(baseline_events);
    let candidate = summarize_lineage_drift_metrics(candidate_events);
    LineageDriftReport {
        cost_delta_usd: candidate.total_cost_usd - baseline.total_cost_usd,
        latency_delta_ms: candidate.total_latency_ms as i128 - baseline.total_latency_ms as i128,
        denial_delta: candidate.denial_count as i128 - baseline.denial_count as i128,
        tool_error_delta: candidate.tool_error_count as i128 - baseline.tool_error_count as i128,
        confidence_delta: candidate.average_confidence - baseline.average_confidence,
        schema_violation_delta: candidate.schema_violation_count as i128
            - baseline.schema_violation_count as i128,
        baseline,
        candidate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drift_metrics_cover_schema_confidence_cost_latency_denials_and_tool_errors() {
        let mut baseline_route = LineageEvent::root("trace-a", LineageKind::Route, "POST /send", 1)
            .finish(LineageStatus::Ok, 11);
        baseline_route.cost_usd = 0.01;
        baseline_route.confidence = 0.90;
        let mut baseline_tool =
            LineageEvent::child(&baseline_route, LineageKind::Tool, "send_email", 0, 2)
                .finish(LineageStatus::Ok, 7);
        baseline_tool.confidence = 0.80;

        let mut candidate_route =
            LineageEvent::root("trace-b", LineageKind::Route, "POST /send", 1)
                .finish(LineageStatus::Denied, 21);
        candidate_route.cost_usd = 0.05;
        candidate_route.confidence = 0.50;
        let mut candidate_tool =
            LineageEvent::child(&candidate_route, LineageKind::Tool, "send_email", 0, 2)
                .finish(LineageStatus::Failed, 19);
        candidate_tool.schema = "bad.schema".to_string();
        candidate_tool.confidence = 0.40;

        let report = compute_lineage_drift_report(
            &[baseline_route, baseline_tool],
            &[candidate_route, candidate_tool],
        );

        assert!((report.cost_delta_usd - 0.04).abs() < f64::EPSILON);
        assert_eq!(report.latency_delta_ms, 22);
        assert_eq!(report.denial_delta, 1);
        assert_eq!(report.tool_error_delta, 1);
        assert_eq!(report.schema_violation_delta, 1);
        assert!(report.confidence_delta < 0.0);
    }
}
