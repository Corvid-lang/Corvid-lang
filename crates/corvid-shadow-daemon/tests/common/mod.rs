use async_trait::async_trait;
use corvid_runtime::TraceEvent;
use corvid_shadow_daemon::{
    AgentInvariantInfo, DangerousToolSpec, DimensionSnapshot, MutationSpec, ProvenanceSnapshot,
    ShadowExecutionMode, ShadowExecutorError, ShadowReplayExecutor, ShadowReplayOutcome, TrustTier,
};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub fn simple_events(run_id: &str, writer: &str, agent: &str) -> Vec<TraceEvent> {
    vec![
        TraceEvent::SchemaHeader {
            version: corvid_trace_schema::SCHEMA_VERSION,
            writer: writer.into(),
            commit_sha: Some("abc123".into()),
            source_path: Some("examples/refund_bot_demo/src/refund_bot.cor".into()),
            ts_ms: 1,
            run_id: run_id.into(),
        },
        TraceEvent::RunStarted {
            ts_ms: 2,
            run_id: run_id.into(),
            agent: agent.into(),
            args: vec![],
        },
        TraceEvent::RunCompleted {
            ts_ms: 3,
            run_id: run_id.into(),
            ok: true,
            result: Some(serde_json::json!("ok")),
            error: None,
        },
    ]
}

pub fn llm_events(run_id: &str, writer: &str, agent: &str, prompt: &str, result: serde_json::Value) -> Vec<TraceEvent> {
    vec![
        TraceEvent::SchemaHeader {
            version: corvid_trace_schema::SCHEMA_VERSION,
            writer: writer.into(),
            commit_sha: Some("abc123".into()),
            source_path: Some("examples/refund_bot_demo/src/refund_bot.cor".into()),
            ts_ms: 1,
            run_id: run_id.into(),
        },
        TraceEvent::RunStarted {
            ts_ms: 2,
            run_id: run_id.into(),
            agent: agent.into(),
            args: vec![],
        },
        TraceEvent::LlmCall {
            ts_ms: 3,
            run_id: run_id.into(),
            prompt: prompt.into(),
            model: Some("mock-1".into()),
            rendered: Some("rendered".into()),
            args: vec![],
        },
        TraceEvent::LlmResult {
            ts_ms: 4,
            run_id: run_id.into(),
            prompt: prompt.into(),
            model: Some("mock-1".into()),
            result: result.clone(),
        },
        TraceEvent::RunCompleted {
            ts_ms: 5,
            run_id: run_id.into(),
            ok: true,
            result: Some(result),
            error: None,
        },
    ]
}

pub fn tool_events(
    run_id: &str,
    writer: &str,
    agent: &str,
    tool: &str,
    arg: serde_json::Value,
    result: serde_json::Value,
) -> Vec<TraceEvent> {
    vec![
        TraceEvent::SchemaHeader {
            version: corvid_trace_schema::SCHEMA_VERSION,
            writer: writer.into(),
            commit_sha: Some("abc123".into()),
            source_path: Some("examples/refund_bot_demo/src/refund_bot.cor".into()),
            ts_ms: 1,
            run_id: run_id.into(),
        },
        TraceEvent::RunStarted {
            ts_ms: 2,
            run_id: run_id.into(),
            agent: agent.into(),
            args: vec![],
        },
        TraceEvent::ToolCall {
            ts_ms: 3,
            run_id: run_id.into(),
            tool: tool.into(),
            args: vec![arg],
        },
        TraceEvent::ToolResult {
            ts_ms: 4,
            run_id: run_id.into(),
            tool: tool.into(),
            result,
        },
        TraceEvent::RunCompleted {
            ts_ms: 5,
            run_id: run_id.into(),
            ok: true,
            result: Some(serde_json::json!("ok")),
            error: None,
        },
    ]
}

pub fn approval_events(run_id: &str, writer: &str, agent: &str, label: &str, approved: bool) -> Vec<TraceEvent> {
    vec![
        TraceEvent::SchemaHeader {
            version: corvid_trace_schema::SCHEMA_VERSION,
            writer: writer.into(),
            commit_sha: Some("abc123".into()),
            source_path: Some("examples/refund_bot_demo/src/refund_bot.cor".into()),
            ts_ms: 1,
            run_id: run_id.into(),
        },
        TraceEvent::RunStarted {
            ts_ms: 2,
            run_id: run_id.into(),
            agent: agent.into(),
            args: vec![],
        },
        TraceEvent::ApprovalRequest {
            ts_ms: 3,
            run_id: run_id.into(),
            label: label.into(),
            args: vec![],
        },
        TraceEvent::ApprovalResponse {
            ts_ms: 4,
            run_id: run_id.into(),
            label: label.into(),
            approved,
        },
        TraceEvent::RunCompleted {
            ts_ms: 5,
            run_id: run_id.into(),
            ok: true,
            result: Some(serde_json::json!(approved)),
            error: None,
        },
    ]
}

