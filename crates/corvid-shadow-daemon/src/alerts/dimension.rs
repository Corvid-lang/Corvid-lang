use crate::alerts::{Alert, AlertKind, AlertSeverity};
use crate::config::DimensionAlertConfig;
use crate::replay_pool::{ShadowReplayOutcome, TrustTier};
use corvid_runtime::now_ms;
use serde_json::json;
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
struct AgentDimensionHistory {
    runs: usize,
    trust_drops: usize,
    latencies_ms: Vec<u64>,
    cumulative_cost: f64,
    first_ts_ms: Option<u64>,
    last_ts_ms: Option<u64>,
}

#[derive(Debug, Default)]
pub struct DimensionAlertEngine {
    history: HashMap<String, AgentDimensionHistory>,
}

impl DimensionAlertEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn evaluate(
        &mut self,
        config: &DimensionAlertConfig,
        outcome: &ShadowReplayOutcome,
    ) -> Vec<Alert> {
        let history = self
            .history
            .entry(outcome.agent.clone())
            .or_default();
        history.runs += 1;
        history.cumulative_cost += outcome.shadow_dimensions.cost;
        history.latencies_ms.push(outcome.shadow_dimensions.latency_ms);

        let observed_ts_ms = outcome
            .shadow_events
            .iter()
            .find_map(|event| match event {
                corvid_runtime::TraceEvent::RunCompleted { ts_ms, .. } => Some(*ts_ms),
                _ => None,
            })
            .unwrap_or_else(now_ms);
        history.first_ts_ms.get_or_insert(observed_ts_ms);
        history.last_ts_ms = Some(observed_ts_ms);

        let mut alerts = Vec::new();

        let trust_dropped = outcome.recorded_dimensions.trust_tier == Some(TrustTier::Autonomous)
            && outcome.shadow_dimensions.trust_tier == Some(TrustTier::HumanRequired);
        if trust_dropped {
            history.trust_drops += 1;
            alerts.push(Alert {
                ts_ms: now_ms(),
                severity: AlertSeverity::Warning,
                kind: AlertKind::Dimension,
                agent: outcome.agent.clone(),
                trace_path: outcome.trace_path.clone(),
                summary: "trust tier dropped below recorded autonomous level".into(),
                payload: json!({
                    "dimension": "trust",
                    "recorded_value": format!("{:?}", outcome.recorded_dimensions.trust_tier),
                    "shadow_value": format!("{:?}", outcome.shadow_dimensions.trust_tier),
                    "threshold": config.trust.threshold_fraction_below_autonomous,
                    "delta": "autonomous_to_human_required",
                }),
            });
        }

        let trust_fraction = history.trust_drops as f64 / history.runs.max(1) as f64;
        if trust_fraction > config.trust.threshold_fraction_below_autonomous {
            alerts.push(Alert {
                ts_ms: now_ms(),
                severity: AlertSeverity::Warning,
                kind: AlertKind::Dimension,
                agent: outcome.agent.clone(),
                trace_path: outcome.trace_path.clone(),
                summary: "rolling trust-drop rate exceeded configured tolerance".into(),
                payload: json!({
                    "dimension": "trust",
                    "recorded_value": "autonomous",
                    "shadow_value": trust_fraction,
                    "threshold": config.trust.threshold_fraction_below_autonomous,
                    "delta": trust_fraction - config.trust.threshold_fraction_below_autonomous,
                }),
            });
        }

        if config.budget.alert_on_overrun {
            if let Some(budget) = outcome.shadow_dimensions.budget_declared {
                if outcome.shadow_dimensions.cost > budget {
                    alerts.push(Alert {
                        ts_ms: now_ms(),
                        severity: AlertSeverity::Critical,
                        kind: AlertKind::Dimension,
                        agent: outcome.agent.clone(),
                        trace_path: outcome.trace_path.clone(),
                        summary: "shadow replay exceeded declared budget".into(),
                        payload: json!({
                            "dimension": "budget",
                            "recorded_value": budget,
                            "shadow_value": outcome.shadow_dimensions.cost,
                            "threshold": budget,
                            "delta": outcome.shadow_dimensions.cost - budget,
                        }),
                    });
                }

                if let (Some(first), Some(last)) = (history.first_ts_ms, history.last_ts_ms) {
                    let days = ((last.saturating_sub(first)) as f64 / 86_400_000.0).max(1.0);
                    let burn_per_day = history.cumulative_cost / days;
                    if burn_per_day > 0.0 {
                        let runway_days = budget / burn_per_day;
                        if runway_days < config.budget.burn_rate_alert_days_runway as f64 {
                            alerts.push(Alert {
                                ts_ms: now_ms(),
                                severity: AlertSeverity::Warning,
                                kind: AlertKind::Dimension,
                                agent: outcome.agent.clone(),
                                trace_path: outcome.trace_path.clone(),
                                summary: "projected burn rate leaves insufficient budget runway".into(),
                                payload: json!({
                                    "dimension": "budget",
                                    "recorded_value": budget,
                                    "shadow_value": burn_per_day,
                                    "threshold": config.budget.burn_rate_alert_days_runway,
                                    "delta": runway_days,
                                }),
                            });
                        }
                    }
                }
            }
        }

        if let Some(p50_ms) = config.latency.p50_ms {
            let observed = percentile(&history.latencies_ms, 0.50);
            if observed > p50_ms {
                alerts.push(Alert {
                    ts_ms: now_ms(),
                    severity: AlertSeverity::Warning,
                    kind: AlertKind::Dimension,
                    agent: outcome.agent.clone(),
                    trace_path: outcome.trace_path.clone(),
                    summary: "latency p50 crossed configured SLO".into(),
                    payload: json!({
                        "dimension": "latency_ms",
                        "recorded_value": outcome.recorded_dimensions.latency_ms,
                        "shadow_value": observed,
                        "threshold": p50_ms,
                        "delta": observed.saturating_sub(p50_ms),
                    }),
                });
            }
        }

        if let Some(p99_ms) = config.latency.p99_ms {
            let observed = percentile(&history.latencies_ms, 0.99);
            if observed > p99_ms {
                alerts.push(Alert {
                    ts_ms: now_ms(),
                    severity: AlertSeverity::Warning,
                    kind: AlertKind::Dimension,
                    agent: outcome.agent.clone(),
                    trace_path: outcome.trace_path.clone(),
                    summary: "latency p99 crossed configured SLO".into(),
                    payload: json!({
                        "dimension": "latency_ms",
                        "recorded_value": outcome.recorded_dimensions.latency_ms,
                        "shadow_value": observed,
                        "threshold": p99_ms,
                        "delta": observed.saturating_sub(p99_ms),
                    }),
                });
            }
        }

        alerts
    }
}

fn percentile(values: &[u64], quantile: f64) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let idx = ((sorted.len() as f64 - 1.0) * quantile).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}
