mod cursor;
mod differential;
mod diverge;
mod event_classify;
mod mutation;
mod mutation_validate;
mod result_factory;
mod substitute;

use event_classify::{display_step, event_to_json};
use mutation_validate::{
    mutated_approval_result, mutated_json_result, mutated_llm_result, validate_mutation,
};
use result_factory::{
    next_approval_outcome_event, replayed_approval_result, replayed_event_json,
    replayed_json_result,
};

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
pub use mutation::{MutationDivergence, ReplayMutationReport};
use substitute::is_initial_metadata;

#[derive(Debug, Clone)]
enum ReplayMode {
    Substitute,
    Differential {
        model: String,
        report: Arc<Mutex<ReplayDifferentialReport>>,
    },
    Mutation(ReplayMutation),
}

#[derive(Debug, Clone)]
pub(super) struct ReplayMutation {
    pub(super) step_1based: usize,
    pub(super) replacement: serde_json::Value,
    report: Arc<Mutex<ReplayMutationReport>>,
    state: Arc<Mutex<ReplayMutationState>>,
}

#[derive(Debug, Clone, Default)]
struct ReplayMutationState {
    next_step: usize,
}

#[derive(Debug, Clone)]
pub struct ReplaySource {
    path: PathBuf,
    events: Arc<Vec<TraceEvent>>,
    cursor: Arc<Mutex<TraceCursor>>,
    initial_rollout_seed: u64,
    mode: ReplayMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayApprovalDecision {
    pub accepted: bool,
    pub decider: String,
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayApprovalOutcome {
    pub approved: bool,
    pub decision: Option<ReplayApprovalDecision>,
}

impl ReplaySource {
    pub fn from_path(path: impl Into<PathBuf>) -> Result<Self, RuntimeError> {
        Self::from_path_for_writer(path, WRITER_INTERPRETER)
    }

    pub fn from_path_for_writer(
        path: impl Into<PathBuf>,
        replay_writer: &'static str,
    ) -> Result<Self, RuntimeError> {
        Self::load(path.into(), replay_writer, ReplayMode::Substitute)
    }

    pub fn from_path_for_writer_with_model(
        path: impl Into<PathBuf>,
        replay_writer: &'static str,
        model: impl Into<String>,
    ) -> Result<Self, RuntimeError> {
        Self::load(
            path.into(),
            replay_writer,
            ReplayMode::Differential {
                model: model.into(),
                report: Arc::new(Mutex::new(ReplayDifferentialReport::default())),
            },
        )
    }

    pub fn from_path_for_writer_with_mutation(
        path: impl Into<PathBuf>,
        replay_writer: &'static str,
        step_1based: usize,
        replacement: serde_json::Value,
    ) -> Result<Self, RuntimeError> {
        Self::load(
            path.into(),
            replay_writer,
            ReplayMode::Mutation(ReplayMutation {
                step_1based,
                replacement,
                report: Arc::new(Mutex::new(ReplayMutationReport::default())),
                state: Arc::new(Mutex::new(ReplayMutationState::default())),
            }),
        )
    }

    fn load(
        path: PathBuf,
        replay_writer: &'static str,
        mode: ReplayMode,
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

        if let ReplayMode::Mutation(mutation) = &mode {
            validate_mutation(&path, &events, mutation.step_1based, &mutation.replacement)?;
        }

        Ok(Self {
            path,
            events: Arc::new(events),
            cursor: Arc::new(Mutex::new(TraceCursor::new(start_index))),
            initial_rollout_seed,
            mode,
        })
    }

    pub fn initial_rollout_seed(&self) -> u64 {
        self.initial_rollout_seed
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn live_model_override(&self) -> Option<&str> {
        match &self.mode {
            ReplayMode::Substitute => None,
            ReplayMode::Differential { model, .. } => Some(model.as_str()),
            ReplayMode::Mutation(_) => None,
        }
    }

    pub fn uses_live_llm(&self) -> bool {
        matches!(self.mode, ReplayMode::Differential { .. })
    }

    pub fn differential_report(&self) -> Option<ReplayDifferentialReport> {
        match &self.mode {
            ReplayMode::Substitute | ReplayMode::Mutation(_) => None,
            ReplayMode::Differential { report, .. } => Some(report.lock().unwrap().clone()),
        }
    }

    pub fn mutation_report(&self) -> Option<ReplayMutationReport> {
        match &self.mode {
            ReplayMode::Mutation(mutation) => Some(mutation.report.lock().unwrap().clone()),
            ReplayMode::Substitute | ReplayMode::Differential { .. } => None,
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
        match &self.mode {
            ReplayMode::Substitute => {
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
            ReplayMode::Differential { .. } => {
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
            ReplayMode::Mutation(_) => {
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
                            self.record_mutation_run_completion_divergence(
                                RunCompletionDivergence {
                                    step,
                                    recorded_ok,
                                    recorded_result,
                                    recorded_error,
                                    live_ok: ok,
                                    live_result: result.cloned(),
                                    live_error: error.map(str::to_string),
                                },
                            );
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
        match &self.mode {
            ReplayMode::Substitute => {
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
            ReplayMode::Differential { .. } => {
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
            ReplayMode::Mutation(mutation) => {
                let step = mutation.next_step();
                let recorded_call = if step < mutation.step_1based {
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
                            format!(
                                "tool={tool} args={}",
                                serde_json::Value::Array(args.to_vec())
                            ),
                        )
                        .map_err(RuntimeError::ReplayDivergence)?
                } else {
                    let recorded = cursor.next_event(&self.events);
                    if !matches!(
                        &recorded,
                        TraceEvent::ToolCall {
                            tool: expected_tool,
                            args: expected_args,
                            ..
                        } if expected_tool == tool && expected_args == args
                    ) {
                        self.record_mutation_divergence(MutationDivergence {
                            step,
                            kind: "tool_call".into(),
                            recorded: event_to_json(&recorded),
                            got: serde_json::json!({
                                "tool": tool,
                                "args": args,
                            }),
                        });
                    }
                    recorded
                };
                let recorded_result = cursor.next_event(&self.events);
                if step == mutation.step_1based {
                    return mutated_json_result(mutation, &recorded_call, recorded_result);
                }
                return replayed_json_result(tool, recorded_result);
            }
        }
        replayed_json_result(tool, cursor.next_event(&self.events))
    }

    pub async fn replay_llm_call(
        &self,
        prompt: &str,
        recorded_model: Option<&str>,
        recorded_model_version: Option<&str>,
        rendered: &str,
        args: &[serde_json::Value],
        live_req: LlmRequestRef<'_>,
        llms: &LlmRegistry,
    ) -> Result<LlmResponse, RuntimeError> {
        let (result_step, recorded_result) = {
            let mut cursor = self.cursor.lock().unwrap();
            match &self.mode {
                ReplayMode::Substitute | ReplayMode::Differential { .. } => {
                    cursor
                        .expect_next(
                            &self.events,
                            |event| matches!(
                                event,
                                TraceEvent::LlmCall {
                                    prompt: expected_prompt,
                                    model: expected_model,
                                    model_version: expected_model_version,
                                    rendered: expected_rendered,
                                    args: expected_args,
                                    ..
                                } if expected_prompt == prompt
                                    && expected_model.as_deref() == recorded_model
                                    && expected_model_version.as_deref() == recorded_model_version
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
                            model_version: expected_model_version,
                            result,
                            ..
                        } if expected_prompt == prompt
                            && expected_model.as_deref() == recorded_model
                            && expected_model_version.as_deref() == recorded_model_version =>
                        {
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
                }
                ReplayMode::Mutation(mutation) => {
                    let step = mutation.next_step();
                    let recorded_call = if step < mutation.step_1based {
                        cursor
                            .expect_next(
                                &self.events,
                                |event| matches!(
                                    event,
                                    TraceEvent::LlmCall {
                                        prompt: expected_prompt,
                                        model: expected_model,
                                        model_version: expected_model_version,
                                        rendered: expected_rendered,
                                        args: expected_args,
                                        ..
                                    } if expected_prompt == prompt
                                        && expected_model.as_deref() == recorded_model
                                        && expected_model_version.as_deref() == recorded_model_version
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
                            .map_err(RuntimeError::ReplayDivergence)?
                    } else {
                        let recorded = cursor.next_event(&self.events);
                        if !matches!(
                            &recorded,
                            TraceEvent::LlmCall {
                                prompt: expected_prompt,
                                model: expected_model,
                                model_version: expected_model_version,
                                rendered: expected_rendered,
                                args: expected_args,
                                ..
                            } if expected_prompt == prompt
                                && expected_model.as_deref() == recorded_model
                                && expected_model_version.as_deref() == recorded_model_version
                                && expected_rendered.as_deref() == Some(rendered)
                                && expected_args == args
                        ) {
                            self.record_mutation_divergence(MutationDivergence {
                                step,
                                kind: "llm_call".into(),
                                recorded: event_to_json(&recorded),
                                got: serde_json::json!({
                                    "prompt": prompt,
                                    "model": recorded_model,
                                    "model_version": recorded_model_version,
                                    "rendered": rendered,
                                    "args": args,
                                }),
                            });
                        }
                        recorded
                    };
                    let recorded_result = cursor.next_event(&self.events);
                    if step == mutation.step_1based {
                        return mutated_llm_result(mutation, &recorded_call, recorded_result);
                    }
                    (step, replayed_event_json(&recorded_result))
                }
            }
        };

        match &self.mode {
            ReplayMode::Substitute => Ok(LlmResponse::new(
                recorded_result,
                TokenUsage::default(),
            )),
            ReplayMode::Differential { .. } => {
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
            ReplayMode::Mutation(_) => Ok(LlmResponse::new(
                recorded_result,
                TokenUsage::default(),
            )),
        }
    }

    pub fn replay_approval(
        &self,
        label: &str,
        args: &[serde_json::Value],
    ) -> Result<ReplayApprovalOutcome, RuntimeError> {
        let mut cursor = self.cursor.lock().unwrap();
        match &self.mode {
            ReplayMode::Substitute => {
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
            ReplayMode::Differential { .. } => {
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
            ReplayMode::Mutation(mutation) => {
                let step = mutation.next_step();
                let recorded_call = if step < mutation.step_1based {
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
                            format!(
                                "label={label} args={}",
                                serde_json::Value::Array(args.to_vec())
                            ),
                        )
                        .map_err(RuntimeError::ReplayDivergence)?
                } else {
                    let recorded = cursor.next_event(&self.events);
                    if !matches!(
                        &recorded,
                        TraceEvent::ApprovalRequest {
                            label: expected_label,
                            args: expected_args,
                            ..
                        } if expected_label == label && expected_args == args
                    ) {
                        self.record_mutation_divergence(MutationDivergence {
                            step,
                            kind: "approval_request".into(),
                            recorded: event_to_json(&recorded),
                            got: serde_json::json!({
                                "label": label,
                                "args": args,
                            }),
                        });
                    }
                    recorded
                };
                let recorded_result = next_approval_outcome_event(&mut cursor, &self.events);
                if step == mutation.step_1based {
                    return mutated_approval_result(mutation, &recorded_call, recorded_result);
                }
                return replayed_approval_result(label, recorded_result);
            }
        }
        replayed_approval_result(label, next_approval_outcome_event(&mut cursor, &self.events))
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
        if let ReplayMode::Differential { report, .. } = &self.mode {
            report.lock().unwrap().llm_divergences.push(divergence);
        }
    }

    fn record_substitution_divergence(&self, divergence: SubstitutionDivergence) {
        if let ReplayMode::Differential { report, .. } = &self.mode {
            report
                .lock()
                .unwrap()
                .substitution_divergences
                .push(divergence);
        }
    }

    fn record_run_completion_divergence(&self, divergence: RunCompletionDivergence) {
        if let ReplayMode::Differential { report, .. } = &self.mode {
            report.lock().unwrap().run_completion_divergence = Some(divergence);
        }
    }

    fn record_mutation_divergence(&self, divergence: MutationDivergence) {
        if let ReplayMode::Mutation(mutation) = &self.mode {
            mutation.report.lock().unwrap().divergences.push(divergence);
        }
    }

    fn record_mutation_run_completion_divergence(&self, divergence: RunCompletionDivergence) {
        if let ReplayMode::Mutation(mutation) = &self.mode {
            mutation.report.lock().unwrap().run_completion_divergence = Some(divergence);
        }
    }
}

impl ReplayMutation {
    fn next_step(&self) -> usize {
        let mut state = self.state.lock().unwrap();
        state.next_step += 1;
        state.next_step
    }
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
