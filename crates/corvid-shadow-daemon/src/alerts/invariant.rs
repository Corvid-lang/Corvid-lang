use crate::alerts::{Alert, AlertKind, AlertSeverity};
use crate::replay_pool::{ShadowReplayOutcome, TrustTier};
use corvid_runtime::{now_ms, TraceEvent};
use serde_json::json;

pub fn evaluate(outcome: &ShadowReplayOutcome) -> Vec<Alert> {
    let mut alerts = Vec::new();

    if outcome.metadata.replayable
        && (!outcome.traces_match()
            || outcome.recorded_output != outcome.shadow_output
            || outcome.replay_divergence.is_some())
    {
        alerts.push(critical(
            outcome,
            "replayable",
            "shadow replay failed byte-identity for a @replayable agent",
            json!({
                "recorded_output": outcome.recorded_output,
                "shadow_output": outcome.shadow_output,
                "replay_divergence": outcome.replay_divergence.as_ref().map(ToString::to_string),
            }),
        ));
    }

    if outcome.metadata.deterministic {
        let recorded_positions = ShadowReplayOutcome::seed_clock_positions(&outcome.recorded_events);
        let shadow_positions = ShadowReplayOutcome::seed_clock_positions(&outcome.shadow_events);
        if recorded_positions != shadow_positions || !outcome.traces_match() {
            alerts.push(critical(
                outcome,
                "deterministic",
                "seed or clock reads moved in a @deterministic agent",
                json!({
                    "recorded_positions": recorded_positions,
                    "shadow_positions": shadow_positions,
                }),
            ));
        }
    }

    if let Some(budget) = outcome.metadata.budget_declared {
        if outcome.shadow_dimensions.cost > budget {
            alerts.push(critical(
                outcome,
                "budget",
                "shadow replay exceeded declared budget invariant",
                json!({
                    "budget": budget,
                    "shadow_cost": outcome.shadow_dimensions.cost,
                }),
            ));
        }
    }

    if let Some(tool) = first_dangerous_without_approval(outcome) {
        alerts.push(critical(
            outcome,
            "approve_before_dangerous",
            "dangerous tool call appeared without a prior approved gate",
            json!({ "tool": tool }),
        ));
    }

    if outcome.metadata.grounded_return && !outcome.shadow_provenance.has_chain {
        alerts.push(critical(
            outcome,
            "grounded_return",
            "Grounded<T> return lost its provenance chain",
            json!({ "shadow_sources": outcome.shadow_provenance.root_sources }),
        ));
    }

    if outcome.recorded_dimensions.trust_tier == Some(TrustTier::Autonomous)
        && outcome.shadow_dimensions.trust_tier == Some(TrustTier::HumanRequired)
    {
        alerts.push(critical(
            outcome,
            "trust_tier",
            "autonomous run regressed to human-required trust at runtime",
            json!({
                "recorded_value": "Autonomous",
                "shadow_value": "HumanRequired",
            }),
        ));
    }

    alerts
}

fn first_dangerous_without_approval(outcome: &ShadowReplayOutcome) -> Option<String> {
    for dangerous in &outcome.metadata.dangerous_tools {
        let mut approved = false;
        for event in &outcome.shadow_events {
            match event {
                TraceEvent::ApprovalResponse { label, approved: yes, .. }
                    if label == &dangerous.approval_label && *yes =>
                {
                    approved = true;
                }
                TraceEvent::ToolCall { tool, .. } if tool == &dangerous.tool && !approved => {
                    return Some(tool.clone());
                }
                _ => {}
            }
        }
    }
    None
}

fn critical(
    outcome: &ShadowReplayOutcome,
    invariant: &str,
    summary: &str,
    payload: serde_json::Value,
) -> Alert {
    Alert {
        ts_ms: now_ms(),
        severity: AlertSeverity::Critical,
        kind: AlertKind::Invariant,
        agent: outcome.agent.clone(),
        trace_path: outcome.trace_path.clone(),
        summary: summary.into(),
        payload: json!({
            "invariant": invariant,
            "violation_detail": payload,
            "commit_sha": outcome
                .recorded_events
                .iter()
                .find_map(|event| match event {
                    TraceEvent::SchemaHeader { commit_sha, .. } => commit_sha.clone(),
                    _ => None,
                }),
        }),
    }
}
