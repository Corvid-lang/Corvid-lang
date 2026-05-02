//! Event-kind classification + small display/JSON helpers used
//! across the replay pipeline.
//!
//! `event_kind` returns a stable `&'static str` slug for every
//! `TraceEvent` variant — the replay diagnostics use these to
//! describe what was expected vs. what showed up. `json_kind` is
//! the JSON-value analogue used by the mutation validator to
//! describe replacement-vs-recorded type mismatches.
//! `same_json_kind` is the same-shape predicate the mutation
//! validator pairs with `json_kind`. `event_to_json` and
//! `display_step` are the small one-liners the divergence
//! reporters use to render an event back to JSON / convert
//! 0-based cursor indices to 1-based step numbers.

use corvid_trace_schema::TraceEvent;

pub(super) fn event_to_json(event: &TraceEvent) -> serde_json::Value {
    serde_json::to_value(event)
        .unwrap_or_else(|_| serde_json::json!({ "debug": format!("{event:?}") }))
}

pub(super) fn event_kind(event: &TraceEvent) -> &'static str {
    match event {
        TraceEvent::SchemaHeader { .. } => "schema_header",
        TraceEvent::RunStarted { .. } => "run_started",
        TraceEvent::RunCompleted { .. } => "run_completed",
        TraceEvent::ToolCall { .. } => "tool_call",
        TraceEvent::ToolResult { .. } => "tool_result",
        TraceEvent::LlmCall { .. } => "llm_call",
        TraceEvent::LlmResult { .. } => "llm_result",
        TraceEvent::PromptCache { .. } => "prompt_cache",
        TraceEvent::ApprovalRequest { .. } => "approval_request",
        TraceEvent::ApprovalDecision { .. } => "approval_decision",
        TraceEvent::ApprovalResponse { .. } => "approval_response",
        TraceEvent::ApprovalTokenIssued { .. } => "approval_token_issued",
        TraceEvent::ApprovalScopeViolation { .. } => "approval_scope_violation",
        TraceEvent::HumanInputRequest { .. } => "human_input_request",
        TraceEvent::HumanInputResponse { .. } => "human_input_response",
        TraceEvent::HumanChoiceRequest { .. } => "human_choice_request",
        TraceEvent::HumanChoiceResponse { .. } => "human_choice_response",
        TraceEvent::HostEvent { .. } => "host_event",
        TraceEvent::SeedRead { .. } => "seed_read",
        TraceEvent::ClockRead { .. } => "clock_read",
        TraceEvent::ModelSelected { .. } => "model_selected",
        TraceEvent::ProgressiveEscalation { .. } => "progressive_escalation",
        TraceEvent::ProgressiveExhausted { .. } => "progressive_exhausted",
        TraceEvent::StreamUpgrade { .. } => "stream_upgrade",
        TraceEvent::AbVariantChosen { .. } => "ab_variant_chosen",
        TraceEvent::EnsembleVote { .. } => "ensemble_vote",
        TraceEvent::AdversarialPipelineCompleted { .. } => "adversarial_pipeline_completed",
        TraceEvent::AdversarialContradiction { .. } => "adversarial_contradiction",
        TraceEvent::ProvenanceEdge { .. } => "provenance_edge",
    }
}

pub(super) fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

pub(super) fn same_json_kind(left: &serde_json::Value, right: &serde_json::Value) -> bool {
    json_kind(left) == json_kind(right)
}

pub(super) fn display_step(index: usize) -> usize {
    index + 1
}
