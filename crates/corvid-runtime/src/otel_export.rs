//! OTLP/HTTP JSON export for Corvid lineage events.
//!
//! The lineage JSONL model remains the lossless Corvid contract. This module
//! builds interoperable OpenTelemetry payloads from that model and optionally
//! sends them to an OTLP/HTTP collector.

use crate::lineage::{validate_lineage, LineageEvent, LineageKind, LineageStatus};
use crate::otel_schema::{lineage_to_otel_span, OTEL_SCHEMA_VERSION};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

const SCOPE_NAME: &str = "corvid-runtime";
const SCOPE_VERSION: &str = env!("CARGO_PKG_VERSION");
const OTLP_JSON_CONTENT_TYPE: &str = "application/json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtelExporterConfig {
    pub traces_endpoint: String,
    pub metrics_endpoint: String,
    pub service_name: String,
    pub service_version: String,
    pub deployment_environment: String,
    pub headers: BTreeMap<String, String>,
    pub timeout_ms: u64,
}

impl OtelExporterConfig {
    pub fn local(service_name: impl Into<String>) -> Self {
        Self {
            traces_endpoint: "http://localhost:4318/v1/traces".to_string(),
            metrics_endpoint: "http://localhost:4318/v1/metrics".to_string(),
            service_name: service_name.into(),
            service_version: SCOPE_VERSION.to_string(),
            deployment_environment: String::new(),
            headers: BTreeMap::new(),
            timeout_ms: 10_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OtelExportBatch {
    pub traces_payload: Value,
    pub metrics_payload: Value,
    pub span_count: usize,
    pub metric_datapoint_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtelExportReport {
    pub span_count: usize,
    pub metric_datapoint_count: usize,
    pub traces_status: u16,
    pub metrics_status: u16,
}

#[derive(Debug)]
pub enum OtelExportError {
    InvalidLineage(Vec<String>),
    InvalidHeader(String),
    Http(reqwest::Error),
    CollectorRejected {
        signal: &'static str,
        status: u16,
        body: String,
    },
}

impl std::fmt::Display for OtelExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLineage(violations) => {
                write!(f, "lineage is incomplete: {}", violations.join(", "))
            }
            Self::InvalidHeader(header) => write!(f, "invalid OTLP header: {header}"),
            Self::Http(err) => write!(f, "OTLP HTTP export failed: {err}"),
            Self::CollectorRejected {
                signal,
                status,
                body,
            } => write!(
                f,
                "OTLP collector rejected {signal} export with status {status}: {body}"
            ),
        }
    }
}

impl std::error::Error for OtelExportError {}

impl From<reqwest::Error> for OtelExportError {
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value)
    }
}

#[derive(Debug, Clone)]
pub struct OtelHttpExporter {
    config: OtelExporterConfig,
    client: reqwest::Client,
}

impl OtelHttpExporter {
    pub fn new(config: OtelExporterConfig) -> Result<Self, OtelExportError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()?;
        Ok(Self { config, client })
    }

    pub fn config(&self) -> &OtelExporterConfig {
        &self.config
    }

    pub async fn export_lineage(
        &self,
        events: &[LineageEvent],
    ) -> Result<OtelExportReport, OtelExportError> {
        let batch = build_otel_export_batch(events, &self.config)?;
        let traces_status = self
            .post_signal(
                "traces",
                &self.config.traces_endpoint,
                &batch.traces_payload,
            )
            .await?;
        let metrics_status = self
            .post_signal(
                "metrics",
                &self.config.metrics_endpoint,
                &batch.metrics_payload,
            )
            .await?;

        Ok(OtelExportReport {
            span_count: batch.span_count,
            metric_datapoint_count: batch.metric_datapoint_count,
            traces_status,
            metrics_status,
        })
    }

    async fn post_signal(
        &self,
        signal: &'static str,
        endpoint: &str,
        payload: &Value,
    ) -> Result<u16, OtelExportError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static(OTLP_JSON_CONTENT_TYPE),
        );
        for (name, value) in &self.config.headers {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| OtelExportError::InvalidHeader(name.clone()))?;
            let value = HeaderValue::from_str(value)
                .map_err(|_| OtelExportError::InvalidHeader(name.to_string()))?;
            headers.insert(name, value);
        }

        let response = self
            .client
            .post(endpoint)
            .headers(headers)
            .json(payload)
            .send()
            .await?;
        let status = response.status();
        if status.is_success() {
            return Ok(status.as_u16());
        }
        let body = response.text().await.unwrap_or_default();
        Err(OtelExportError::CollectorRejected {
            signal,
            status: status.as_u16(),
            body,
        })
    }
}

