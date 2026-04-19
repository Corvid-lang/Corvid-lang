mod cursor;
mod diverge;
mod substitute;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::errors::RuntimeError;
use crate::llm::{LlmResponse, TokenUsage};
use corvid_trace_schema::{
    read_events_from_path, validate_supported_schema, TraceEvent, WRITER_INTERPRETER,
};
use cursor::TraceCursor;
pub use diverge::ReplayDivergence;
use substitute::is_initial_metadata;

#[derive(Debug, Clone)]
pub struct ReplaySource {
    path: PathBuf,
    events: Arc<Vec<TraceEvent>>,
    cursor: Arc<Mutex<TraceCursor>>,
    initial_rollout_seed: u64,
}

impl ReplaySource {
    pub fn from_path(path: impl Into<PathBuf>) -> Result<Self, RuntimeError> {
        let path = path.into();
        let events = read_events_from_path(&path).map_err(|err| RuntimeError::ReplayTraceLoad {
            path: path.clone(),
            message: err.to_string(),
        })?;
        if events.is_empty() {
            return Err(RuntimeError::ReplayTraceLoad {
                path,
                message: "trace is empty".into(),
            });
        }
        validate_supported_schema(&events).map_err(|err| RuntimeError::ReplayTraceLoad {
            path: path.clone(),
            message: err.to_string(),
        })?;
        match events.first() {
            Some(TraceEvent::SchemaHeader { writer, .. }) if writer == WRITER_INTERPRETER => {}
            Some(TraceEvent::SchemaHeader { writer, .. }) => {
                return Err(RuntimeError::CrossTierReplayUnsupported {
                    recorded_writer: writer.clone(),
                    replay_writer: WRITER_INTERPRETER.to_string(),
                });
            }
            _ => {
                return Err(RuntimeError::ReplayTraceLoad {
                    path,
                    message: "trace is missing a schema header".into(),
                });
            }
        }

        let initial_rollout_seed = events
            .iter()
            .find_map(|event| match event {
                TraceEvent::SeedRead { purpose, value, .. } if purpose == "rollout_default_seed" => {
                    Some(*value)
                }
                _ => None,
            })
            .ok_or_else(|| RuntimeError::ReplayTraceLoad {
                path: path.clone(),
                message: "trace is missing rollout_default_seed".into(),
            })?;

        let start_index = events
            .iter()
            .position(|event| !is_initial_metadata(event))
            .ok_or_else(|| RuntimeError::ReplayTraceLoad {
                path: path.clone(),
                message: "trace contains no executable events".into(),
            })?;

        Ok(Self {
            path,
            events: Arc::new(events),
            cursor: Arc::new(Mutex::new(TraceCursor::new(start_index))),
            initial_rollout_seed,
        })
    }

