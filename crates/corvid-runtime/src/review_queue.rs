//! Human-review queue envelopes linked to lineage and audit evidence.

use crate::lineage::{LineageEvent, LineageKind, LineageStatus};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewReason {
    LowConfidence,
    HighRisk,
    DeniedApproval,
    SchemaFailure,
    GuaranteeViolation,
    OperatorEscalation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    Pending,
    Approved,
    Rejected,
    Escalated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewQueueRecord {
    pub review_id: String,
    pub trace_id: String,
    pub span_id: String,
    pub tenant_id: String,
    pub actor_id: String,
    pub reason: ReviewReason,
    pub status: ReviewStatus,
    pub cost_of_being_wrong: f64,
    pub source_prompt_hash: String,
    pub model_fingerprint: String,
    pub approval_id: String,
    pub replay_key: String,
    pub guarantee_id: String,
    pub audit_event_id: String,
    pub reviewer_actor_id: String,
    pub decision_note: String,
    pub created_ms: u64,
    pub resolved_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewQueueError {
    MissingTraceId,
    MissingSpanId,
    MissingTenantId,
    MissingActorId,
    MissingReviewer,
    AlreadyResolved,
}

impl std::fmt::Display for ReviewQueueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingTraceId => write!(f, "review record requires trace_id"),
            Self::MissingSpanId => write!(f, "review record requires span_id"),
            Self::MissingTenantId => write!(f, "review record requires tenant_id"),
            Self::MissingActorId => write!(f, "review record requires actor_id"),
            Self::MissingReviewer => write!(f, "review decision requires reviewer_actor_id"),
            Self::AlreadyResolved => write!(f, "review record is already resolved"),
        }
    }
}

impl std::error::Error for ReviewQueueError {}

pub fn create_review_record(
    event: &LineageEvent,
    reason: ReviewReason,
    cost_of_being_wrong: f64,
    created_ms: u64,
) -> Result<ReviewQueueRecord, ReviewQueueError> {
    validate_review_source(event)?;
    Ok(ReviewQueueRecord {
        review_id: review_id(event, reason, created_ms),
        trace_id: event.trace_id.clone(),
        span_id: event.span_id.clone(),
        tenant_id: event.tenant_id.clone(),
        actor_id: event.actor_id.clone(),
        reason,
        status: ReviewStatus::Pending,
        cost_of_being_wrong: if cost_of_being_wrong.is_finite() && cost_of_being_wrong > 0.0 {
            cost_of_being_wrong
        } else {
            0.0
        },
        source_prompt_hash: event.prompt_hash.clone(),
        model_fingerprint: event.model_fingerprint.clone(),
        approval_id: event.approval_id.clone(),
        replay_key: event.replay_key.clone(),
        guarantee_id: event.guarantee_id.clone(),
        audit_event_id: String::new(),
        reviewer_actor_id: String::new(),
        decision_note: String::new(),
        created_ms,
        resolved_ms: 0,
    })
}

pub fn resolve_review_record(
    record: &ReviewQueueRecord,
    status: ReviewStatus,
    reviewer_actor_id: impl Into<String>,
    audit_event_id: impl Into<String>,
    decision_note: impl Into<String>,
    resolved_ms: u64,
) -> Result<ReviewQueueRecord, ReviewQueueError> {
    if record.status != ReviewStatus::Pending {
        return Err(ReviewQueueError::AlreadyResolved);
    }
    let reviewer_actor_id = reviewer_actor_id.into();
    if reviewer_actor_id.trim().is_empty() {
        return Err(ReviewQueueError::MissingReviewer);
    }
    let mut resolved = record.clone();
    resolved.status = status;
    resolved.reviewer_actor_id = reviewer_actor_id;
    resolved.audit_event_id = audit_event_id.into();
    resolved.decision_note = decision_note.into();
    resolved.resolved_ms = resolved_ms;
    Ok(resolved)
}