pub fn build_otel_export_batch(
    events: &[LineageEvent],
    config: &OtelExporterConfig,
) -> Result<OtelExportBatch, OtelExportError> {
    let validation = validate_lineage(events);
    if !validation.complete {
        return Err(OtelExportError::InvalidLineage(validation.violations));
    }

    let traces_payload = build_traces_payload(events, config);
    let metric_points = build_metric_points(events);
    let metric_datapoint_count = metric_points.len();
    let metrics_payload = build_metrics_payload(metric_points, config);

    Ok(OtelExportBatch {
        traces_payload,
        metrics_payload,
        span_count: events.len(),
        metric_datapoint_count,
    })
}

fn build_traces_payload(events: &[LineageEvent], config: &OtelExporterConfig) -> Value {
    let spans = events
        .iter()
        .map(|event| {
            json!({
                "traceId": otel_trace_id(&event.trace_id),
                "spanId": otel_span_id(&event.span_id),
                "parentSpanId": if event.parent_span_id.is_empty() {
                    Value::Null
                } else {
                    json!(otel_span_id(&event.parent_span_id))
                },
                "name": lineage_to_otel_span(event).name,
                "kind": span_kind(event.kind),
                "startTimeUnixNano": millis_to_nanos_string(event.started_ms),
                "endTimeUnixNano": millis_to_nanos_string(event.ended_ms),
                "attributes": span_attributes(event),
                "status": span_status(event.status),
            })
        })
        .collect::<Vec<_>>();

    json!({
        "resourceSpans": [{
            "resource": { "attributes": resource_attributes(config) },
            "scopeSpans": [{
                "scope": {
                    "name": SCOPE_NAME,
                    "version": SCOPE_VERSION,
                    "attributes": [{
                        "key": "corvid.otel_schema",
                        "value": { "stringValue": OTEL_SCHEMA_VERSION }
                    }]
                },
                "spans": spans
            }]
        }]
    })
}

fn build_metrics_payload(points: Vec<MetricPoint>, config: &OtelExporterConfig) -> Value {
    let metrics = points
        .into_iter()
        .map(|point| {
            json!({
                "name": point.name,
                "unit": point.unit,
                "description": point.description,
                point.data_kind: {
                    "dataPoints": [{
                        "attributes": point.attributes,
                        "timeUnixNano": millis_to_nanos_string(point.time_ms),
                        point.value_kind: point.value,
                    }]
                }
            })
        })
        .collect::<Vec<_>>();

    json!({
        "resourceMetrics": [{
            "resource": { "attributes": resource_attributes(config) },
            "scopeMetrics": [{
                "scope": {
                    "name": SCOPE_NAME,
                    "version": SCOPE_VERSION,
                    "attributes": [{
                        "key": "corvid.otel_schema",
                        "value": { "stringValue": OTEL_SCHEMA_VERSION }
                    }]
                },
                "metrics": metrics
            }]
        }]
    })
}

#[derive(Debug, Clone, PartialEq)]
struct MetricPoint {
    name: &'static str,
    unit: &'static str,
    description: &'static str,
    data_kind: &'static str,
    value_kind: &'static str,
    value: Value,
    attributes: Vec<Value>,
    time_ms: u64,
}

