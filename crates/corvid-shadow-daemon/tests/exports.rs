mod common;

use common::outcome;
use corvid_shadow_daemon::alerts::{Alert, AlertKind, AlertSeverity};
use corvid_shadow_daemon::exports::{ExportSink, OtelExporter, PrometheusExporter};

#[tokio::test]
async fn prometheus_endpoint_serves_metrics_with_typed_labels() {
    let exporter = PrometheusExporter::bind("127.0.0.1:0").await.unwrap();
    let outcome = outcome("refund_bot");
    let alert = Alert {
        ts_ms: 0,
        severity: AlertSeverity::Warning,
        kind: AlertKind::Dimension,
        agent: "refund_bot".into(),
        trace_path: outcome.trace_path.clone(),
        summary: "trust drop".into(),
        payload: serde_json::json!({ "dimension": "trust" }),
    };
    exporter.record_outcome(&outcome, &[alert]).await.unwrap();

    let body = reqwest::get(format!("http://{}", exporter.bind_addr()))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("corvid_shadow_divergences_total"));
    assert!(body.contains("dimension=\"trust\""));
}

#[tokio::test]
async fn otel_spans_emit_with_corvid_dimension_attributes() {
    let server = wiremock::MockServer::start().await;
    let exporter = OtelExporter::new(server.uri());
    let outcome = outcome("refund_bot");
    let alert = Alert {
        ts_ms: 0,
        severity: AlertSeverity::Warning,
        kind: AlertKind::Dimension,
        agent: "refund_bot".into(),
        trace_path: outcome.trace_path.clone(),
        summary: "trust drop".into(),
        payload: serde_json::json!({ "dimension": "trust" }),
    };
    exporter.record_outcome(&outcome, &[alert]).await.unwrap();
}

#[tokio::test]
async fn export_sinks_degrade_gracefully_when_collector_unavailable() {
    let exporter = OtelExporter::new("http://127.0.0.1:9");
    let outcome = outcome("refund_bot");
    exporter.record_outcome(&outcome, &[]).await.unwrap();
}
