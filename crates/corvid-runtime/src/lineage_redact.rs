//! Deterministic redaction for Phase 40 lineage traces.
//!
//! This module only owns the lineage privacy transform. It preserves trace
//! topology and statuses so observe, eval promotion, and OTel export can keep
//! correlating spans after sensitive values are removed.

use crate::lineage::LineageEvent;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageRedactionPolicy {
    pub name: String,
    pub redact_tenant_id: bool,
    pub redact_actor_id: bool,
    pub redact_request_id: bool,
    pub redact_replay_key: bool,
    pub redact_idempotency_key: bool,
    pub redact_approval_id: bool,
    pub redact_model_fingerprint: bool,
    pub redact_prompt_hash: bool,
    pub redact_retrieval_index_hash: bool,
    pub redact_input_fingerprint: bool,
    pub redact_output_fingerprint: bool,
}

impl LineageRedactionPolicy {
    pub fn production_default() -> Self {
        Self {
            name: "corvid.production.lineage.v1".to_string(),
            redact_tenant_id: true,
            redact_actor_id: true,
            redact_request_id: true,
            redact_replay_key: true,
            redact_idempotency_key: true,
            redact_approval_id: true,
            redact_model_fingerprint: true,
            redact_prompt_hash: true,
            redact_retrieval_index_hash: true,
            redact_input_fingerprint: true,
            redact_output_fingerprint: true,
        }
    }
}

impl Default for LineageRedactionPolicy {
    fn default() -> Self {
        Self::production_default()
    }
}

pub fn lineage_redaction_policy_hash(policy: &LineageRedactionPolicy) -> String {
    let bytes = serde_json::to_vec(policy).unwrap_or_default();
    format!("sha256:{}", hex_prefix(&Sha256::digest(bytes), 16))
}

pub fn redact_lineage_events(
    events: &[LineageEvent],
    policy: &LineageRedactionPolicy,
) -> Vec<LineageEvent> {
    events
        .iter()
        .map(|event| redact_lineage_event(event, policy))
        .collect()
}

pub fn redact_lineage_event(event: &LineageEvent, policy: &LineageRedactionPolicy) -> LineageEvent {
    let policy_hash = lineage_redaction_policy_hash(policy);
    let mut redacted = event.clone();
    redacted.name = redact_sensitive_text("name", &redacted.name);
    redacted.tenant_id = redact_field(policy.redact_tenant_id, "tenant", &redacted.tenant_id);
    redacted.actor_id = redact_field(policy.redact_actor_id, "actor", &redacted.actor_id);
    redacted.request_id = redact_field(policy.redact_request_id, "request", &redacted.request_id);
    redacted.replay_key = redact_field(policy.redact_replay_key, "replay", &redacted.replay_key);
    redacted.idempotency_key = redact_field(
        policy.redact_idempotency_key,
        "idempotency",
        &redacted.idempotency_key,
    );
    redacted.approval_id =
        redact_field(policy.redact_approval_id, "approval", &redacted.approval_id);
    redacted.effect_ids = redacted
        .effect_ids
        .iter()
        .map(|value| redact_sensitive_text("effect", value))
        .collect();
    redacted.data_classes = redacted
        .data_classes
        .iter()
        .map(|value| redact_sensitive_text("data_class", value))
        .collect();
    redacted.model_id = redact_sensitive_text("model", &redacted.model_id);
    redacted.model_fingerprint = redact_field(
        policy.redact_model_fingerprint,
        "model_fingerprint",
        &redacted.model_fingerprint,
    );
    redacted.prompt_hash = redact_field(
        policy.redact_prompt_hash,
        "prompt_hash",
        &redacted.prompt_hash,
    );
    redacted.retrieval_index_hash = redact_field(
        policy.redact_retrieval_index_hash,
        "retrieval_index_hash",
        &redacted.retrieval_index_hash,
    );
    redacted.input_fingerprint = redact_field(
        policy.redact_input_fingerprint,
        "input_fingerprint",
        &redacted.input_fingerprint,
    );
    redacted.output_fingerprint = redact_field(
        policy.redact_output_fingerprint,
        "output_fingerprint",
        &redacted.output_fingerprint,
    );
    redacted.redaction_policy_hash = policy_hash;
    redacted
}

fn redact_field(enabled: bool, kind: &str, value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    if enabled {
        return redaction_token(kind, value);
    }
    redact_sensitive_text(kind, value)
}