fn build_metric_points(events: &[LineageEvent]) -> Vec<MetricPoint> {
    let mut points = Vec::new();
    let mut seen_guarantees = BTreeSet::new();
    for event in events {
        let attrs = base_metric_attributes(event);
        match event.kind {
            LineageKind::Route => {
                points.push(count_metric(
                    "corvid.request.count",
                    "Backend request count",
                    attrs.clone(),
                    event.ended_ms,
                ));
                points.push(gauge_metric(
                    "corvid.request.duration_ms",
                    "ms",
                    "Backend request latency",
                    event.latency_ms as f64,
                    attrs.clone(),
                    event.ended_ms,
                ));
                if event.status == LineageStatus::Failed {
                    points.push(count_metric(
                        "corvid.request.error.count",
                        "Backend request errors",
                        attrs.clone(),
                        event.ended_ms,
                    ));
                }
            }
            LineageKind::Job => points.push(count_metric(
                "corvid.job.count",
                "Durable job count",
                attrs.clone(),
                event.ended_ms,
            )),
            LineageKind::Retry => points.push(count_metric(
                "corvid.job.retry.count",
                "Durable job retries",
                attrs.clone(),
                event.ended_ms,
            )),
            LineageKind::Prompt => {
                points.push(count_metric(
                    "corvid.llm.call.count",
                    "LLM call count",
                    attrs.clone(),
                    event.ended_ms,
                ));
                if event.tokens_in + event.tokens_out > 0 {
                    points.push(gauge_metric(
                        "corvid.llm.tokens",
                        "tokens",
                        "LLM token usage",
                        (event.tokens_in + event.tokens_out) as f64,
                        attrs.clone(),
                        event.ended_ms,
                    ));
                }
                if event.cost_usd > 0.0 {
                    points.push(gauge_metric(
                        "corvid.llm.cost_usd",
                        "USD",
                        "LLM cost",
                        event.cost_usd,
                        attrs.clone(),
                        event.ended_ms,
                    ));
                }
                if event.confidence > 0.0 {
                    points.push(gauge_metric(
                        "corvid.llm.confidence",
                        "1",
                        "LLM confidence",
                        event.confidence,
                        attrs.clone(),
                        event.ended_ms,
                    ));
                }
            }
            LineageKind::Tool => {
                points.push(count_metric(
                    "corvid.tool.call.count",
                    "Tool call count",
                    attrs.clone(),
                    event.ended_ms,
                ));
                if event.status == LineageStatus::Failed {
                    points.push(count_metric(
                        "corvid.tool.error.count",
                        "Tool call errors",
                        attrs.clone(),
                        event.ended_ms,
                    ));
                }
            }
            LineageKind::Approval => {
                points.push(count_metric(
                    "corvid.approval.created.count",
                    "Created approvals",
                    attrs.clone(),
                    event.started_ms,
                ));
                match event.status {
                    LineageStatus::Ok => points.push(count_metric(
                        "corvid.approval.approved.count",
                        "Approved approvals",
                        attrs.clone(),
                        event.ended_ms,
                    )),
                    LineageStatus::Denied => points.push(count_metric(
                        "corvid.approval.denied.count",
                        "Denied approvals",
                        attrs.clone(),
                        event.ended_ms,
                    )),
                    LineageStatus::Failed => points.push(count_metric(
                        "corvid.approval.expired.count",
                        "Expired approvals",
                        attrs.clone(),
                        event.ended_ms,
                    )),
                    _ => {}
                }
            }
            LineageKind::Db => points.push(count_metric(
                "corvid.db.query.count",
                "Database query count",
                attrs.clone(),
                event.ended_ms,
            )),
            LineageKind::Error => points.push(count_metric(
                "corvid.request.error.count",
                "Backend request errors",
                attrs.clone(),
                event.ended_ms,
            )),
            LineageKind::Agent | LineageKind::Eval | LineageKind::Review => {}
        }

        if !event.guarantee_id.is_empty()
            && event.status == LineageStatus::Failed
            && seen_guarantees.insert((event.trace_id.clone(), event.guarantee_id.clone()))
        {
            points.push(count_metric(
                "corvid.guarantee.violation.count",
                "Contract guarantee violations",
                attrs.clone(),
                event.ended_ms,
            ));
        }
        if !event.replay_key.is_empty() || event.status == LineageStatus::Replayed {
            points.push(count_metric(
                "corvid.replay.count",
                "Replay attempts",
                attrs,
                event.ended_ms,
            ));
        }
    }
    points
}

