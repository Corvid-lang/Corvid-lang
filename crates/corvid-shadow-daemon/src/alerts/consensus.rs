use crate::alerts::{Alert, AlertKind, AlertSeverity};
use crate::config::ConsensusAlertConfig;
use crate::replay_pool::ShadowReplayOutcome;
use corvid_runtime::now_ms;
use serde_json::json;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;

pub fn should_sample(path: &Path, config: &ConsensusAlertConfig) -> bool {
    if config.models.is_empty() || config.sample_fraction <= 0.0 {
        return false;
    }
    if config.sample_fraction >= 1.0 {
        return true;
    }
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    let bucket = (hasher.finish() % 10_000) as f64 / 10_000.0;
    bucket < config.sample_fraction
}

pub fn evaluate(
    config: &ConsensusAlertConfig,
    base: &ShadowReplayOutcome,
    by_model: &HashMap<String, ShadowReplayOutcome>,
) -> Option<Alert> {
    if by_model.is_empty() {
        return None;
    }
    let prod = base.shadow_output.clone().unwrap_or(serde_json::Value::Null);
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut rendered: HashMap<String, serde_json::Value> = HashMap::new();

    for (model, outcome) in by_model {
        let result = outcome.shadow_output.clone().unwrap_or(serde_json::Value::Null);
        let key = result.to_string();
        *counts.entry(key.clone()).or_insert(0) += 1;
        rendered.insert(model.clone(), result);
    }

    let Some((winner_key, winner_count)) = counts
        .iter()
        .max_by_key(|(_, count)| **count)
        .map(|(key, count)| (key.clone(), *count))
    else {
        return None;
    };

    if winner_count < config.min_agreement {
        return Some(Alert {
            ts_ms: now_ms(),
            severity: AlertSeverity::Warning,
            kind: AlertKind::Consensus,
            agent: base.agent.clone(),
            trace_path: base.trace_path.clone(),
            summary: "consensus sample had insufficient agreement across comparison models".into(),
            payload: json!({
                "prompt": base.agent,
                "prod_result": prod,
                "model_results": rendered,
                "consensus_mode": "insufficient_agreement",
            }),
        });
    }

    if prod.to_string() != winner_key {
        return Some(Alert {
            ts_ms: now_ms(),
            severity: AlertSeverity::Warning,
            kind: AlertKind::Consensus,
            agent: base.agent.clone(),
            trace_path: base.trace_path.clone(),
            summary: "production result disagreed with cross-model consensus".into(),
            payload: json!({
                "prompt": base.agent,
                "prod_result": prod,
                "model_results": rendered,
                "consensus_mode": winner_key,
            }),
        });
    }

    None
}
