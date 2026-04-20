use crate::alerts::{Alert, AlertKind, AlertSeverity};
use crate::config::CounterfactualAlertConfig;
use crate::replay_pool::{MutationSpec, ShadowReplayOutcome};
use corvid_runtime::{now_ms, TraceEvent};
use serde_json::json;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

pub fn should_sample(path: &Path, config: &CounterfactualAlertConfig) -> bool {
    if config.sample_fraction <= 0.0 {
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

pub fn propose_mutations(
    outcome: &ShadowReplayOutcome,
    config: &CounterfactualAlertConfig,
) -> Vec<MutationSpec> {
    let mut mutations = Vec::new();
    let mut substitutable_step = 0usize;
    let events = &outcome.recorded_events;
    let mut i = 0usize;
    while i + 1 < events.len() && mutations.len() < config.max_mutations_per_trace {
        match (&events[i], &events[i + 1]) {
            (TraceEvent::LlmCall { prompt, .. }, TraceEvent::LlmResult { result, .. }) => {
                substitutable_step += 1;
                if let Some(replacement) = mutate_json_value(result) {
                    mutations.push(MutationSpec {
                        step_1based: substitutable_step,
                        replacement,
                        label: format!("llm:{prompt}"),
                    });
                }
                i += 2;
            }
            (TraceEvent::ToolCall { tool, .. }, TraceEvent::ToolResult { result, .. }) => {
                substitutable_step += 1;
                if let Some(replacement) = mutate_json_value(result) {
                    mutations.push(MutationSpec {
                        step_1based: substitutable_step,
                        replacement,
                        label: format!("tool:{tool}"),
                    });
                }
                i += 2;
            }
            (
                TraceEvent::ApprovalRequest { label, .. },
                TraceEvent::ApprovalResponse { approved, .. },
            ) => {
                substitutable_step += 1;
                mutations.push(MutationSpec {
                    step_1based: substitutable_step,
                    replacement: serde_json::Value::Bool(!approved),
                    label: format!("approval:{label}"),
                });
                i += 2;
            }
            _ => i += 1,
        }
    }
    mutations
}

pub fn analyze_mutation_outcome(
    base: &ShadowReplayOutcome,
    mutated: &ShadowReplayOutcome,
    mutation: &MutationSpec,
) -> Option<Alert> {
    let base_dangerous = has_dangerous_outcome(base);
    let mutated_dangerous = has_dangerous_outcome(mutated);
    if !base_dangerous && mutated_dangerous {
        Some(Alert {
            ts_ms: now_ms(),
            severity: AlertSeverity::Critical,
            kind: AlertKind::Counterfactual,
            agent: base.agent.clone(),
            trace_path: base.trace_path.clone(),
            summary: "counterfactual mutation uncovered a dangerous path".into(),
            payload: json!({
                "base_trace": base.trace_path,
                "mutation": {
                    "step_1based": mutation.step_1based,
                    "label": mutation.label,
                    "replacement": mutation.replacement,
                },
                "recorded_verdict": base.shadow_output,
                "new_verdict": mutated.shadow_output,
                "severity": "critical",
            }),
        })
    } else {
        None
    }
}

pub fn has_dangerous_outcome(outcome: &ShadowReplayOutcome) -> bool {
    outcome.shadow_events.iter().any(|event| match event {
        TraceEvent::ToolCall { tool, .. } => outcome
            .metadata
            .dangerous_tools
            .iter()
            .any(|dangerous| dangerous.tool == *tool),
        _ => false,
    })
}

fn mutate_json_value(value: &serde_json::Value) -> Option<serde_json::Value> {
    match value {
        serde_json::Value::Bool(v) => Some(serde_json::Value::Bool(!v)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(serde_json::json!(i + 1))
            } else if let Some(f) = n.as_f64() {
                Some(serde_json::json!(f + 1.0))
            } else {
                None
            }
        }
        serde_json::Value::String(s) => Some(serde_json::Value::String(match s.as_str() {
            "refund" => "cancel".into(),
            "cancel" => "refund".into(),
            _ => format!("{s}-mutated"),
        })),
        serde_json::Value::Object(map) => {
            if let Some((key, value)) = map.iter().next() {
                let mut mutated = map.clone();
                mutated.insert(key.clone(), mutate_json_value(value).unwrap_or_else(|| value.clone()));
                Some(serde_json::Value::Object(mutated))
            } else {
                None
            }
        }
        _ => None,
    }
}
