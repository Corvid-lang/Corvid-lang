//! OpenTelemetry semantic mapping for Corvid lineage events.
//!
//! This module owns names and field mapping only. The exporter transport lands
//! in the next slice so schema conformance can be tested independently from any
//! collector dependency.

use crate::lineage::LineageEvent;

pub const OTEL_SCHEMA_VERSION: &str = "corvid.otel.v1";

#[derive(Debug, Clone, PartialEq)]
pub struct OtelSpanMapping {
    pub name: String,
    pub attributes: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtelMetricMapping {
    pub name: &'static str,
    pub unit: &'static str,
    pub description: &'static str,
}

pub fn lineage_to_otel_span(event: &LineageEvent) -> OtelSpanMapping {
    let mut attributes = vec![
        ("corvid.schema".to_string(), OTEL_SCHEMA_VERSION.to_string()),
        ("corvid.trace_id".to_string(), event.trace_id.clone()),
        ("corvid.span_id".to_string(), event.span_id.clone()),
        (
            "corvid.parent_span_id".to_string(),
            event.parent_span_id.clone(),
        ),
        (
            "corvid.kind".to_string(),
            format!("{:?}", event.kind).to_lowercase(),
        ),
        (
            "corvid.status".to_string(),
            format!("{:?}", event.status).to_lowercase(),
        ),
        ("corvid.replay_key".to_string(), event.replay_key.clone()),
        (
            "corvid.idempotency_key".to_string(),
            event.idempotency_key.clone(),
        ),
        ("corvid.tenant_id".to_string(), event.tenant_id.clone()),
        ("corvid.actor_id".to_string(), event.actor_id.clone()),
        (
            "corvid.guarantee_id".to_string(),
            event.guarantee_id.clone(),
        ),
        ("corvid.effect_ids".to_string(), event.effect_ids.join(",")),
        ("corvid.approval_id".to_string(), event.approval_id.clone()),
        (
            "corvid.data_classes".to_string(),
            event.data_classes.join(","),
        ),
        ("corvid.cost_usd".to_string(), format_float(event.cost_usd)),
        ("corvid.tokens_in".to_string(), event.tokens_in.to_string()),
        (
            "corvid.tokens_out".to_string(),
            event.tokens_out.to_string(),
        ),
        (
            "corvid.confidence".to_string(),
            format_float(event.confidence),
        ),
        (
            "corvid.latency_ms".to_string(),
            event.latency_ms.to_string(),
        ),
        ("corvid.model_id".to_string(), event.model_id.clone()),
        (
            "corvid.model_fingerprint".to_string(),
            event.model_fingerprint.clone(),
        ),
        ("corvid.prompt_hash".to_string(), event.prompt_hash.clone()),
        (
            "corvid.retrieval_index_hash".to_string(),
            event.retrieval_index_hash.clone(),
        ),
        (
            "corvid.input_fingerprint".to_string(),
            event.input_fingerprint.clone(),
        ),
        (
            "corvid.output_fingerprint".to_string(),
            event.output_fingerprint.clone(),
        ),
        (
            "corvid.redaction_policy_hash".to_string(),
            event.redaction_policy_hash.clone(),
        ),
    ];
    attributes.retain(|(_, value)| !value.is_empty());
    OtelSpanMapping {
        name: format!("{:?} {}", event.kind, event.name).to_lowercase(),
        attributes,
    }
}

pub fn required_otel_metrics() -> Vec<OtelMetricMapping> {
    vec![
        metric("corvid.request.count", "1", "Backend request count"),
        metric(
            "corvid.request.duration_ms",
            "ms",
            "Backend request latency",
        ),
        metric("corvid.request.error.count", "1", "Backend request errors"),
        metric("corvid.job.count", "1", "Durable job count"),
        metric("corvid.job.retry.count", "1", "Durable job retries"),
        metric("corvid.llm.call.count", "1", "LLM call count"),
        metric("corvid.llm.tokens", "tokens", "LLM token usage"),
        metric("corvid.llm.cost_usd", "USD", "LLM cost"),
        metric("corvid.tool.call.count", "1", "Tool call count"),
        metric("corvid.tool.error.count", "1", "Tool call errors"),
        metric("corvid.approval.created.count", "1", "Created approvals"),
        metric("corvid.approval.approved.count", "1", "Approved approvals"),
        metric("corvid.approval.denied.count", "1", "Denied approvals"),
        metric("corvid.approval.expired.count", "1", "Expired approvals"),
        metric("corvid.db.query.count", "1", "Database query count"),
        metric(
            "corvid.guarantee.violation.count",
            "1",
            "Contract guarantee violations",
        ),
        metric("corvid.replay.count", "1", "Replay attempts"),
    ]
}

fn metric(name: &'static str, unit: &'static str, description: &'static str) -> OtelMetricMapping {
    OtelMetricMapping {
        name,
        unit,
        description,
    }
}

fn format_float(value: f64) -> String {
    if value.is_finite() && value != 0.0 {
        format!("{value:.6}")
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lineage::{LineageEvent, LineageKind, LineageStatus};

    #[test]
    fn lineage_span_mapping_includes_required_corvid_attributes() {
        let mut event = LineageEvent::root("trace-1", LineageKind::Tool, "send_email", 1)
            .finish(LineageStatus::Ok, 8);
        event.tenant_id = "tenant-1".to_string();
        event.actor_id = "user-1".to_string();
        event.replay_key = "replay-1".to_string();
        event.guarantee_id = "approval.reachable_entrypoints_require_contract".to_string();
        event.effect_ids = vec!["send_email".to_string()];
        event.approval_id = "approval-1".to_string();
        event.data_classes = vec!["private".to_string()];
        event.cost_usd = 0.02;
        event.tokens_in = 11;
        event.tokens_out = 7;

        let mapping = lineage_to_otel_span(&event);
        assert_eq!(mapping.name, "tool send_email");
        let attrs = mapping
            .attributes
            .iter()
            .cloned()
            .collect::<std::collections::BTreeMap<_, _>>();
        for key in [
            "corvid.trace_id",
            "corvid.span_id",
            "corvid.kind",
            "corvid.status",
            "corvid.replay_key",
            "corvid.tenant_id",
            "corvid.actor_id",
            "corvid.guarantee_id",
            "corvid.effect_ids",
            "corvid.approval_id",
            "corvid.data_classes",
            "corvid.cost_usd",
            "corvid.tokens_in",
            "corvid.tokens_out",
            "corvid.latency_ms",
        ] {
            assert!(attrs.contains_key(key), "missing {key}: {attrs:?}");
        }
        assert_eq!(
            attrs["corvid.guarantee_id"],
            "approval.reachable_entrypoints_require_contract"
        );
    }

    #[test]
    fn required_metrics_cover_phase_40_taxonomy() {
        let names = required_otel_metrics()
            .into_iter()
            .map(|metric| metric.name)
            .collect::<std::collections::BTreeSet<_>>();
        for expected in [
            "corvid.request.count",
            "corvid.job.retry.count",
            "corvid.llm.cost_usd",
            "corvid.tool.error.count",
            "corvid.approval.denied.count",
            "corvid.db.query.count",
            "corvid.guarantee.violation.count",
            "corvid.replay.count",
        ] {
            assert!(names.contains(expected), "missing metric {expected}");
        }
    }
}