fn count_metric(
    name: &'static str,
    description: &'static str,
    attributes: Vec<Value>,
    time_ms: u64,
) -> MetricPoint {
    MetricPoint {
        name,
        unit: "1",
        description,
        data_kind: "sum",
        value_kind: "asInt",
        value: json!("1"),
        attributes,
        time_ms,
    }
}

fn gauge_metric(
    name: &'static str,
    unit: &'static str,
    description: &'static str,
    value: f64,
    attributes: Vec<Value>,
    time_ms: u64,
) -> MetricPoint {
    MetricPoint {
        name,
        unit,
        description,
        data_kind: "gauge",
        value_kind: "asDouble",
        value: json!(value),
        attributes,
        time_ms,
    }
}

fn resource_attributes(config: &OtelExporterConfig) -> Vec<Value> {
    let mut attrs = vec![
        string_attr("service.name", &config.service_name),
        string_attr("service.version", &config.service_version),
        string_attr("telemetry.sdk.language", "rust"),
        string_attr("telemetry.sdk.name", "corvid-runtime"),
        string_attr("telemetry.sdk.version", SCOPE_VERSION),
    ];
    if !config.deployment_environment.is_empty() {
        attrs.push(string_attr(
            "deployment.environment.name",
            &config.deployment_environment,
        ));
    }
    attrs
}

fn span_attributes(event: &LineageEvent) -> Vec<Value> {
    let mut attrs = lineage_to_otel_span(event)
        .attributes
        .into_iter()
        .map(|(key, value)| string_attr(&key, &value))
        .collect::<Vec<_>>();
    attrs.push(string_attr("corvid.name", &event.name));
    attrs.push(int_attr("corvid.started_ms", event.started_ms));
    attrs.push(int_attr("corvid.ended_ms", event.ended_ms));
    attrs.push(int_attr("corvid.latency_ms", event.latency_ms));
    attrs
}

fn base_metric_attributes(event: &LineageEvent) -> Vec<Value> {
    let mut attrs = vec![
        string_attr("corvid.trace_id", &event.trace_id),
        string_attr("corvid.kind", &kind_name(event.kind)),
        string_attr("corvid.status", &status_name(event.status)),
        string_attr("corvid.name", &event.name),
    ];
    for (key, value) in [
        ("corvid.tenant_id", &event.tenant_id),
        ("corvid.actor_id", &event.actor_id),
        ("corvid.guarantee_id", &event.guarantee_id),
        ("corvid.approval_id", &event.approval_id),
    ] {
        if !value.is_empty() {
            attrs.push(string_attr(key, value));
        }
    }
    attrs
}

fn span_kind(kind: LineageKind) -> &'static str {
    match kind {
        LineageKind::Route => "SPAN_KIND_SERVER",
        LineageKind::Db | LineageKind::Tool => "SPAN_KIND_CLIENT",
        _ => "SPAN_KIND_INTERNAL",
    }
}

fn span_status(status: LineageStatus) -> Value {
    match status {
        LineageStatus::Failed | LineageStatus::Denied => {
            json!({ "code": "STATUS_CODE_ERROR", "message": status_name(status) })
        }
        LineageStatus::Ok | LineageStatus::Replayed | LineageStatus::Redacted => {
            json!({ "code": "STATUS_CODE_OK" })
        }
        LineageStatus::PendingReview => json!({ "code": "STATUS_CODE_UNSET" }),
    }
}

fn kind_name(kind: LineageKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| format!("{kind:?}").to_lowercase())
}

fn status_name(status: LineageStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| format!("{status:?}").to_lowercase())
}

fn string_attr(key: &str, value: &str) -> Value {
    json!({ "key": key, "value": { "stringValue": value } })
}

fn int_attr(key: &str, value: u64) -> Value {
    json!({ "key": key, "value": { "intValue": value.to_string() } })
}

fn otel_trace_id(value: &str) -> String {
    hash_hex(value, 32)
}

fn otel_span_id(value: &str) -> String {
    hash_hex(value, 16)
}

