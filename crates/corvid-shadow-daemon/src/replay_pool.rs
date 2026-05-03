use anyhow::{Context, Result};
use async_trait::async_trait;
use corvid_ast::{AgentAttribute, Decl, Effect};
use corvid_driver::{
    build_or_get_cached_native, compile_to_ir_with_config_at_path, load_corvid_config_for,
};
use corvid_ir::IrFile;
use corvid_runtime::{
    AnthropicAdapter, EnvVarMockAdapter, OpenAiAdapter, ProgrammaticApprover, RedactionSet,
    ReplayDifferentialReport, ReplayDivergence, ReplayMutationReport, Runtime, RuntimeError,
    TraceEvent, Tracer, WRITER_INTERPRETER, WRITER_NATIVE,
};
use corvid_syntax::{lex, parse_file};
use corvid_trace_schema::{read_events_from_path, validate_supported_schema};
use corvid_types::Type;
use corvid_vm::{json_to_value, run_agent, value_to_json};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;

mod executors;
mod parse;
mod spec;
pub use executors::{InterpreterShadowExecutor, NativeShadowExecutor};
pub use parse::{approval_label_for_tool, parse_program_source};
pub use spec::*;

#[async_trait]
pub trait ShadowReplayExecutor: Send + Sync {
    async fn execute(
        &self,
        trace_path: &Path,
        mode: ShadowExecutionMode,
    ) -> Result<ShadowReplayOutcome, ShadowExecutorError>;
}

#[derive(Clone)]
pub struct ReplayPool {
    executor: Arc<dyn ShadowReplayExecutor>,
    semaphore: Arc<Semaphore>,
}

impl ReplayPool {
    pub fn new(executor: Arc<dyn ShadowReplayExecutor>, max_concurrent_replays: usize) -> Self {
        Self {
            executor,
            semaphore: Arc::new(Semaphore::new(max_concurrent_replays.max(1))),
        }
    }

    pub async fn execute(
        &self,
        trace_path: &Path,
        mode: ShadowExecutionMode,
    ) -> Result<ShadowReplayOutcome, ShadowExecutorError> {
        let _permit = self.semaphore.acquire().await.expect("semaphore closed");
        self.executor.execute(trace_path, mode).await
    }
}

fn normalize_event_json(event: &TraceEvent) -> serde_json::Value {
    match event {
        TraceEvent::SchemaHeader {
            version,
            writer,
            commit_sha,
            source_path,
            ..
        } => serde_json::json!({
            "kind": "schema_header",
            "version": version,
            "writer": writer,
            "commit_sha": commit_sha,
            "source_path": source_path,
            "ts_ms": 0,
            "run_id": "<normalized>",
        }),
        TraceEvent::RunStarted { agent, args, .. } => serde_json::json!({
            "kind": "run_started",
            "ts_ms": 0,
            "run_id": "<normalized>",
            "agent": agent,
            "args": args,
        }),
        TraceEvent::RunCompleted {
            ok, result, error, ..
        } => serde_json::json!({
            "kind": "run_completed",
            "ts_ms": 0,
            "run_id": "<normalized>",
            "ok": ok,
            "result": result,
            "error": error,
        }),
        other => {
            let mut value = serde_json::to_value(other).unwrap_or_else(|_| {
                serde_json::json!({
                    "kind": "serialization_error",
                    "debug": format!("{other:?}"),
                })
            });
            if let serde_json::Value::Object(object) = &mut value {
                if object.contains_key("ts_ms") {
                    object.insert("ts_ms".into(), serde_json::json!(0));
                }
                if object.contains_key("run_id") {
                    object.insert("run_id".into(), serde_json::json!("<normalized>"));
                }
            }
            value
        }
    }
}
