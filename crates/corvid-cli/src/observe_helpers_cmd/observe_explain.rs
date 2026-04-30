//! `corvid observe explain <trace-id>` — RAG-grounded incident
//! root cause from a typed lineage trace.
//!
//! Walks a single trace, identifies the first non-`ok` event,
//! classifies its root cause from the typed status + guarantee_id,
//! and suggests next steps. The output's `sources` array carries
//! `(trace_id, span_id)` pairs for every event the analysis
//! consulted — the `Grounded<T>` shape at the JSON layer.

use anyhow::{anyhow, Result};
use corvid_runtime::lineage::{LineageEvent, LineageStatus};
use serde_json::Value;
use std::path::PathBuf;

use super::{read_lineage_input, select_run, source_descriptor};

#[derive(Debug, Clone)]
pub struct ObserveExplainArgs {
    pub trace_dir: PathBuf,
    pub trace_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IncidentExplanation {
    pub trace_id: String,
    pub root_cause_kind: String,
    pub first_failed_event: Option<EventSummary>,
    pub affected_guarantees: Vec<String>,
    pub suggested_next_steps: Vec<String>,
    pub sources: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EventSummary {
    pub name: String,
    pub kind: String,
    pub status: String,
    pub guarantee_id: String,
    pub latency_ms: u64,
    pub cost_usd: f64,
    pub trace_id: String,
    pub span_id: String,
}

/// Walk a single trace, identify the first non-`ok` event, classify
/// its root cause from the typed status + guarantee_id, and
/// suggest next steps. The output's `sources` array carries
/// `(trace_id, span_id)` pairs for every event the analysis
/// consulted — the `Grounded<T>` shape.
pub fn run_observe_explain(args: ObserveExplainArgs) -> Result<IncidentExplanation> {
    let events = read_lineage_input(&args.trace_dir)?;
    let run = select_run(&events, &args.trace_id);
    if run.is_empty() {
        return Err(anyhow!(
            "no lineage events found for trace `{}` under `{}`",
            args.trace_id,
            args.trace_dir.display()
        ));
    }
    let mut affected = Vec::new();
    let mut sources = Vec::new();
    let mut first_failed: Option<&LineageEvent> = None;
    for event in &run {
        sources.push(source_descriptor(event));
        if !matches!(event.status, LineageStatus::Ok | LineageStatus::Replayed) {
            if first_failed.is_none() {
                first_failed = Some(event);
            }
            if !event.guarantee_id.is_empty() && !affected.contains(&event.guarantee_id) {
                affected.push(event.guarantee_id.clone());
            }
        }
    }

    let (root_cause_kind, suggested) = match first_failed {
        None => (
            "ok".to_string(),
            vec![
                "all events succeeded; nothing to do".to_string(),
            ],
        ),
        Some(event) => match event.status {
            LineageStatus::Denied => (
                "approval_denied".to_string(),
                vec![
                    format!(
                        "review the approval contract for `{}` — the approver denied the request",
                        event.name
                    ),
                    "if the denial reflects a policy gap, update the approval contract's required_role".to_string(),
                    "if the denial was correct, capture it as a positive eval case so the agent learns the boundary".to_string(),
                ],
            ),
            LineageStatus::Failed => (
                "tool_failure".to_string(),
                vec![
                    format!(
                        "the `{}` event failed at attempt {}; check the connector trace for the underlying provider error",
                        event.name, event.span_id
                    ),
                    "if this is a 429/5xx pattern, raise the connector's rate-limit window or add Retry-After honouring".to_string(),
                    "if the failure recurs across runs, promote this trace to an eval fixture via `corvid eval promote`".to_string(),
                ],
            ),
            LineageStatus::Redacted => (
                "redaction_blocked".to_string(),
                vec![
                    "the redaction policy stripped data the agent needed; review the policy's `redact_*` flags".to_string(),
                ],
            ),
            LineageStatus::PendingReview => (
                "pending_review".to_string(),
                vec![
                    "the trace is waiting on a human-review queue resolution".to_string(),
                ],
            ),
            _ => (
                "unclassified".to_string(),
                vec!["non-OK status with no specific classifier".to_string()],
            ),
        },
    };

    Ok(IncidentExplanation {
        trace_id: args.trace_id,
        root_cause_kind,
        first_failed_event: first_failed.map(event_summary),
        affected_guarantees: affected,
        suggested_next_steps: suggested,
        sources,
    })
}

fn event_summary(event: &LineageEvent) -> EventSummary {
    EventSummary {
        name: event.name.clone(),
        kind: format!("{:?}", event.kind).to_lowercase(),
        status: format!("{:?}", event.status).to_lowercase(),
        guarantee_id: event.guarantee_id.clone(),
        latency_ms: event.latency_ms,
        cost_usd: event.cost_usd,
        trace_id: event.trace_id.clone(),
        span_id: event.span_id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe_helpers_cmd::test_support::{ev, write_lineage};
    use corvid_runtime::lineage::{LineageKind, LineageStatus};

    /// Slice 40K: `observe explain` classifies a Failed tool call
    /// as `tool_failure`, surfaces the affected guarantee id, and
    /// suggests the connector + retry-after track.
    #[test]
    fn observe_explain_classifies_tool_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.lineage.jsonl");
        write_lineage(
            &path,
            &[
                ev(
                    LineageKind::Tool,
                    "get_order",
                    "t1",
                    "s1",
                    LineageStatus::Ok,
                    "",
                    0.0,
                ),
                ev(
                    LineageKind::Tool,
                    "issue_refund",
                    "t1",
                    "s2",
                    LineageStatus::Failed,
                    "connector.rate_limit_respects_provider",
                    0.001,
                ),
            ],
        );
        let report = run_observe_explain(ObserveExplainArgs {
            trace_dir: dir.path().to_path_buf(),
            trace_id: "t1".to_string(),
        })
        .unwrap();
        assert_eq!(report.root_cause_kind, "tool_failure");
        assert!(report
            .affected_guarantees
            .contains(&"connector.rate_limit_respects_provider".to_string()));
        assert_eq!(report.first_failed_event.unwrap().name, "issue_refund");
        assert!(report.suggested_next_steps.iter().any(|s| s.contains("connector")));
    }

    /// Slice 40K: `observe explain` classifies a Denied event as
    /// `approval_denied` with appropriate suggestions.
    #[test]
    fn observe_explain_classifies_approval_denied() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.lineage.jsonl");
        write_lineage(
            &path,
            &[ev(
                LineageKind::Approval,
                "RefundApproval",
                "t1",
                "s1",
                LineageStatus::Denied,
                "approval.policy_clause_static_check",
                0.0,
            )],
        );
        let report = run_observe_explain(ObserveExplainArgs {
            trace_dir: dir.path().to_path_buf(),
            trace_id: "t1".to_string(),
        })
        .unwrap();
        assert_eq!(report.root_cause_kind, "approval_denied");
    }

    /// Slice 40K adversarial: an unknown trace id surfaces a
    /// clear diagnostic, not an empty report.
    #[test]
    fn observe_explain_unknown_trace_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.lineage.jsonl");
        write_lineage(
            &path,
            &[ev(
                LineageKind::Tool,
                "x",
                "t1",
                "s1",
                LineageStatus::Ok,
                "",
                0.0,
            )],
        );
        let err = run_observe_explain(ObserveExplainArgs {
            trace_dir: dir.path().to_path_buf(),
            trace_id: "no-such-trace".to_string(),
        })
        .unwrap_err();
        assert!(err.to_string().contains("no lineage events"));
    }
}
