//! Typed replay loader over runtime JSONL traces.

use corvid_runtime::TraceEvent;
use serde_json::Value;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ReplaySession {
    pub path: PathBuf,
    pub run_id: String,
    pub steps: Vec<ReplayStep>,
    pub duration_ms: u64,
    pub final_status: ReplayFinalStatus,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub enum ReplayStep {
    RunStart {
        ts_ms: u64,
        agent: String,
        args: Vec<Value>,
    },
    Tool {
        start_ts_ms: u64,
        end_ts_ms: Option<u64>,
        tool: String,
        args: Vec<Value>,
        result: Option<Value>,
    },
    Llm {
        start_ts_ms: u64,
        end_ts_ms: Option<u64>,
        prompt: String,
        model: Option<String>,
        rendered: Option<String>,
        args: Vec<Value>,
        result: Option<Value>,
    },
    Approval {
        start_ts_ms: u64,
        end_ts_ms: Option<u64>,
        label: String,
        args: Vec<Value>,
        approved: Option<bool>,
    },
    RunComplete {
        ts_ms: u64,
        ok: bool,
        result: Option<Value>,
        error: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayFinalStatus {
    Ok,
    Error,
    Truncated,
}

#[derive(Debug)]
pub enum ReplayLoadError {
    Read {
        path: PathBuf,
        error: std::io::Error,
    },
    Empty {
        path: PathBuf,
    },
    InvalidLine {
        path: PathBuf,
        line: usize,
        message: String,
    },
    InvalidShape {
        path: PathBuf,
        message: String,
    },
}

impl ReplaySession {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ReplayLoadError> {
        let path = path.as_ref().to_path_buf();
        let body = std::fs::read_to_string(&path).map_err(|error| ReplayLoadError::Read {
            path: path.clone(),
            error,
        })?;
        if body.trim().is_empty() {
            return Err(ReplayLoadError::Empty { path });
        }

        let mut events = Vec::new();
        for (index, line) in body.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let event: TraceEvent = serde_json::from_str(line).map_err(|error| {
                ReplayLoadError::InvalidLine {
                    path: path.clone(),
                    line: index + 1,
                    message: error.to_string(),
                }
            })?;
            events.push(event);
        }
        if events.is_empty() {
            return Err(ReplayLoadError::Empty { path });
        }

        let run_id = first_run_id(&events)?.to_string();
        let start_ts = first_ts(&events);
        let end_ts = last_ts(&events);
        let duration_ms = end_ts.saturating_sub(start_ts);

        let mut steps = Vec::new();
        let mut truncated = false;
        let mut final_status = ReplayFinalStatus::Truncated;
        let mut saw_completion = false;

        let mut i = 0;
        while i < events.len() {
            match &events[i] {
                TraceEvent::RunStarted {
                    ts_ms,
                    run_id: event_run_id,
                    agent,
                    args,
                } => {
                    ensure_run_id(&path, &run_id, event_run_id)?;
                    steps.push(ReplayStep::RunStart {
                        ts_ms: *ts_ms,
                        agent: agent.clone(),
                        args: args.clone(),
                    });
                    i += 1;
                }
                TraceEvent::ToolCall {
                    ts_ms,
                    run_id: event_run_id,
                    tool,
                    args,
                } => {
                    ensure_run_id(&path, &run_id, event_run_id)?;
                    let session_run_id = run_id.as_str();
                    let (end_ts_ms, result, consumed, was_truncated) =
                        match events.get(i + 1) {
                            Some(TraceEvent::ToolResult {
                                ts_ms,
                                run_id,
                                tool: result_tool,
                                result,
                            }) if run_id == session_run_id && result_tool == tool => {
                                (Some(*ts_ms), Some(result.clone()), 2, false)
                            }
                            _ => (None, None, 1, true),
                        };
                    if was_truncated {
                        truncated = true;
                    }
                    steps.push(ReplayStep::Tool {
                        start_ts_ms: *ts_ms,
                        end_ts_ms,
                        tool: tool.clone(),
                        args: args.clone(),
                        result,
                    });
                    i += consumed;
                }
                TraceEvent::LlmCall {
                    ts_ms,
                    run_id: event_run_id,
                    prompt,
                    model,
                    rendered,
                    args,
                } => {
                    ensure_run_id(&path, &run_id, event_run_id)?;
                    let session_run_id = run_id.as_str();
                    let (end_ts_ms, result, consumed, was_truncated) =
                        match events.get(i + 1) {
                            Some(TraceEvent::LlmResult {
                                ts_ms,
                                run_id,
                                prompt: result_prompt,
                                result,
                                ..
                            }) if run_id == session_run_id && result_prompt == prompt => {
                                (Some(*ts_ms), Some(result.clone()), 2, false)
                            }
                            _ => (None, None, 1, true),
                        };
                    if was_truncated {
                        truncated = true;
                    }
                    steps.push(ReplayStep::Llm {
                        start_ts_ms: *ts_ms,
                        end_ts_ms,
                        prompt: prompt.clone(),
                        model: model.clone(),
                        rendered: rendered.clone(),
                        args: args.clone(),
                        result,
                    });
                    i += consumed;
                }
                TraceEvent::ApprovalRequest {
                    ts_ms,
                    run_id: event_run_id,
                    label,
                    args,
                } => {
                    ensure_run_id(&path, &run_id, event_run_id)?;
                    let session_run_id = run_id.as_str();
                    let (end_ts_ms, approved, consumed, was_truncated) =
                        match events.get(i + 1) {
                            Some(TraceEvent::ApprovalResponse {
                                ts_ms,
                                run_id,
                                label: result_label,
                                approved,
                            }) if run_id == session_run_id && result_label == label => {
                                (Some(*ts_ms), Some(*approved), 2, false)
                            }
                            _ => (None, None, 1, true),
                        };
                    if was_truncated {
                        truncated = true;
                    }
                    steps.push(ReplayStep::Approval {
                        start_ts_ms: *ts_ms,
                        end_ts_ms,
                        label: label.clone(),
                        args: args.clone(),
                        approved,
                    });
                    i += consumed;
                }
                TraceEvent::RunCompleted {
                    ts_ms,
                    run_id: event_run_id,
                    ok,
                    result,
                    error,
                } => {
                    ensure_run_id(&path, &run_id, event_run_id)?;
                    steps.push(ReplayStep::RunComplete {
                        ts_ms: *ts_ms,
                        ok: *ok,
                        result: result.clone(),
                        error: error.clone(),
                    });
                    final_status = if *ok {
                        ReplayFinalStatus::Ok
                    } else {
                        ReplayFinalStatus::Error
                    };
                    saw_completion = true;
                    i += 1;
                }
                TraceEvent::ModelSelected {
                    run_id: event_run_id,
                    ..
                } => {
                    ensure_run_id(&path, &run_id, event_run_id)?;
                    i += 1;
                }
                TraceEvent::ProgressiveEscalation {
                    run_id: event_run_id,
                    ..
                }
                | TraceEvent::ProgressiveExhausted {
                    run_id: event_run_id,
                    ..
                }
                | TraceEvent::AbVariantChosen {
                    run_id: event_run_id,
                    ..
                }
                | TraceEvent::EnsembleVote {
                    run_id: event_run_id,
                    ..
                }
                | TraceEvent::AdversarialPipelineCompleted {
                    run_id: event_run_id,
                    ..
                }
                | TraceEvent::AdversarialContradiction {
                    run_id: event_run_id,
                    ..
                } => {
                    ensure_run_id(&path, &run_id, event_run_id)?;
                    i += 1;
                }
                TraceEvent::ToolResult { .. }
                | TraceEvent::LlmResult { .. }
                | TraceEvent::ApprovalResponse { .. } => {
                    return Err(ReplayLoadError::InvalidShape {
                        path,
                        message: "trace contains an unpaired result/response event".into(),
                    });
                }
            }
        }

        if !saw_completion {
            truncated = true;
            final_status = ReplayFinalStatus::Truncated;
        }

        Ok(Self {
            path,
            run_id,
            steps,
            duration_ms,
            final_status,
            truncated,
        })
    }

    pub fn summary_line(&self) -> String {
        let truncation = if self.truncated { " (truncated)" } else { "" };
        format!(
            "loaded replay `{}` [run {}]: {} step(s), {} ms, final status: {}{}",
            self.path.display(),
            self.run_id,
            self.steps.len(),
            self.duration_ms,
            self.final_status,
            truncation
        )
    }
}

impl ReplayStep {
    pub fn title(&self) -> String {
        match self {
            ReplayStep::RunStart { agent, .. } => format!("run start: {agent}"),
            ReplayStep::Tool { tool, .. } => format!("tool: {tool}"),
            ReplayStep::Llm { prompt, .. } => format!("llm: {prompt}"),
            ReplayStep::Approval { label, .. } => format!("approval: {label}"),
            ReplayStep::RunComplete { ok, .. } => {
                if *ok {
                    "run complete: ok".into()
                } else {
                    "run complete: error".into()
                }
            }
        }
    }

    pub fn render(&self) -> String {
        match self {
            ReplayStep::RunStart { ts_ms, agent, args } => {
                format!(
                    "run start\n  ts    : {ts_ms}\n  agent : {agent}\n  inputs: {}",
                    render_json_array(args)
                )
            }
            ReplayStep::Tool {
                start_ts_ms,
                end_ts_ms,
                tool,
                args,
                result,
                ..
            } => format!(
                "tool call\n  ts    : {}\n  tool  : {tool}\n  inputs: {}\n  output: {}",
                render_window(*start_ts_ms, *end_ts_ms),
                render_json_array(args),
                render_optional_json(result.as_ref())
            ),
            ReplayStep::Llm {
                start_ts_ms,
                end_ts_ms,
                prompt,
                model,
                rendered,
                args,
                result,
                ..
            } => format!(
                "llm call\n  ts     : {}\n  prompt : {prompt}\n  model  : {}\n  rendered: {}\n  inputs : {}\n  output : {}",
                render_window(*start_ts_ms, *end_ts_ms),
                model.as_deref().unwrap_or("<none>"),
                rendered.as_deref().unwrap_or("<none>"),
                render_json_array(args),
                render_optional_json(result.as_ref())
            ),
            ReplayStep::Approval {
                start_ts_ms,
                end_ts_ms,
                label,
                args,
                approved,
                ..
            } => format!(
                "approval\n  ts    : {}\n  label : {label}\n  inputs: {}\n  output: {}",
                render_window(*start_ts_ms, *end_ts_ms),
                render_json_array(args),
                approved
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "<missing>".into())
            ),
            ReplayStep::RunComplete {
                ts_ms,
                ok,
                result,
                error,
            } => format!(
                "run complete\n  ts    : {ts_ms}\n  ok    : {ok}\n  output: {}\n  error : {}",
                render_optional_json(result.as_ref()),
                error.as_deref().unwrap_or("<none>")
            ),
        }
    }
}

impl fmt::Display for ReplayFinalStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplayFinalStatus::Ok => write!(f, "OK"),
            ReplayFinalStatus::Error => write!(f, "ERROR"),
            ReplayFinalStatus::Truncated => write!(f, "TRUNCATED"),
        }
    }
}

