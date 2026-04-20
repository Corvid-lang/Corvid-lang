mod common;

use common::outcome;
use corvid_runtime::TraceEvent;
use corvid_shadow_daemon::alerts::invariant;

#[test]
fn replayable_violation_alerts_at_critical_severity() {
    let mut result = outcome("refund_bot");
    result.shadow_output = Some(serde_json::json!("changed"));
    let alerts = invariant::evaluate(&result);
    assert!(alerts.iter().any(|alert| alert.summary.contains("@replayable")));
}

#[test]
fn deterministic_violation_alerts() {
    let mut result = outcome("refund_bot");
    result.shadow_events.insert(
        2,
        TraceEvent::SeedRead {
            ts_ms: 3,
            run_id: "run-recorded".into(),
            purpose: "rollout_cohort".into(),
            value: 42,
        },
    );
    let alerts = invariant::evaluate(&result);
    assert!(alerts.iter().any(|alert| alert.summary.contains("@deterministic")));
}

#[test]
fn budget_invariant_violation_alerts() {
    let mut result = outcome("refund_bot");
    result.shadow_dimensions.cost = 11.0;
    let alerts = invariant::evaluate(&result);
    assert!(alerts.iter().any(|alert| alert.summary.contains("declared budget")));
}

#[test]
fn approve_before_dangerous_violation_alerts() {
    let mut result = outcome("refund_bot");
    result.shadow_events.insert(
        2,
        TraceEvent::ToolCall {
            ts_ms: 3,
            run_id: "run-recorded".into(),
            tool: "issue_refund".into(),
            args: vec![],
        },
    );
    let alerts = invariant::evaluate(&result);
    assert!(alerts.iter().any(|alert| alert.summary.contains("dangerous tool call")));
}

#[test]
fn grounded_chain_break_alerts() {
    let mut result = outcome("refund_bot");
    result.shadow_provenance.has_chain = false;
    let alerts = invariant::evaluate(&result);
    assert!(alerts.iter().any(|alert| alert.summary.contains("Grounded<T> return")));
}

#[test]
fn no_invariant_violations_fire_for_clean_traces() {
    let result = outcome("refund_bot");
    assert!(invariant::evaluate(&result).is_empty());
}