pub fn review_lineage_event(record: &ReviewQueueRecord) -> LineageEvent {
    let mut event = LineageEvent::root(
        &record.trace_id,
        LineageKind::Review,
        format!("{:?}", record.reason).to_lowercase(),
        record.created_ms,
    )
    .finish(
        review_lineage_status(record.status),
        record.resolved_ms.max(record.created_ms),
    );
    event.span_id = record.review_id.clone();
    event.parent_span_id = record.span_id.clone();
    event.tenant_id = record.tenant_id.clone();
    event.actor_id = record.actor_id.clone();
    event.approval_id = record.approval_id.clone();
    event.replay_key = record.replay_key.clone();
    event.guarantee_id = record.guarantee_id.clone();
    event.prompt_hash = record.source_prompt_hash.clone();
    event.model_fingerprint = record.model_fingerprint.clone();
    event
}

fn validate_review_source(event: &LineageEvent) -> Result<(), ReviewQueueError> {
    if event.trace_id.trim().is_empty() {
        return Err(ReviewQueueError::MissingTraceId);
    }
    if event.span_id.trim().is_empty() {
        return Err(ReviewQueueError::MissingSpanId);
    }
    if event.tenant_id.trim().is_empty() {
        return Err(ReviewQueueError::MissingTenantId);
    }
    if event.actor_id.trim().is_empty() {
        return Err(ReviewQueueError::MissingActorId);
    }
    Ok(())
}

fn review_lineage_status(status: ReviewStatus) -> LineageStatus {
    match status {
        ReviewStatus::Pending | ReviewStatus::Escalated => LineageStatus::PendingReview,
        ReviewStatus::Approved => LineageStatus::Ok,
        ReviewStatus::Rejected => LineageStatus::Denied,
    }
}

fn review_id(event: &LineageEvent, reason: ReviewReason, created_ms: u64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(event.trace_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(event.span_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(format!("{reason:?}").as_bytes());
    hasher.update(b"\0");
    hasher.update(created_ms.to_le_bytes());
    format!("review-{}", hex_prefix(&hasher.finalize(), 16))
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

    fn source_event() -> LineageEvent {
        let mut event = LineageEvent::root("trace-1", LineageKind::Prompt, "draft", 10)
            .finish(LineageStatus::PendingReview, 20);
        event.tenant_id = "tenant-1".to_string();
        event.actor_id = "user-1".to_string();
        event.approval_id = "approval-1".to_string();
        event.replay_key = "replay-1".to_string();
        event.guarantee_id = "approval.reachable_entrypoints_require_contract".to_string();
        event.prompt_hash = "prompt-sha".to_string();
        event.model_fingerprint = "model-sha".to_string();
        event
    }

    #[test]
    fn review_record_links_to_trace_audit_and_model_evidence() {
        let record =
            create_review_record(&source_event(), ReviewReason::LowConfidence, 1000.0, 30).unwrap();
        assert!(record.review_id.starts_with("review-"));
        assert_eq!(record.trace_id, "trace-1");
        assert_eq!(record.approval_id, "approval-1");
        assert_eq!(record.replay_key, "replay-1");
        assert_eq!(record.source_prompt_hash, "prompt-sha");
        assert_eq!(record.model_fingerprint, "model-sha");
        assert_eq!(record.status, ReviewStatus::Pending);

        let resolved = resolve_review_record(
            &record,
            ReviewStatus::Approved,
            "reviewer-1",
            "audit-1",
            "looks grounded",
            50,
        )
        .unwrap();
        assert_eq!(resolved.reviewer_actor_id, "reviewer-1");
        assert_eq!(resolved.audit_event_id, "audit-1");
        assert_eq!(resolved.status, ReviewStatus::Approved);
    }

    #[test]
    fn review_record_requires_lineage_identity_and_one_resolution() {
        let mut event = source_event();
        event.tenant_id.clear();
        let err = create_review_record(&event, ReviewReason::HighRisk, 1.0, 10).unwrap_err();
        assert_eq!(err, ReviewQueueError::MissingTenantId);

        let record =
            create_review_record(&source_event(), ReviewReason::HighRisk, 1.0, 10).unwrap();
        let resolved = resolve_review_record(
            &record,
            ReviewStatus::Rejected,
            "reviewer-1",
            "audit-1",
            "unsafe",
            11,
        )
        .unwrap();
        let err = resolve_review_record(
            &resolved,
            ReviewStatus::Approved,
            "reviewer-2",
            "audit-2",
            "retry",
            12,
        )
        .unwrap_err();
        assert_eq!(err, ReviewQueueError::AlreadyResolved);
    }
}