impl fmt::Display for ReplayLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplayLoadError::Read { path, error } => {
                write!(f, "cannot read replay `{}`: {error}", path.display())
            }
            ReplayLoadError::Empty { path } => {
                write!(f, "replay `{}` is empty", path.display())
            }
            ReplayLoadError::InvalidLine {
                path,
                line,
                message,
            } => {
                write!(
                    f,
                    "replay `{}` has invalid JSONL at line {}: {}",
                    path.display(),
                    line,
                    message
                )
            }
            ReplayLoadError::InvalidShape { path, message } => {
                write!(f, "replay `{}` has invalid event shape: {message}", path.display())
            }
        }
    }
}

fn first_run_id(events: &[TraceEvent]) -> Result<&str, ReplayLoadError> {
    match events.first() {
        Some(TraceEvent::RunStarted { run_id, .. })
        | Some(TraceEvent::RunCompleted { run_id, .. })
        | Some(TraceEvent::ToolCall { run_id, .. })
        | Some(TraceEvent::ToolResult { run_id, .. })
        | Some(TraceEvent::LlmCall { run_id, .. })
        | Some(TraceEvent::LlmResult { run_id, .. })
        | Some(TraceEvent::ApprovalRequest { run_id, .. })
        | Some(TraceEvent::ApprovalResponse { run_id, .. })
        | Some(TraceEvent::ModelSelected { run_id, .. })
        | Some(TraceEvent::ProgressiveEscalation { run_id, .. })
        | Some(TraceEvent::ProgressiveExhausted { run_id, .. })
        | Some(TraceEvent::AbVariantChosen { run_id, .. })
        | Some(TraceEvent::EnsembleVote { run_id, .. })
        | Some(TraceEvent::AdversarialPipelineCompleted { run_id, .. })
        | Some(TraceEvent::AdversarialContradiction { run_id, .. }) => Ok(run_id),
        None => unreachable!("empty event list handled earlier"),
    }
}