pub fn outcome(agent: &str) -> ShadowReplayOutcome {
    let events = simple_events("run-recorded", corvid_runtime::WRITER_INTERPRETER, agent);
    ShadowReplayOutcome {
        trace_path: PathBuf::from(format!("{agent}.jsonl")),
        run_id: "run-recorded".into(),
        agent: agent.into(),
        recorded_events: events.clone(),
        shadow_trace_path: PathBuf::from(format!("{agent}-shadow.jsonl")),
        shadow_events: events,
        recorded_output: Some(serde_json::json!("ok")),
        shadow_output: Some(serde_json::json!("ok")),
        replay_divergence: None,
        differential_report: None,
        mutation_report: None,
        recorded_dimensions: DimensionSnapshot {
            cost: 1.0,
            latency_ms: 100,
            trust_tier: Some(TrustTier::Autonomous),
            budget_declared: Some(10.0),
        },
        shadow_dimensions: DimensionSnapshot {
            cost: 1.0,
            latency_ms: 100,
            trust_tier: Some(TrustTier::Autonomous),
            budget_declared: Some(10.0),
        },
        recorded_provenance: ProvenanceSnapshot {
            nodes: BTreeSet::from(["tool:get_order".into()]),
            root_sources: BTreeSet::from(["retrieval:get_order".into()]),
            has_chain: true,
        },
        shadow_provenance: ProvenanceSnapshot {
            nodes: BTreeSet::from(["tool:get_order".into()]),
            root_sources: BTreeSet::from(["retrieval:get_order".into()]),
            has_chain: true,
        },
        metadata: AgentInvariantInfo {
            agent: agent.into(),
            replayable: true,
            deterministic: true,
            grounded_return: true,
            budget_declared: Some(10.0),
            dangerous_tools: vec![DangerousToolSpec {
                tool: "issue_refund".into(),
                approval_label: "IssueRefund".into(),
            }],
        },
        mode: "replay".into(),
        ok: true,
        error: None,
    }
}

#[derive(Clone, Default)]
pub struct FakeExecutor {
    pub calls: Arc<Mutex<Vec<(PathBuf, String)>>>,
    pub outcomes: Arc<Mutex<HashMap<(PathBuf, String), Result<ShadowReplayOutcome, String>>>>,
}

impl FakeExecutor {
    pub fn set_ok(&self, path: &Path, mode: &str, outcome: ShadowReplayOutcome) {
        self.outcomes
            .lock()
            .unwrap()
            .insert((path.to_path_buf(), mode.into()), Ok(outcome));
    }

    pub fn set_err(&self, path: &Path, mode: &str, message: &str) {
        self.outcomes
            .lock()
            .unwrap()
            .insert((path.to_path_buf(), mode.into()), Err(message.into()));
    }
}

#[async_trait]
impl ShadowReplayExecutor for FakeExecutor {
    async fn execute(
        &self,
        trace_path: &Path,
        mode: ShadowExecutionMode,
    ) -> Result<ShadowReplayOutcome, ShadowExecutorError> {
        let mode_key = match &mode {
            ShadowExecutionMode::Replay => "replay".to_string(),
            ShadowExecutionMode::Differential { model } => format!("differential:{model}"),
            ShadowExecutionMode::Mutation(MutationSpec { step_1based, .. }) => {
                format!("mutation:{step_1based}")
            }
        };
        self.calls
            .lock()
            .unwrap()
            .push((trace_path.to_path_buf(), mode_key.clone()));
        match self
            .outcomes
            .lock()
            .unwrap()
            .get(&(trace_path.to_path_buf(), mode_key))
            .cloned()
            .unwrap_or_else(|| Ok(outcome("refund_bot")))
        {
            Ok(outcome) => Ok(outcome),
            Err(message) => Err(ShadowExecutorError::Runtime(corvid_runtime::RuntimeError::Other(
                message,
            ))),
        }
    }
}
