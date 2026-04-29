//! Runtime lineage model for Phase 40 observability.
//!
//! This is the lossless local model that later `observe`, OTel, eval-promotion,
//! drift, and review-queue slices consume. It intentionally sits beside the
//! older replay `TraceEvent` schema so replay compatibility does not block the
//! richer production observability model.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const LINEAGE_SCHEMA: &str = "corvid.trace.lineage.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineageKind {
    Route,
    Job,
    Agent,
    Prompt,
    Tool,
    Approval,
    Db,
    Retry,
    Error,
    Eval,
    Review,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineageStatus {
    Ok,
    Failed,
    Denied,
    PendingReview,
    Replayed,
    Redacted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineageEvent {
    pub schema: String,
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: String,
    pub kind: LineageKind,
    pub name: String,
    pub status: LineageStatus,
    pub started_ms: u64,
    pub ended_ms: u64,
    pub tenant_id: String,
    pub actor_id: String,
    pub request_id: String,
    pub replay_key: String,
    pub idempotency_key: String,
    pub guarantee_id: String,
    pub effect_ids: Vec<String>,
    pub approval_id: String,
    pub data_classes: Vec<String>,
    pub cost_usd: f64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub latency_ms: u64,
    pub model_id: String,
    pub model_fingerprint: String,
    pub prompt_hash: String,
    pub retrieval_index_hash: String,
    pub input_fingerprint: String,
    pub output_fingerprint: String,
    pub redaction_policy_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineageValidation {
    pub complete: bool,
    pub violations: Vec<String>,
}

impl LineageEvent {
    pub fn root(
        trace_id: impl Into<String>,
        kind: LineageKind,
        name: impl Into<String>,
        started_ms: u64,
    ) -> Self {
        let trace_id = trace_id.into();
        let name = name.into();
        Self::new(
            trace_id.clone(),
            lineage_span_id(&trace_id, "", kind, &name, 0),
            "",
            kind,
            name,
            started_ms,
        )
    }

    pub fn child(
        parent: &LineageEvent,
        kind: LineageKind,
        name: impl Into<String>,
        ordinal: u64,
        started_ms: u64,
    ) -> Self {
        let name = name.into();
        Self::new(
            parent.trace_id.clone(),
            lineage_span_id(&parent.trace_id, &parent.span_id, kind, &name, ordinal),
            parent.span_id.clone(),
            kind,
            name,
            started_ms,
        )
    }

    fn new(
        trace_id: String,
        span_id: String,
        parent_span_id: impl Into<String>,
        kind: LineageKind,
        name: String,
        started_ms: u64,
    ) -> Self {
        Self {
            schema: LINEAGE_SCHEMA.to_string(),
            trace_id,
            span_id,
            parent_span_id: parent_span_id.into(),
            kind,
            name,
            status: LineageStatus::Ok,
            started_ms,
            ended_ms: started_ms,
            tenant_id: String::new(),
            actor_id: String::new(),
            request_id: String::new(),
            replay_key: String::new(),
            idempotency_key: String::new(),
            guarantee_id: String::new(),
            effect_ids: Vec::new(),
            approval_id: String::new(),
            data_classes: Vec::new(),
            cost_usd: 0.0,
            tokens_in: 0,
            tokens_out: 0,
            latency_ms: 0,
            model_id: String::new(),
            model_fingerprint: String::new(),
            prompt_hash: String::new(),
            retrieval_index_hash: String::new(),
            input_fingerprint: String::new(),
            output_fingerprint: String::new(),
            redaction_policy_hash: String::new(),
        }
    }

    pub fn finish(mut self, status: LineageStatus, ended_ms: u64) -> Self {
        self.status = status;
        self.ended_ms = ended_ms;
        self.latency_ms = ended_ms.saturating_sub(self.started_ms);
        self
    }
}

pub fn lineage_span_id(
    trace_id: &str,
    parent_span_id: &str,
    kind: LineageKind,
    name: &str,
    ordinal: u64,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(trace_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(parent_span_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(format!("{kind:?}").as_bytes());
    hasher.update(b"\0");
    hasher.update(name.as_bytes());
    hasher.update(b"\0");
    hasher.update(ordinal.to_le_bytes());
    let digest = hasher.finalize();
    format!("span-{}", hex_prefix(&digest, 16))
}

pub fn validate_lineage(events: &[LineageEvent]) -> LineageValidation {
    let mut violations = Vec::new();
    if events.is_empty() {
        violations.push("empty_lineage".to_string());
        return LineageValidation {
            complete: false,
            violations,
        };
    }

    let trace_id = &events[0].trace_id;
    let mut roots = 0usize;
    let mut span_ids = BTreeSet::new();
    for event in events {
        if event.schema != LINEAGE_SCHEMA {
            violations.push(format!("unsupported_schema:{}", event.schema));
        }
        if &event.trace_id != trace_id {
            violations.push(format!("mixed_trace_id:{}", event.trace_id));
        }
        if event.span_id.trim().is_empty() {
            violations.push("missing_span_id".to_string());
        } else if !span_ids.insert(event.span_id.clone()) {
            violations.push(format!("duplicate_span_id:{}", event.span_id));
        }
        if event.parent_span_id.is_empty() {
            roots += 1;
        }
        if event.ended_ms < event.started_ms {
            violations.push(format!("negative_duration:{}", event.span_id));
        }
    }

    if roots != 1 {
        violations.push(format!("expected_one_root:{roots}"));
    }
    for event in events {
        if !event.parent_span_id.is_empty() && !span_ids.contains(&event.parent_span_id) {
            violations.push(format!(
                "missing_parent:{}:{}",
                event.span_id, event.parent_span_id
            ));
        }
    }

    LineageValidation {
        complete: violations.is_empty(),
        violations,
    }
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

    #[test]
    fn lineage_ids_are_stable_and_parented_across_backend_kinds() {
        let route = LineageEvent::root("trace-1", LineageKind::Route, "POST /work", 10);
        let job = LineageEvent::child(&route, LineageKind::Job, "daily_brief", 0, 11);
        let agent = LineageEvent::child(&job, LineageKind::Agent, "planner", 0, 12);
        let prompt = LineageEvent::child(&agent, LineageKind::Prompt, "summarize", 0, 13);
        let tool = LineageEvent::child(&agent, LineageKind::Tool, "send_email", 0, 14);
        let approval = LineageEvent::child(&tool, LineageKind::Approval, "SendEmail", 0, 15);
        let db = LineageEvent::child(&route, LineageKind::Db, "approval_queue", 0, 16);

        assert_eq!(
            route.span_id,
            LineageEvent::root("trace-1", LineageKind::Route, "POST /work", 99).span_id
        );
        assert_eq!(job.parent_span_id, route.span_id);
        assert_eq!(agent.parent_span_id, job.span_id);
        assert_eq!(prompt.parent_span_id, agent.span_id);
        assert_eq!(tool.parent_span_id, agent.span_id);
        assert_eq!(approval.parent_span_id, tool.span_id);
        assert_eq!(db.parent_span_id, route.span_id);

        let report = validate_lineage(&[
            route,
            job,
            agent,
            prompt.finish(LineageStatus::Ok, 30),
            tool,
            approval,
            db,
        ]);
        assert!(report.complete, "{report:?}");
    }

    #[test]
    fn lineage_validation_fails_closed_for_missing_parent_or_duplicate_root() {
        let route = LineageEvent::root("trace-1", LineageKind::Route, "GET /a", 1);
        let mut orphan = LineageEvent::child(&route, LineageKind::Tool, "send_email", 0, 2);
        orphan.parent_span_id = "span-missing".to_string();
        let second_root = LineageEvent::root("trace-1", LineageKind::Job, "daily", 3);

        let report = validate_lineage(&[route, orphan, second_root]);
        assert!(!report.complete);
        assert!(report
            .violations
            .iter()
            .any(|violation| violation.starts_with("expected_one_root")));
        assert!(report
            .violations
            .iter()
            .any(|violation| violation.starts_with("missing_parent")));
    }
}
