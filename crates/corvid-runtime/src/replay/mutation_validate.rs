//! Mutation classification + validation for `try replay --mutate`.
//!
//! `validate_mutation` walks the trace at load time, finds the
//! Nth substitutable step (1-based), and verifies the requested
//! replacement value is type-compatible with the recorded result
//! at that step. `validate_mutation_replacement` is the typed
//! compatibility check (`bool` for approval, matching JSON kind
//! for tool/LLM results). `validate_mutation_result_pair` is the
//! call→result pair-shape check applied at substitution time.
//!
//! `mutated_json_result` / `mutated_llm_result` /
//! `mutated_approval_result` produce the substituted result
//! values once the mutation step is reached during replay. Each
//! validates the recorded call+result pair shape before
//! substituting in the user-supplied replacement.

use std::path::Path;

use corvid_trace_schema::TraceEvent;

use super::event_classify::{event_kind, json_kind, same_json_kind};
use super::result_factory::ReplayApprovalTraceOutcome;
use super::substitute::{is_dispatch_metadata, is_initial_metadata};
use super::{ReplayApprovalOutcome, ReplayMutation};
use crate::errors::RuntimeError;
use crate::llm::{LlmResponse, TokenUsage};

pub(super) fn validate_mutation(
    path: &Path,
    events: &[TraceEvent],
    step_1based: usize,
    replacement: &serde_json::Value,
) -> Result<(), RuntimeError> {
    if step_1based == 0 {
        return Err(RuntimeError::InvalidReplayMutation {
            step: 0,
            message: "step indices are 1-based".into(),
        });
    }
    let mut substitutable_step = 0usize;
    let mut index = 0usize;
    while index < events.len() {
        if is_initial_metadata(&events[index]) || is_dispatch_metadata(&events[index]) {
            index += 1;
            continue;
        }
        match &events[index] {
            TraceEvent::ToolCall { .. }
            | TraceEvent::LlmCall { .. }
            | TraceEvent::ApprovalRequest { .. } => {
                substitutable_step += 1;
                if substitutable_step == step_1based {
                    let result_index = next_non_metadata_index(events, index + 1).ok_or_else(|| {
                        RuntimeError::ReplayTraceLoad {
                            path: path.to_path_buf(),
                            message: format!(
                                "trace is missing a result event for substitutable step {step_1based}"
                            ),
                        }
                    })?;
                    validate_mutation_replacement(step_1based, &events[result_index], replacement)?;
                    return Ok(());
                }
            }
            _ => {}
        }
        index += 1;
    }
    Err(RuntimeError::InvalidReplayMutation {
        step: step_1based,
        message: format!("trace only has {substitutable_step} substitutable steps"),
    })
}

fn validate_mutation_replacement(
    step: usize,
    recorded_result: &TraceEvent,
    replacement: &serde_json::Value,
) -> Result<(), RuntimeError> {
    match recorded_result {
        TraceEvent::ApprovalResponse { .. } => {
            if replacement.is_boolean() {
                Ok(())
            } else {
                Err(RuntimeError::InvalidReplayMutation {
                    step,
                    message: format!(
                        "replacement for approval_response must be bool, got {}",
                        json_kind(replacement)
                    ),
                })
            }
        }
        TraceEvent::ToolResult { result, .. } | TraceEvent::LlmResult { result, .. } => {
            if same_json_kind(result, replacement) {
                Ok(())
            } else {
                Err(RuntimeError::InvalidReplayMutation {
                    step,
                    message: format!(
                        "replacement kind {} does not match recorded result kind {}",
                        json_kind(replacement),
                        json_kind(result)
                    ),
                })
            }
        }
        other => Err(RuntimeError::InvalidReplayMutation {
            step,
            message: format!(
                "expected a result event after the mutated call, got {}",
                event_kind(other)
            ),
        }),
    }
}

pub(super) fn next_non_metadata_index(
    events: &[TraceEvent],
    mut index: usize,
) -> Option<usize> {
    while index < events.len() {
        if is_dispatch_metadata(&events[index]) {
            index += 1;
        } else {
            return Some(index);
        }
    }
    None
}

pub(super) fn mutated_json_result(
    mutation: &ReplayMutation,
    recorded_call: &TraceEvent,
    recorded_result: TraceEvent,
) -> Result<serde_json::Value, RuntimeError> {
    validate_mutation_result_pair(recorded_call, &recorded_result, mutation.step_1based)?;
    Ok(mutation.replacement.clone())
}

pub(super) fn mutated_llm_result(
    mutation: &ReplayMutation,
    recorded_call: &TraceEvent,
    recorded_result: TraceEvent,
) -> Result<LlmResponse, RuntimeError> {
    Ok(LlmResponse::new(
        mutated_json_result(mutation, recorded_call, recorded_result)?,
        TokenUsage::default(),
    ))
}

pub(super) fn mutated_approval_result(
    mutation: &ReplayMutation,
    recorded_call: &TraceEvent,
    recorded_result: ReplayApprovalTraceOutcome,
) -> Result<ReplayApprovalOutcome, RuntimeError> {
    validate_mutation_result_pair(
        recorded_call,
        &recorded_result.response,
        mutation.step_1based,
    )?;
    let approved = mutation
        .replacement
        .as_bool()
        .ok_or_else(|| RuntimeError::InvalidReplayMutation {
            step: mutation.step_1based,
            message: "replacement for approval_response must be bool".into(),
        })?;
    Ok(ReplayApprovalOutcome {
        approved,
        decision: recorded_result.decision,
    })
}

fn validate_mutation_result_pair(
    recorded_call: &TraceEvent,
    recorded_result: &TraceEvent,
    step: usize,
) -> Result<(), RuntimeError> {
    let valid = matches!(
        (recorded_call, recorded_result),
        (TraceEvent::ToolCall { .. }, TraceEvent::ToolResult { .. })
            | (TraceEvent::LlmCall { .. }, TraceEvent::LlmResult { .. })
            | (
                TraceEvent::ApprovalRequest { .. },
                TraceEvent::ApprovalResponse { .. }
            )
    );
    if valid {
        Ok(())
    } else {
        Err(RuntimeError::InvalidReplayMutation {
            step,
            message: format!(
                "recorded step pairs {} with {}, expected matching call/result kinds",
                event_kind(recorded_call),
                event_kind(recorded_result)
            ),
        })
    }
}