fn ensure_run_id(path: &Path, expected: &str, got: &str) -> Result<(), ReplayLoadError> {
    if expected == got {
        Ok(())
    } else {
        Err(ReplayLoadError::InvalidShape {
            path: path.to_path_buf(),
            message: format!("mixed run ids in one replay file: `{expected}` vs `{got}`"),
        })
    }
}

fn first_ts(events: &[TraceEvent]) -> u64 {
    match events.first() {
        Some(TraceEvent::RunStarted { ts_ms, .. })
        | Some(TraceEvent::RunCompleted { ts_ms, .. })
        | Some(TraceEvent::ToolCall { ts_ms, .. })
        | Some(TraceEvent::ToolResult { ts_ms, .. })
        | Some(TraceEvent::LlmCall { ts_ms, .. })
        | Some(TraceEvent::LlmResult { ts_ms, .. })
        | Some(TraceEvent::ApprovalRequest { ts_ms, .. })
        | Some(TraceEvent::ApprovalResponse { ts_ms, .. })
        | Some(TraceEvent::ModelSelected { ts_ms, .. })
        | Some(TraceEvent::ProgressiveEscalation { ts_ms, .. })
        | Some(TraceEvent::ProgressiveExhausted { ts_ms, .. })
        | Some(TraceEvent::AbVariantChosen { ts_ms, .. })
        | Some(TraceEvent::EnsembleVote { ts_ms, .. })
        | Some(TraceEvent::AdversarialPipelineCompleted { ts_ms, .. })
        | Some(TraceEvent::AdversarialContradiction { ts_ms, .. }) => *ts_ms,
        None => 0,
    }
}

