mod common;

use common::outcome;
use corvid_shadow_daemon::alerts::dimension::DimensionAlertEngine;
use corvid_shadow_daemon::config::DimensionAlertConfig;
use corvid_shadow_daemon::TrustTier;

#[test]
fn trust_drop_emits_dimension_alert() {
    let mut engine = DimensionAlertEngine::new();
    let mut result = outcome("refund_bot");
    result.shadow_dimensions.trust_tier = Some(TrustTier::HumanRequired);
    let alerts = engine.evaluate(&DimensionAlertConfig::default(), &result);
    assert!(alerts.iter().any(|alert| alert.summary.contains("trust tier dropped")));
}

#[test]
fn budget_overrun_emits_dimension_alert() {
    let mut engine = DimensionAlertEngine::new();
    let mut result = outcome("refund_bot");
    result.shadow_dimensions.cost = 12.0;
    let alerts = engine.evaluate(&DimensionAlertConfig::default(), &result);
    assert!(alerts.iter().any(|alert| alert.summary.contains("exceeded declared budget")));
}

#[test]
fn budget_burn_rate_projection_fires_when_runway_below_threshold() {
    let mut engine = DimensionAlertEngine::new();
    let mut result = outcome("refund_bot");
    result.shadow_dimensions.cost = 9.0;
    result.shadow_events[2] = corvid_runtime::TraceEvent::RunCompleted {
        ts_ms: 86_400_000,
        run_id: "run-recorded".into(),
        ok: true,
        result: Some(serde_json::json!("ok")),
        error: None,
    };
    engine.evaluate(&DimensionAlertConfig::default(), &result);
    let alerts = engine.evaluate(&DimensionAlertConfig::default(), &result);
    assert!(alerts.iter().any(|alert| alert.summary.contains("burn rate")));
}

#[test]
fn latency_slo_crossing_emits_dimension_alert() {
    let mut engine = DimensionAlertEngine::new();
    let mut result = outcome("refund_bot");
    result.shadow_dimensions.latency_ms = 500;
    let mut config = DimensionAlertConfig::default();
    config.latency.p50_ms = Some(100);
    let alerts = engine.evaluate(&config, &result);
    assert!(alerts.iter().any(|alert| alert.summary.contains("latency p50")));
}

#[test]
fn no_alert_fires_when_within_thresholds() {
    let mut engine = DimensionAlertEngine::new();
    let result = outcome("refund_bot");
    let mut config = DimensionAlertConfig::default();
    config.latency.p50_ms = Some(1000);
    config.latency.p99_ms = Some(1000);
    let alerts = engine.evaluate(&config, &result);
    assert!(alerts.is_empty());
}