fn hash_hex(value: &str, nibbles: usize) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut out = String::with_capacity(nibbles);
    for byte in digest {
        if out.len() >= nibbles {
            break;
        }
        out.push_str(&format!("{byte:02x}"));
    }
    out.truncate(nibbles);
    out
}

fn millis_to_nanos_string(ms: u64) -> String {
    ms.saturating_mul(1_000_000).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_lineage() -> Vec<LineageEvent> {
        let mut route = LineageEvent::root(
            "trace-1",
            LineageKind::Route,
            "POST /actions/follow-up/send",
            10,
        )
        .finish(LineageStatus::Ok, 80);
        route.tenant_id = "tenant-1".to_string();
        route.actor_id = "user-1".to_string();
        route.request_id = "req-1".to_string();
        route.replay_key = "route:trace-1".to_string();

        let mut prompt = LineageEvent::child(&route, LineageKind::Prompt, "draft_follow_up", 0, 20)
            .finish(LineageStatus::Ok, 50);
        prompt.tenant_id = route.tenant_id.clone();
        prompt.actor_id = route.actor_id.clone();
        prompt.model_id = "gpt-prod".to_string();
        prompt.model_fingerprint = "model-sha".to_string();
        prompt.prompt_hash = "prompt-sha".to_string();
        prompt.tokens_in = 100;
        prompt.tokens_out = 40;
        prompt.cost_usd = 0.03;

        let mut tool = LineageEvent::child(&route, LineageKind::Tool, "send_email", 1, 55)
            .finish(LineageStatus::Failed, 75);
        tool.tenant_id = route.tenant_id.clone();
        tool.actor_id = route.actor_id.clone();
        tool.guarantee_id = "approval.reachable_entrypoints_require_contract".to_string();
        tool.effect_ids = vec!["send_email".to_string()];
        tool.approval_id = "approval-1".to_string();
        tool.data_classes = vec!["private".to_string()];

        vec![route, prompt, tool]
    }

    #[test]
    fn export_batch_contains_otlp_trace_payload_for_all_lineage_events() {
        let config = OtelExporterConfig::local("executive-agent");
        let batch = build_otel_export_batch(&sample_lineage(), &config).unwrap();

        assert_eq!(batch.span_count, 3);
        let spans = batch.traces_payload["resourceSpans"][0]["scopeSpans"][0]["spans"]
            .as_array()
            .unwrap();
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0]["kind"], "SPAN_KIND_SERVER");
        assert_eq!(spans[2]["kind"], "SPAN_KIND_CLIENT");
        assert_eq!(spans[2]["status"]["code"], "STATUS_CODE_ERROR");
        assert_eq!(
            batch.traces_payload["resourceSpans"][0]["resource"]["attributes"][0]["key"],
            "service.name"
        );
    }

    #[test]
    fn export_batch_emits_phase_40_metrics_for_costs_retries_replay_and_errors() {
        let config = OtelExporterConfig::local("executive-agent");
        let batch = build_otel_export_batch(&sample_lineage(), &config).unwrap();
        let metrics = batch.metrics_payload["resourceMetrics"][0]["scopeMetrics"][0]["metrics"]
            .as_array()
            .unwrap();
        let names = metrics
            .iter()
            .map(|metric| metric["name"].as_str().unwrap())
            .collect::<BTreeSet<_>>();

        for expected in [
            "corvid.request.count",
            "corvid.request.duration_ms",
            "corvid.llm.call.count",
            "corvid.llm.tokens",
            "corvid.llm.cost_usd",
            "corvid.tool.call.count",
            "corvid.tool.error.count",
            "corvid.guarantee.violation.count",
            "corvid.replay.count",
        ] {
            assert!(names.contains(expected), "missing metric {expected}");
        }
        assert_eq!(batch.metric_datapoint_count, metrics.len());
    }

    #[test]
    fn export_batch_fails_closed_for_incomplete_lineage() {
        let config = OtelExporterConfig::local("executive-agent");
        let mut events = sample_lineage();
        events[1].parent_span_id = "missing".to_string();
        let err = build_otel_export_batch(&events, &config).unwrap_err();
        assert!(matches!(err, OtelExportError::InvalidLineage(_)));
    }
}