fn last_ts(events: &[TraceEvent]) -> u64 {
    match events.last() {
        Some(TraceEvent::RunStarted { ts_ms, .. })
        | Some(TraceEvent::RunCompleted { ts_ms, .. })
        | Some(TraceEvent::ToolCall { ts_ms, .. })
        | Some(TraceEvent::ToolResult { ts_ms, .. })
        | Some(TraceEvent::LlmCall { ts_ms, .. })
        | Some(TraceEvent::LlmResult { ts_ms, .. })
        | Some(TraceEvent::ApprovalRequest { ts_ms, .. })
        | Some(TraceEvent::ApprovalResponse { ts_ms, .. })
        | Some(TraceEvent::ModelSelected { ts_ms, .. })
        | Some(TraceEvent::ProgressiveEscalation { ts_ms, .. })
        | Some(TraceEvent::ProgressiveExhausted { ts_ms, .. })
        | Some(TraceEvent::AbVariantChosen { ts_ms, .. })
        | Some(TraceEvent::EnsembleVote { ts_ms, .. })
        | Some(TraceEvent::AdversarialPipelineCompleted { ts_ms, .. })
        | Some(TraceEvent::AdversarialContradiction { ts_ms, .. }) => *ts_ms,
        None => 0,
    }
}

fn render_json_array(values: &[Value]) -> String {
    Value::Array(values.to_vec()).to_string()
}

fn render_optional_json(value: Option<&Value>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "<missing>".into())
}

fn render_window(start_ts_ms: u64, end_ts_ms: Option<u64>) -> String {
    match end_ts_ms {
        Some(end_ts_ms) if end_ts_ms >= start_ts_ms => {
            format!("{start_ts_ms} -> {end_ts_ms} ({} ms)", end_ts_ms - start_ts_ms)
        }
        Some(end_ts_ms) => format!("{start_ts_ms} -> {end_ts_ms}"),
        None => format!("{start_ts_ms} -> <missing>"),
    }
}
