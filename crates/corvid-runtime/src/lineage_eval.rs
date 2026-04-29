//! Promote redacted lineage traces into deterministic eval fixtures.

use crate::lineage::{validate_lineage, LineageEvent};
use crate::lineage_redact::{redact_lineage_events, LineageRedactionPolicy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const LINEAGE_EVAL_FIXTURE_SCHEMA: &str = "corvid.eval.lineage_fixture.v1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineageEvalFixture {
    pub schema: String,
    pub trace_id: String,
    pub fixture_hash: String,
    pub redaction_policy_hash: String,
    pub source_event_count: usize,
    pub selected_span_ids: Vec<String>,
    pub expected_guarantee_ids: Vec<String>,
    pub expected_effect_ids: Vec<String>,
    pub replay_keys: Vec<String>,
    pub model_fingerprints: Vec<String>,
    pub prompt_hashes: Vec<String>,
    pub retrieval_index_hashes: Vec<String>,
    pub events: Vec<LineageEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineageEvalPromotionError {
    InvalidLineage(Vec<String>),
    EmptyTrace,
    MissingRedactionPolicyHash,
}

impl std::fmt::Display for LineageEvalPromotionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLineage(violations) => {
                write!(f, "lineage is incomplete: {}", violations.join(", "))
            }
            Self::EmptyTrace => write!(f, "lineage trace is empty"),
            Self::MissingRedactionPolicyHash => {
                write!(
                    f,
                    "redacted lineage did not include a redaction policy hash"
                )
            }
        }
    }
}

impl std::error::Error for LineageEvalPromotionError {}

pub fn promote_lineage_events_to_eval(
    events: &[LineageEvent],
    policy: &LineageRedactionPolicy,
) -> Result<LineageEvalFixture, LineageEvalPromotionError> {
    if events.is_empty() {
        return Err(LineageEvalPromotionError::EmptyTrace);
    }
    let validation = validate_lineage(events);
    if !validation.complete {
        return Err(LineageEvalPromotionError::InvalidLineage(
            validation.violations,
        ));
    }

    let redacted = redact_lineage_events(events, policy);
    let redaction_policy_hash = redacted
        .first()
        .map(|event| event.redaction_policy_hash.clone())
        .filter(|hash| !hash.is_empty())
        .ok_or(LineageEvalPromotionError::MissingRedactionPolicyHash)?;
    let trace_id = redacted[0].trace_id.clone();

    let mut fixture = LineageEvalFixture {
        schema: LINEAGE_EVAL_FIXTURE_SCHEMA.to_string(),
        trace_id,
        fixture_hash: String::new(),
        redaction_policy_hash,
        source_event_count: events.len(),
        selected_span_ids: redacted.iter().map(|event| event.span_id.clone()).collect(),
        expected_guarantee_ids: collect_unique(redacted.iter().map(|event| &event.guarantee_id)),
        expected_effect_ids: collect_unique(
            redacted.iter().flat_map(|event| event.effect_ids.iter()),
        ),
        replay_keys: collect_unique(redacted.iter().map(|event| &event.replay_key)),
        model_fingerprints: collect_unique(redacted.iter().map(|event| &event.model_fingerprint)),
        prompt_hashes: collect_unique(redacted.iter().map(|event| &event.prompt_hash)),
        retrieval_index_hashes: collect_unique(
            redacted.iter().map(|event| &event.retrieval_index_hash),
        ),
        events: redacted,
    };
    fixture.fixture_hash = lineage_eval_fixture_hash(&fixture);
    Ok(fixture)
}

pub fn lineage_eval_fixture_hash(fixture: &LineageEvalFixture) -> String {
    let mut clone = fixture.clone();
    clone.fixture_hash.clear();
    let bytes = serde_json::to_vec(&clone).unwrap_or_default();
    format!("sha256:{}", hex_prefix(&Sha256::digest(bytes), 16))
}

fn collect_unique<'a>(values: impl Iterator<Item = &'a String>) -> Vec<String> {
    values
        .filter(|value| !value.is_empty())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
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
    fn promotion_builds_deterministic_redacted_fixture() {
        let mut route = LineageEvent::root("trace-1", LineageKind::Route, "POST /send", 10)
            .finish(LineageStatus::Ok, 100);
        route.replay_key = "replay-secret".to_string();
        let mut tool =
            LineageEvent::child(&route, LineageKind::Tool, "send alice@example.com", 0, 20)
                .finish(LineageStatus::Failed, 60);
        tool.guarantee_id = "approval.reachable_entrypoints_require_contract".to_string();
        tool.effect_ids = vec!["send_email".to_string()];
        tool.approval_id = "approval-secret".to_string();
        tool.model_fingerprint = "model-secret".to_string();
        tool.prompt_hash = "prompt-secret".to_string();

        let policy = LineageRedactionPolicy::production_default();
        let fixture_a =
            promote_lineage_events_to_eval(&[route.clone(), tool.clone()], &policy).unwrap();
        let fixture_b = promote_lineage_events_to_eval(&[route, tool], &policy).unwrap();

        assert_eq!(fixture_a, fixture_b);
        assert_eq!(fixture_a.schema, LINEAGE_EVAL_FIXTURE_SCHEMA);
        assert_eq!(fixture_a.source_event_count, 2);
        assert_eq!(
            fixture_a.expected_guarantee_ids,
            vec!["approval.reachable_entrypoints_require_contract".to_string()]
        );
        assert_eq!(
            fixture_a.expected_effect_ids,
            vec!["send_email".to_string()]
        );
        assert_eq!(
            fixture_a.fixture_hash,
            lineage_eval_fixture_hash(&fixture_a)
        );
        let json = serde_json::to_string(&fixture_a).unwrap();
        assert!(!json.contains("alice@example.com"));
        assert!(!json.contains("approval-secret"));
        assert!(!json.contains("model-secret"));
        assert!(!json.contains("prompt-secret"));
    }

    #[test]
    fn promotion_rejects_incomplete_lineage() {
        let root = LineageEvent::root("trace-1", LineageKind::Route, "GET /", 1);
        let mut orphan = LineageEvent::child(&root, LineageKind::Tool, "send", 0, 2);
        orphan.parent_span_id = "missing".to_string();

        let err =
            promote_lineage_events_to_eval(&[root, orphan], &LineageRedactionPolicy::default())
                .unwrap_err();
        assert!(matches!(err, LineageEvalPromotionError::InvalidLineage(_)));
    }
}
