use crate::alerts::{Alert, AlertKind, AlertSeverity};
use crate::config::ProvenanceAlertConfig;
use crate::replay_pool::ShadowReplayOutcome;
use corvid_runtime::now_ms;
use serde_json::json;

pub fn evaluate(config: &ProvenanceAlertConfig, outcome: &ShadowReplayOutcome) -> Vec<Alert> {
    let mut alerts = Vec::new();

    if config.alert_on_reasoning_drift
        && outcome.recorded_output == outcome.shadow_output
        && outcome.recorded_provenance.root_sources != outcome.shadow_provenance.root_sources
    {
        alerts.push(Alert {
            ts_ms: now_ms(),
            severity: AlertSeverity::Warning,
            kind: AlertKind::Provenance,
            agent: outcome.agent.clone(),
            trace_path: outcome.trace_path.clone(),
            summary: "output stayed the same but cited provenance sources drifted".into(),
            payload: json!({
                "drift_kind": "reasoning_drift",
                "recorded_sources": outcome.recorded_provenance.root_sources,
                "shadow_sources": outcome.shadow_provenance.root_sources,
            }),
        });
    }

    if config.alert_on_chain_break
        && outcome.recorded_provenance.has_chain
        && !outcome.shadow_provenance.has_chain
    {
        alerts.push(Alert {
            ts_ms: now_ms(),
            severity: AlertSeverity::Critical,
            kind: AlertKind::Provenance,
            agent: outcome.agent.clone(),
            trace_path: outcome.trace_path.clone(),
            summary: "grounded value lost its provenance chain in shadow replay".into(),
            payload: json!({
                "drift_kind": "chain_break",
                "recorded_sources": outcome.recorded_provenance.root_sources,
                "shadow_sources": outcome.shadow_provenance.root_sources,
            }),
        });
    }

    alerts
}
