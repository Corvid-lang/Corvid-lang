mod cursor;
mod differential;
mod diverge;
mod substitute;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::errors::RuntimeError;
use crate::llm::{LlmRegistry, LlmRequestRef, LlmResponse, TokenUsage};
use corvid_trace_schema::{
    read_events_from_path, validate_supported_schema, TraceEvent, WRITER_INTERPRETER,
};
use cursor::TraceCursor;
pub use differential::{
    LlmDivergence, ReplayDifferentialReport, RunCompletionDivergence, SubstitutionDivergence,
};
pub use diverge::ReplayDivergence;
use substitute::is_initial_metadata;

#[derive(Debug, Clone)]
enum ReplayLlmMode {
    Substitute,
    Differential {
        model: String,
        report: Arc<Mutex<ReplayDifferentialReport>>,
    },
}

#[derive(Debug, Clone)]
pub struct ReplaySource {
    path: PathBuf,
    events: Arc<Vec<TraceEvent>>,
    cursor: Arc<Mutex<TraceCursor>>,
    initial_rollout_seed: u64,
    llm_mode: ReplayLlmMode,
}

impl ReplaySource {
    pub fn from_path(path: impl Into<PathBuf>) -> Result<Self, RuntimeError> {
        Self::from_path_for_writer(path, WRITER_INTERPRETER)
    }

    pub fn from_path_for_writer(
        path: impl Into<PathBuf>,
        replay_writer: &'static str,
    ) -> Result<Self, RuntimeError> {
        Self::load(path.into(), replay_writer, ReplayLlmMode::Substitute)
    }

    pub fn from_path_for_writer_with_model(
        path: impl Into<PathBuf>,
        replay_writer: &'static str,
        model: impl Into<String>,
    ) -> Result<Self, RuntimeError> {
        Self::load(
            path.into(),
            replay_writer,
            ReplayLlmMode::Differential {
                model: model.into(),
                report: Arc::new(Mutex::new(ReplayDifferentialReport::default())),
            },
        )
    }

