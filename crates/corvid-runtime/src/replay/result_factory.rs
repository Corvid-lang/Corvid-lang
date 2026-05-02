//! JSON-result production for replayed tool / LLM / approval
//! events.
//!
//! Each `replayed_*_result` helper takes the next `TraceEvent`
//! popped off the recorded trace and converts it back into the
//! shape the live runtime would have returned — tool/LLM
//! results map to their `serde_json::Value` body; approval
//! responses map to the `ReplayApprovalOutcome` typed pair
//! `{approved, decision}` carrying the matching
//! `ApprovalDecision` event when present.
//!
//! `next_approval_outcome_event` walks the cursor far enough to
//! grab the `ApprovalDecision` + `ApprovalResponse` pair the
//! recorder emits as a unit. `coerce_json_to_bool` is the
//! mutation-replacement compatibility shim that lets a
//! `try replay --mutate` cycle stand in any JSON value for a
//! pre-recorded boolean approval.

use corvid_trace_schema::TraceEvent;

use super::cursor::TraceCursor;
use super::diverge::ReplayDivergence;
use super::{ReplayApprovalDecision, ReplayApprovalOutcome};
use crate::errors::RuntimeError;

pub(super) struct ReplayApprovalTraceOutcome {
    pub(super) decision: Option<ReplayApprovalDecision>,
    pub(super) response: TraceEvent,
}

pub(super) fn next_approval_outcome_event(
    cursor: &mut TraceCursor,
    events: &[TraceEvent],
) -> ReplayApprovalTraceOutcome {
    match cursor.next_event(events) {
        TraceEvent::ApprovalDecision {
            accepted,
            decider,
            rationale,
            ..
        } => ReplayApprovalTraceOutcome {
            decision: Some(ReplayApprovalDecision {
                accepted,
                decider,
                rationale,
            }),
            response: cursor.next_event(events),
        },
        other => ReplayApprovalTraceOutcome {
            decision: None,
            response: other,
        },
    }
}

pub(super) fn replayed_json_result(
    tool: &str,
    event: TraceEvent,
) -> Result<serde_json::Value, RuntimeError> {
    match event {
        TraceEvent::ToolResult { result, .. } | TraceEvent::LlmResult { result, .. } => Ok(result),
        TraceEvent::ApprovalResponse { approved, .. } => Ok(serde_json::json!(approved)),
        other => Err(RuntimeError::ReplayDivergence(ReplayDivergence {
            step: 0,
            expected: other,
            got_kind: "tool_result",
            got_description: format!("tool={tool}"),
        })),
    }
}

pub(super) fn replayed_approval_result(
    label: &str,
    outcome: ReplayApprovalTraceOutcome,
) -> Result<ReplayApprovalOutcome, RuntimeError> {
    match outcome.response {
        TraceEvent::ApprovalResponse { approved, .. } => Ok(ReplayApprovalOutcome {
            approved,
            decision: outcome.decision,
        }),
        TraceEvent::ToolResult { result, .. } | TraceEvent::LlmResult { result, .. } => {
            Ok(ReplayApprovalOutcome {
                approved: coerce_json_to_bool(&result),
                decision: outcome.decision,
            })
        }
        other => Err(RuntimeError::ReplayDivergence(ReplayDivergence {
            step: 0,
            expected: other,
            got_kind: "approval_response",
            got_description: format!("label={label}"),
        })),
    }
}

pub(super) fn replayed_event_json(event: &TraceEvent) -> serde_json::Value {
    match event {
        TraceEvent::ToolResult { result, .. } | TraceEvent::LlmResult { result, .. } => {
            result.clone()
        }
        TraceEvent::ApprovalResponse { approved, .. } => serde_json::json!(approved),
        _ => serde_json::Value::Null,
    }
}

pub(super) fn coerce_json_to_bool(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Bool(v) => *v,
        serde_json::Value::Null => false,
        serde_json::Value::Number(n) => n.as_i64().map(|v| v != 0).unwrap_or(true),
        serde_json::Value::String(s) => !s.is_empty(),
        serde_json::Value::Array(values) => !values.is_empty(),
        serde_json::Value::Object(map) => !map.is_empty(),
    }
}