fn redact_sensitive_text(kind: &str, value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    if looks_sensitive(value) {
        redaction_token(kind, value)
    } else {
        value.to_string()
    }
}

fn looks_sensitive(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("bearer ")
        || lower.contains("api_key")
        || lower.contains("apikey")
        || lower.contains("access_token")
        || lower.contains("session_id")
        || lower.starts_with("sk-")
        || contains_email(value)
        || contains_ssn_like_id(value)
        || contains_phone_like_value(value)
}

fn contains_email(value: &str) -> bool {
    value
        .split(|ch: char| ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | '\''))
        .any(|token| {
            let at = token.find('@');
            at.is_some() && token[at.unwrap() + 1..].contains('.')
        })
}

fn contains_ssn_like_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.windows(11).any(|window| {
        window[0].is_ascii_digit()
            && window[1].is_ascii_digit()
            && window[2].is_ascii_digit()
            && window[3] == b'-'
            && window[4].is_ascii_digit()
            && window[5].is_ascii_digit()
            && window[6] == b'-'
            && window[7].is_ascii_digit()
            && window[8].is_ascii_digit()
            && window[9].is_ascii_digit()
            && window[10].is_ascii_digit()
    })
}

fn contains_phone_like_value(value: &str) -> bool {
    let digits = value.chars().filter(|ch| ch.is_ascii_digit()).count();
    digits >= 10 && value.chars().any(|ch| matches!(ch, '+' | '(' | ')' | '-'))
}

fn redaction_token(kind: &str, value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    format!("<redacted:{kind}:sha256:{}>", hex_prefix(&digest, 12))
}

fn hex_prefix(bytes: &[u8], nibbles: usize) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(nibbles);
    for byte in bytes {
        if out.len() >= nibbles {
            break;
        }
        out.push(HEX[(byte >> 4) as usize] as char);
        if out.len() >= nibbles {
            break;
        }
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lineage::{LineageKind, LineageStatus};

    #[test]
    fn redaction_preserves_topology_and_redacts_identifiers_deterministically() {
        let mut route = crate::lineage::LineageEvent::root(
            "trace-1",
            LineageKind::Route,
            "POST /users/alice@example.com/send",
            10,
        )
        .finish(LineageStatus::Failed, 20);
        route.tenant_id = "tenant-raw".to_string();
        route.actor_id = "alice@example.com".to_string();
        route.request_id = "req-secret".to_string();
        route.replay_key = "replay-key".to_string();
        route.approval_id = "approval-raw".to_string();

        let policy = LineageRedactionPolicy::production_default();
        let redacted_once = redact_lineage_event(&route, &policy);
        let redacted_twice = redact_lineage_event(&route, &policy);

        assert_eq!(redacted_once, redacted_twice);
        assert_eq!(redacted_once.trace_id, route.trace_id);
        assert_eq!(redacted_once.span_id, route.span_id);
        assert_eq!(redacted_once.parent_span_id, route.parent_span_id);
        assert_eq!(redacted_once.status, LineageStatus::Failed);
        assert!(redacted_once
            .tenant_id
            .starts_with("<redacted:tenant:sha256:"));
        assert!(redacted_once
            .actor_id
            .starts_with("<redacted:actor:sha256:"));
        assert!(redacted_once.name.starts_with("<redacted:name:sha256:"));
        assert!(redacted_once.redaction_policy_hash.starts_with("sha256:"));
    }

    #[test]
    fn redaction_removes_obvious_secrets_from_serialized_lineage() {
        let mut event = crate::lineage::LineageEvent::root(
            "trace-2",
            LineageKind::Tool,
            "Bearer sk-live-123 for 123-45-6789",
            1,
        );
        event.data_classes = vec!["phone +1 (415) 555-0100".to_string()];
        event.effect_ids = vec!["send_email".to_string()];
        event.model_fingerprint = "model-secret".to_string();
        event.prompt_hash = "prompt-secret".to_string();

        let policy = LineageRedactionPolicy::production_default();
        let redacted = redact_lineage_events(&[event], &policy);
        let json = serde_json::to_string(&redacted).unwrap();

        assert!(!json.contains("sk-live-123"));
        assert!(!json.contains("123-45-6789"));
        assert!(!json.contains("415"));
        assert!(!json.contains("model-secret"));
        assert!(!json.contains("prompt-secret"));
        assert!(json.contains("<redacted:name:sha256:"));
        assert!(json.contains("<redacted:data_class:sha256:"));
    }
}