    fn load(
        path: PathBuf,
        replay_writer: &'static str,
        llm_mode: ReplayLlmMode,
    ) -> Result<Self, RuntimeError> {
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
            Some(TraceEvent::SchemaHeader { writer, .. }) if writer == replay_writer => {}
            Some(TraceEvent::SchemaHeader { writer, .. }) => {
                return Err(RuntimeError::CrossTierReplayUnsupported {
                    recorded_writer: writer.clone(),
                    replay_writer: replay_writer.to_string(),
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
            llm_mode,
        })
    }

    pub fn initial_rollout_seed(&self) -> u64 {
        self.initial_rollout_seed
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn live_model_override(&self) -> Option<&str> {
        match &self.llm_mode {
            ReplayLlmMode::Substitute => None,
            ReplayLlmMode::Differential { model, .. } => Some(model.as_str()),
        }
    }

    pub fn uses_live_llm(&self) -> bool {
        matches!(self.llm_mode, ReplayLlmMode::Differential { .. })
    }

    pub fn differential_report(&self) -> Option<ReplayDifferentialReport> {
        match &self.llm_mode {
            ReplayLlmMode::Substitute => None,
            ReplayLlmMode::Differential { report, .. } => Some(report.lock().unwrap().clone()),
        }
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
        match &self.llm_mode {
            ReplayLlmMode::Substitute => {
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
            }
            ReplayLlmMode::Differential { .. } => {
                let step = display_step(cursor.current_step());
                match cursor.next_event(&self.events) {
                    TraceEvent::RunCompleted {
                        ok: recorded_ok,
                        result: recorded_result,
                        error: recorded_error,
                        ..
                    } => {
                        if recorded_ok != ok
                            || recorded_result.as_ref() != result
                            || recorded_error.as_deref() != error
                        {
                            self.record_run_completion_divergence(RunCompletionDivergence {
                                step,
                                recorded_ok,
                                recorded_result,
                                recorded_error,
                                live_ok: ok,
                                live_result: result.cloned(),
                                live_error: error.map(str::to_string),
                            });
                        }
                    }
                    other => {
                        return Err(RuntimeError::ReplayDivergence(ReplayDivergence {
                            step: cursor.current_step() - 1,
                            expected: other,
                            got_kind: "run_completed",
                            got_description: format!(
                                "ok={ok} result={} error={}",
                                result.cloned().unwrap_or(serde_json::Value::Null),
                                error.unwrap_or("<none>")
                            ),
                        }));
                    }
                }
            }
        }
        cursor.finish(&self.events).map_err(RuntimeError::ReplayDivergence)
    }

    pub fn replay_tool_call(
        &self,
        tool: &str,
        args: &[serde_json::Value],
    ) -> Result<serde_json::Value, RuntimeError> {
        let mut cursor = self.cursor.lock().unwrap();
        match &self.llm_mode {
            ReplayLlmMode::Substitute => {
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
            }
            ReplayLlmMode::Differential { .. } => {
                let step = display_step(cursor.current_step());
                match cursor.next_event(&self.events) {
                    TraceEvent::ToolCall {
                        tool: expected_tool,
                        args: expected_args,
                        ..
                    } => {
                        if expected_tool != tool || expected_args != args {
                            self.record_substitution_divergence(SubstitutionDivergence {
                                step,
                                expected: TraceEvent::ToolCall {
                                    ts_ms: 0,
                                    run_id: String::new(),
                                    tool: expected_tool,
                                    args: expected_args,
                                },
                                got_kind: "tool_call".into(),
                                got_description: format!(
                                    "tool={tool} args={}",
                                    serde_json::Value::Array(args.to_vec())
                                ),
                            });
                        }
                    }
                    other => {
                        return Err(RuntimeError::ReplayDivergence(ReplayDivergence {
                            step: cursor.current_step() - 1,
                            expected: other,
                            got_kind: "tool_call",
                            got_description: format!(
                                "tool={tool} args={}",
                                serde_json::Value::Array(args.to_vec())
                            ),
                        }));
                    }
                }
            }
        }
        match cursor.next_event(&self.events) {
            TraceEvent::ToolResult { result, .. } => Ok(result),
            other => Err(RuntimeError::ReplayDivergence(ReplayDivergence {
                step: cursor.current_step() - 1,
                expected: other,
                got_kind: "tool_result",
                got_description: format!("tool={tool}"),
            })),
        }
    }

    pub async fn replay_llm_call(
        &self,
        prompt: &str,
        recorded_model: Option<&str>,
        rendered: &str,
        args: &[serde_json::Value],
        live_req: LlmRequestRef<'_>,
        llms: &LlmRegistry,
    ) -> Result<LlmResponse, RuntimeError> {
        let (result_step, recorded_result) = {
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
                            && expected_model.as_deref() == recorded_model
                            && expected_rendered.as_deref() == Some(rendered)
                            && expected_args == args
                    ),
                    "llm_call",
                    format!(
                        "prompt={prompt} model={} args={}",
                        recorded_model.unwrap_or("<none>"),
                        serde_json::Value::Array(args.to_vec())
                    ),
                )
                .map_err(RuntimeError::ReplayDivergence)?;
            let result_step = display_step(cursor.current_step());
            let recorded_result = match cursor.next_event(&self.events) {
                TraceEvent::LlmResult {
                    prompt: expected_prompt,
                    model: expected_model,
                    result,
                    ..
                } if expected_prompt == prompt && expected_model.as_deref() == recorded_model => {
                    result
                }
                other => {
                    return Err(RuntimeError::ReplayDivergence(ReplayDivergence {
                        step: cursor.current_step() - 1,
                        expected: other,
                        got_kind: "llm_result",
                        got_description: format!(
                            "prompt={prompt} model={}",
                            recorded_model.unwrap_or("<none>")
                        ),
                    }));
                }
            };
            (result_step, recorded_result)
        };

        match &self.llm_mode {
            ReplayLlmMode::Substitute => Ok(LlmResponse {
                value: recorded_result,
                usage: TokenUsage::default(),
            }),
            ReplayLlmMode::Differential { .. } => {
                let live = llms.call(&live_req).await?;
                if live.value != recorded_result {
                    self.record_llm_divergence(LlmDivergence {
                        step: result_step,
                        prompt: prompt.to_string(),
                        recorded: recorded_result,
                        live: live.value.clone(),
                    });
                }
                Ok(live)
            }
        }
    }