    pub fn initial_rollout_seed(&self) -> u64 {
        self.initial_rollout_seed
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn prepare_run(
        &self,
        agent: &str,
        args: &[serde_json::Value],
    ) -> Result<(), RuntimeError> {
        let mut cursor = self.cursor.lock().unwrap();
        cursor
            .expect_next(
                &self.events,
                |event| matches!(
                    event,
                    TraceEvent::RunStarted {
                        agent: expected_agent,
                        args: expected_args,
                        ..
                    } if expected_agent == agent && expected_args == args
                ),
                "run_started",
                format!("agent={agent} args={}", serde_json::Value::Array(args.to_vec())),
            )
            .map(|_| ())
            .map_err(RuntimeError::ReplayDivergence)
    }

    pub fn complete_run(
        &self,
        ok: bool,
        result: Option<&serde_json::Value>,
        error: Option<&str>,
    ) -> Result<(), RuntimeError> {
        let mut cursor = self.cursor.lock().unwrap();
        cursor
            .expect_next(
                &self.events,
                |event| matches!(
                    event,
                    TraceEvent::RunCompleted {
                        ok: expected_ok,
                        result: expected_result,
                        error: expected_error,
                        ..
                    } if *expected_ok == ok
                        && expected_result.as_ref() == result
                        && expected_error.as_deref() == error
                ),
                "run_completed",
                format!(
                    "ok={ok} result={} error={}",
                    result.cloned().unwrap_or(serde_json::Value::Null),
                    error.unwrap_or("<none>")
                ),
            )
            .map_err(RuntimeError::ReplayDivergence)?;
        cursor.finish(&self.events).map_err(RuntimeError::ReplayDivergence)
    }

    pub fn replay_tool_call(
        &self,
        tool: &str,
        args: &[serde_json::Value],
    ) -> Result<serde_json::Value, RuntimeError> {
        let mut cursor = self.cursor.lock().unwrap();
        cursor
            .expect_next(
                &self.events,
                |event| matches!(
                    event,
                    TraceEvent::ToolCall {
                        tool: expected_tool,
                        args: expected_args,
                        ..
                    } if expected_tool == tool && expected_args == args
                ),
                "tool_call",
                format!("tool={tool} args={}", serde_json::Value::Array(args.to_vec())),
            )
            .map_err(RuntimeError::ReplayDivergence)?;
        match cursor
            .expect_next(
                &self.events,
                |event| matches!(
                    event,
                    TraceEvent::ToolResult {
                        tool: expected_tool,
                        ..
                    } if expected_tool == tool
                ),
                "tool_result",
                format!("tool={tool}"),
            )
            .map_err(RuntimeError::ReplayDivergence)?
        {
            TraceEvent::ToolResult { result, .. } => Ok(result),
            other => Err(RuntimeError::ReplayDivergence(ReplayDivergence {
                step: cursor.current_step(),
                expected: other,
                got_kind: "tool_result",
                got_description: format!("tool={tool}"),
            })),
        }
    }

    pub fn replay_llm_call(
        &self,
        prompt: &str,
        model: Option<&str>,
        rendered: &str,
        args: &[serde_json::Value],
    ) -> Result<LlmResponse, RuntimeError> {
        let mut cursor = self.cursor.lock().unwrap();
        cursor
            .expect_next(
                &self.events,
                |event| matches!(
                    event,
                    TraceEvent::LlmCall {
                        prompt: expected_prompt,
                        model: expected_model,
                        rendered: expected_rendered,
                        args: expected_args,
                        ..
                    } if expected_prompt == prompt
                        && expected_model.as_deref() == model
                        && expected_rendered.as_deref() == Some(rendered)
                        && expected_args == args
                ),
                "llm_call",
                format!(
                    "prompt={prompt} model={} args={}",
                    model.unwrap_or("<none>"),
                    serde_json::Value::Array(args.to_vec())
                ),
            )
            .map_err(RuntimeError::ReplayDivergence)?;
        match cursor
            .expect_next(
                &self.events,
                |event| matches!(
                    event,
                    TraceEvent::LlmResult {
                        prompt: expected_prompt,
                        model: expected_model,
                        ..
                    } if expected_prompt == prompt && expected_model.as_deref() == model
                ),
                "llm_result",
                format!("prompt={prompt} model={}", model.unwrap_or("<none>")),
            )
            .map_err(RuntimeError::ReplayDivergence)?
        {
            TraceEvent::LlmResult { result, .. } => Ok(LlmResponse {
                value: result,
                usage: TokenUsage::default(),
            }),
            other => Err(RuntimeError::ReplayDivergence(ReplayDivergence {
                step: cursor.current_step(),
                expected: other,
                got_kind: "llm_result",
                got_description: format!("prompt={prompt}"),
            })),
        }
    }

    pub fn replay_approval(
        &self,
        label: &str,
        args: &[serde_json::Value],
    ) -> Result<bool, RuntimeError> {
        let mut cursor = self.cursor.lock().unwrap();
        cursor
            .expect_next(
                &self.events,
                |event| matches!(
                    event,
                    TraceEvent::ApprovalRequest {
                        label: expected_label,
                        args: expected_args,
                        ..
                    } if expected_label == label && expected_args == args
                ),
                "approval_request",
                format!("label={label} args={}", serde_json::Value::Array(args.to_vec())),
            )
            .map_err(RuntimeError::ReplayDivergence)?;
        match cursor
            .expect_next(
                &self.events,
                |event| matches!(
                    event,
                    TraceEvent::ApprovalResponse {
                        label: expected_label,
                        ..
                    } if expected_label == label
                ),
                "approval_response",
                format!("label={label}"),
            )
            .map_err(RuntimeError::ReplayDivergence)?
        {
            TraceEvent::ApprovalResponse { approved, .. } => Ok(approved),
            other => Err(RuntimeError::ReplayDivergence(ReplayDivergence {
                step: cursor.current_step(),
                expected: other,
                got_kind: "approval_response",
                got_description: format!("label={label}"),
            })),
        }
    }

    pub fn replay_rollout_sample(&self) -> Result<u64, RuntimeError> {
        let mut cursor = self.cursor.lock().unwrap();
        match cursor
            .expect_next(
                &self.events,
                |event| matches!(
                    event,
                    TraceEvent::SeedRead { purpose, .. } if purpose == "rollout_cohort"
                ),
                "seed_read",
                "purpose=rollout_cohort".into(),
            )
            .map_err(RuntimeError::ReplayDivergence)?
        {
            TraceEvent::SeedRead { value, .. } => Ok(value),
            other => Err(RuntimeError::ReplayDivergence(ReplayDivergence {
                step: cursor.current_step(),
                expected: other,
                got_kind: "seed_read",
                got_description: "purpose=rollout_cohort".into(),
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ReplaySource;
    use crate::errors::RuntimeError;
    use corvid_trace_schema::{
        write_events_to_path, TraceEvent, SCHEMA_VERSION, WRITER_NATIVE,
    };

    #[test]
    fn rejects_cross_tier_native_trace() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("native.jsonl");
        write_events_to_path(
            &path,
            &[TraceEvent::SchemaHeader {
                version: SCHEMA_VERSION,
                writer: WRITER_NATIVE.into(),
                commit_sha: None,
                ts_ms: 0,
                run_id: "native".into(),
            }],
        )
        .unwrap();
        let err = ReplaySource::from_path(&path).unwrap_err();
        assert!(matches!(err, RuntimeError::CrossTierReplayUnsupported { .. }));
    }
}
