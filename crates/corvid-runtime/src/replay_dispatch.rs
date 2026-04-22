use crate::errors::RuntimeError;
use corvid_trace_schema::{read_events_from_path, validate_supported_schema, TraceEvent};
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct ReplayDispatchArm {
    pub pattern: ReplayDispatchPattern,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReplayDispatchPattern {
    Llm {
        prompt: String,
    },
    Tool {
        tool: String,
        arg: ReplayDispatchToolArgPattern,
    },
    Approve {
        label: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReplayDispatchToolArgPattern {
    Wildcard,
    StringLit(String),
    Capture,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReplayDispatchMatch {
    pub arm_index: usize,
    pub whole_value: serde_json::Value,
    pub tool_arg_value: Option<serde_json::Value>,
}

pub fn find_first_replay_match(
    path: impl AsRef<Path>,
    replay_writer: &'static str,
    arms: &[ReplayDispatchArm],
) -> Result<Option<ReplayDispatchMatch>, RuntimeError> {
    let path = path.as_ref();
    let events = read_events_from_path(path).map_err(|err| RuntimeError::ReplayTraceLoad {
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;
    if events.is_empty() {
        return Err(RuntimeError::ReplayTraceLoad {
            path: path.to_path_buf(),
            message: "trace is empty".into(),
        });
    }
    validate_supported_schema(&events).map_err(|err| RuntimeError::ReplayTraceLoad {
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;
    match events.first() {
        Some(TraceEvent::SchemaHeader { writer, .. }) if writer == replay_writer => {}
        Some(TraceEvent::SchemaHeader { writer, .. }) => {
            return Err(RuntimeError::CrossTierReplayUnsupported {
                recorded_writer: writer.clone(),
                replay_writer: replay_writer.to_string(),
            });
        }
        Some(_) => {
            return Err(RuntimeError::ReplayTraceLoad {
                path: path.to_path_buf(),
                message: "trace missing schema_header".into(),
            });
        }
        None => return Ok(None),
    }

    let mut i = 0usize;
    while i < events.len() {
        match &events[i] {
            TraceEvent::ToolCall { tool, args, .. } => {
                let Some(TraceEvent::ToolResult {
                    tool: result_tool,
                    result,
                    ..
                }) = events.get(i + 1)
                else {
                    return malformed_pair(path, "tool_call", i + 1);
                };
                if result_tool != tool {
                    return malformed_pair(path, "tool_call", i + 1);
                }
                if let Some(found) = match_tool_call(tool, args, result, arms) {
                    return Ok(Some(found));
                }
                i += 2;
            }
            TraceEvent::LlmCall { prompt, .. } => {
                let Some(TraceEvent::LlmResult {
                    prompt: result_prompt,
                    result,
                    ..
                }) = events.get(i + 1)
                else {
                    return malformed_pair(path, "llm_call", i + 1);
                };
                if result_prompt != prompt {
                    return malformed_pair(path, "llm_call", i + 1);
                }
                if let Some(found) = match_llm_call(prompt, result, arms) {
                    return Ok(Some(found));
                }
                i += 2;
            }
            TraceEvent::ApprovalRequest { label, .. } => {
                let (response_index, response_label, approved) = match events.get(i + 1) {
                    Some(TraceEvent::ApprovalDecision { .. }) => {
                        let Some(TraceEvent::ApprovalResponse {
                            label: response_label,
                            approved,
                            ..
                        }) = events.get(i + 2)
                        else {
                            return malformed_pair(path, "approval_request", i + 2);
                        };
                        (i + 2, response_label, *approved)
                    }
                    Some(TraceEvent::ApprovalResponse {
                        label: response_label,
                        approved,
                        ..
                    }) => (i + 1, response_label, *approved),
                    _ => return malformed_pair(path, "approval_request", i + 1),
                };
                if response_label != label {
                    return malformed_pair(path, "approval_request", response_index);
                }
                if let Some(found) = match_approval(label, approved, arms) {
                    return Ok(Some(found));
                }
                i = response_index + 1;
            }
            _ => i += 1,
        }
    }

    Ok(None)
}

fn malformed_pair<T>(
    path: &Path,
    event_kind: &'static str,
    expected_step_1based: usize,
) -> Result<T, RuntimeError> {
    Err(RuntimeError::ReplayTraceLoad {
        path: path.to_path_buf(),
        message: format!(
            "malformed replay trace: `{event_kind}` at event {} is not followed by its recorded result",
            expected_step_1based
        ),
    })
}

fn match_llm_call(
    prompt: &str,
    result: &serde_json::Value,
    arms: &[ReplayDispatchArm],
) -> Option<ReplayDispatchMatch> {
    for (arm_index, arm) in arms.iter().enumerate() {
        if matches!(
            &arm.pattern,
            ReplayDispatchPattern::Llm { prompt: expected } if expected == prompt
        ) {
            return Some(ReplayDispatchMatch {
                arm_index,
                whole_value: result.clone(),
                tool_arg_value: None,
            });
        }
    }
    None
}

fn match_tool_call(
    tool: &str,
    args: &[serde_json::Value],
    result: &serde_json::Value,
    arms: &[ReplayDispatchArm],
) -> Option<ReplayDispatchMatch> {
    for (arm_index, arm) in arms.iter().enumerate() {
        let ReplayDispatchPattern::Tool {
            tool: expected_tool,
            arg,
        } = &arm.pattern
        else {
            continue;
        };
        if expected_tool != tool {
            continue;
        }
        match arg {
            ReplayDispatchToolArgPattern::Wildcard => {
                return Some(ReplayDispatchMatch {
                    arm_index,
                    whole_value: result.clone(),
                    tool_arg_value: None,
                });
            }
            ReplayDispatchToolArgPattern::StringLit(expected) => {
                if args.first().and_then(|value| value.as_str()) == Some(expected.as_str()) {
                    return Some(ReplayDispatchMatch {
                        arm_index,
                        whole_value: result.clone(),
                        tool_arg_value: None,
                    });
                }
            }
            ReplayDispatchToolArgPattern::Capture => {
                if let Some(first_arg) = args.first() {
                    return Some(ReplayDispatchMatch {
                        arm_index,
                        whole_value: result.clone(),
                        tool_arg_value: Some(first_arg.clone()),
                    });
                }
            }
        }
    }
    None
}

fn match_approval(
    label: &str,
    approved: bool,
    arms: &[ReplayDispatchArm],
) -> Option<ReplayDispatchMatch> {
    for (arm_index, arm) in arms.iter().enumerate() {
        if matches!(
            &arm.pattern,
            ReplayDispatchPattern::Approve { label: expected } if expected == label
        ) {
            return Some(ReplayDispatchMatch {
                arm_index,
                whole_value: serde_json::Value::Bool(approved),
                tool_arg_value: None,
            });
        }
    }
    None
}