    pub fn replay_approval(
        &self,
        label: &str,
        args: &[serde_json::Value],
    ) -> Result<bool, RuntimeError> {
        let mut cursor = self.cursor.lock().unwrap();
        match &self.llm_mode {
            ReplayLlmMode::Substitute => {
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
            }
            ReplayLlmMode::Differential { .. } => {
                let step = display_step(cursor.current_step());
                match cursor.next_event(&self.events) {
                    TraceEvent::ApprovalRequest {
                        label: expected_label,
                        args: expected_args,
                        ..
                    } => {
                        if expected_label != label || expected_args != args {
                            self.record_substitution_divergence(SubstitutionDivergence {
                                step,
                                expected: TraceEvent::ApprovalRequest {
                                    ts_ms: 0,
                                    run_id: String::new(),
                                    label: expected_label,
                                    args: expected_args,
                                },
                                got_kind: "approval_request".into(),
                                got_description: format!(
                                    "label={label} args={}",
                                    serde_json::Value::Array(args.to_vec())
                                ),
                            });
                        }
                    }
                    other => {
                        return Err(RuntimeError::ReplayDivergence(ReplayDivergence {
                            step: cursor.current_step() - 1,
                            expected: other,
                            got_kind: "approval_request",
                            got_description: format!(
                                "label={label} args={}",
                                serde_json::Value::Array(args.to_vec())
                            ),
                        }));
                    }
                }
            }
        }
        match cursor.next_event(&self.events) {
            TraceEvent::ApprovalResponse { approved, .. } => Ok(approved),
            other => Err(RuntimeError::ReplayDivergence(ReplayDivergence {
                step: cursor.current_step() - 1,
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

    fn record_llm_divergence(&self, divergence: LlmDivergence) {
        if let ReplayLlmMode::Differential { report, .. } = &self.llm_mode {
            report.lock().unwrap().llm_divergences.push(divergence);
        }
    }

    fn record_substitution_divergence(&self, divergence: SubstitutionDivergence) {
        if let ReplayLlmMode::Differential { report, .. } = &self.llm_mode {
            report
                .lock()
                .unwrap()
                .substitution_divergences
                .push(divergence);
        }
    }

    fn record_run_completion_divergence(&self, divergence: RunCompletionDivergence) {
        if let ReplayLlmMode::Differential { report, .. } = &self.llm_mode {
            report.lock().unwrap().run_completion_divergence = Some(divergence);
        }
    }
}

fn display_step(index: usize) -> usize {
    index + 1
}

#[cfg(test)]
mod tests {
    use super::{ReplaySource, SubstitutionDivergence};
    use crate::errors::RuntimeError;
    use corvid_trace_schema::{write_events_to_path, TraceEvent, SCHEMA_VERSION, WRITER_NATIVE};

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
                source_path: None,
                ts_ms: 0,
                run_id: "native".into(),
            }],
        )
        .unwrap();
        let err = ReplaySource::from_path(&path).unwrap_err();
        assert!(matches!(err, RuntimeError::CrossTierReplayUnsupported { .. }));
    }

    #[test]
    fn differential_reports_are_exposed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("interp.jsonl");
        write_events_to_path(
            &path,
            &[
                TraceEvent::SchemaHeader {
                    version: SCHEMA_VERSION,
                    writer: "interpreter".into(),
                    commit_sha: None,
                    source_path: None,
                    ts_ms: 0,
                    run_id: "run".into(),
                },
                TraceEvent::SeedRead {
                    ts_ms: 0,
                    run_id: "run".into(),
                    purpose: "rollout_default_seed".into(),
                    value: 1,
                },
                TraceEvent::RunStarted {
                    ts_ms: 0,
                    run_id: "run".into(),
                    agent: "main".into(),
                    args: vec![],
                },
                TraceEvent::RunCompleted {
                    ts_ms: 0,
                    run_id: "run".into(),
                    ok: true,
                    result: None,
                    error: None,
                },
            ],
        )
        .unwrap();
        let replay = ReplaySource::from_path_for_writer_with_model(&path, "interpreter", "mock-2")
            .unwrap();
        replay.record_substitution_divergence(SubstitutionDivergence {
            step: 1,
            expected: TraceEvent::RunStarted {
                ts_ms: 0,
                run_id: "run".into(),
                agent: "main".into(),
                args: vec![],
            },
            got_kind: "run_started".into(),
            got_description: "agent=main args=[]".into(),
        });
        assert_eq!(
            replay
                .differential_report()
                .unwrap()
                .substitution_divergences
                .len(),
            1
        );
    }
}
