//! Human-review queue envelopes linked to lineage and audit evidence.

use crate::lineage::{LineageEvent, LineageKind, LineageStatus};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

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

#[derive(Debug, Clone, PartialEq)]
pub struct ReviewQueuePolicy {
    pub min_confidence: f64,
    pub high_risk_cost_threshold: f64,
}

impl Default for ReviewQueuePolicy {
    fn default() -> Self {
        Self {
            min_confidence: 0.70,
            high_risk_cost_threshold: 1000.0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ReviewQueueRuntime {
    records: BTreeMap<String, ReviewQueueRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewQueueError {
    MissingTraceId,
    MissingSpanId,
    MissingTenantId,
    MissingActorId,
    MissingReviewer,
    AlreadyResolved,
    NotFound,
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
            Self::NotFound => write!(f, "review record was not found"),
        }
    }
}

impl std::error::Error for ReviewQueueError {}

impl ReviewQueueRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn submit(&mut self, record: ReviewQueueRecord) -> &ReviewQueueRecord {
        let review_id = record.review_id.clone();
        self.records.entry(review_id).or_insert(record)
    }

    pub fn enqueue_if_required(
        &mut self,
        event: &LineageEvent,
        cost_of_being_wrong: f64,
        policy: &ReviewQueuePolicy,
        now_ms: u64,
    ) -> Result<Option<&ReviewQueueRecord>, ReviewQueueError> {
        let reason = review_reason_for_event(event, cost_of_being_wrong, policy);
        let Some(reason) = reason else {
            return Ok(None);
        };
        let record = create_review_record(event, reason, cost_of_being_wrong, now_ms)?;
        Ok(Some(self.submit(record)))
    }

    pub fn pending(&self) -> Vec<&ReviewQueueRecord> {
        self.records
            .values()
            .filter(|record| record.status == ReviewStatus::Pending)
            .collect()
    }

    pub fn get(&self, review_id: &str) -> Option<&ReviewQueueRecord> {
        self.records.get(review_id)
    }

    pub fn resolve(
        &mut self,
        review_id: &str,
        status: ReviewStatus,
        reviewer_actor_id: impl Into<String>,
        audit_event_id: impl Into<String>,
        decision_note: impl Into<String>,
        resolved_ms: u64,
    ) -> Result<&ReviewQueueRecord, ReviewQueueError> {
        let current = self
            .records
            .get(review_id)
            .ok_or(ReviewQueueError::NotFound)?
            .clone();
        let resolved = resolve_review_record(
            &current,
            status,
            reviewer_actor_id,
            audit_event_id,
            decision_note,
            resolved_ms,
        )?;
        self.records.insert(review_id.to_string(), resolved);
        self.records
            .get(review_id)
            .ok_or(ReviewQueueError::NotFound)
    }
}

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

pub fn review_reason_for_event(
    event: &LineageEvent,
    cost_of_being_wrong: f64,
    policy: &ReviewQueuePolicy,
) -> Option<ReviewReason> {
    if event.status == LineageStatus::Denied {
        return Some(ReviewReason::DeniedApproval);
    }
    if event.status == LineageStatus::Failed && !event.guarantee_id.is_empty() {
        return Some(ReviewReason::GuaranteeViolation);
    }
    if event.status == LineageStatus::PendingReview {
        return Some(ReviewReason::OperatorEscalation);
    }
    if event.confidence.is_finite()
        && event.confidence > 0.0
        && event.confidence < policy.min_confidence
    {
        return Some(ReviewReason::LowConfidence);
    }
    if cost_of_being_wrong.is_finite() && cost_of_being_wrong >= policy.high_risk_cost_threshold {
        return Some(ReviewReason::HighRisk);
    }
    None
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

    #[test]
    fn queue_enqueues_low_confidence_and_high_risk_records() {
        let mut runtime = ReviewQueueRuntime::new();
        let policy = ReviewQueuePolicy {
            min_confidence: 0.80,
            high_risk_cost_threshold: 500.0,
        };
        let mut low_confidence = source_event();
        low_confidence.status = LineageStatus::Ok;
        low_confidence.confidence = 0.40;
        let low = runtime
            .enqueue_if_required(&low_confidence, 10.0, &policy, 100)
            .unwrap()
            .unwrap();
        assert_eq!(low.reason, ReviewReason::LowConfidence);

        let mut high_risk = source_event();
        high_risk.status = LineageStatus::Ok;
        high_risk.confidence = 0.95;
        high_risk.span_id = "span-high-risk".to_string();
        let high = runtime
            .enqueue_if_required(&high_risk, 1000.0, &policy, 101)
            .unwrap()
            .unwrap();
        assert_eq!(high.reason, ReviewReason::HighRisk);
        assert_eq!(runtime.pending().len(), 2);
    }

    #[test]
    fn queue_resolves_with_audit_evidence() {
        let mut runtime = ReviewQueueRuntime::new();
        let mut event = source_event();
        event.confidence = 0.30;
        let review_id = runtime
            .enqueue_if_required(&event, 42.0, &ReviewQueuePolicy::default(), 100)
            .unwrap()
            .unwrap()
            .review_id
            .clone();

        let resolved = runtime
            .resolve(
                &review_id,
                ReviewStatus::Rejected,
                "reviewer-1",
                "audit-1",
                "not enough evidence",
                120,
            )
            .unwrap()
            .clone();
        assert_eq!(resolved.status, ReviewStatus::Rejected);
        assert_eq!(resolved.audit_event_id, "audit-1");
        assert!(runtime.pending().is_empty());

        let lineage = review_lineage_event(&resolved);
        assert_eq!(lineage.kind, LineageKind::Review);
        assert_eq!(lineage.status, LineageStatus::Denied);
        assert_eq!(lineage.parent_span_id, event.span_id);
    }
}
