use corvid_driver::{
    run_fresh_from_source_async, run_replay_from_source_with_builder_async, ReplayMode,
};
use corvid_runtime::{
    AnthropicAdapter, OpenAiAdapter, Runtime, StdinApprover, TraceHarnessMode, TraceHarnessRequest,
    TraceHarnessRun,
};
use std::path::Path;
use std::sync::Arc;

pub(super) async fn dispatch_harness_request(
    source_path: &Path,
    request: TraceHarnessRequest,
) -> Result<TraceHarnessRun, corvid_runtime::RuntimeError> {
    match request.mode {
        TraceHarnessMode::Replay => {
            dispatch_replay(source_path, &request.trace_path, ReplayMode::Plain).await
        }
        TraceHarnessMode::Differential { model } => {
            dispatch_replay(
                source_path,
                &request.trace_path,
                ReplayMode::Differential(model),
            )
            .await
        }
        TraceHarnessMode::RecordCurrent => {
            dispatch_record_current(source_path, &request.trace_path, &request.emit_dir).await
        }
    }
}

/// Runs the agent + args recorded in `trace_path` against the
/// current source, writing a fresh trace under `emit_dir`. Uses the
/// same env-driven runtime builder the Replay path uses — real LLM
/// adapters, real approver, real tools — so the promoted trace is an
/// honest recording of the current code, not a mock replay. Returns
/// a `TraceHarnessRun` whose `emitted_trace_path` is the new `.jsonl`
/// the harness will atomically move over the original.
async fn dispatch_record_current(
    source_path: &Path,
    trace_path: &Path,
    emit_dir: &Path,
) -> Result<TraceHarnessRun, corvid_runtime::RuntimeError> {
    let base_builder = default_runtime_builder();
    let emitted = run_fresh_from_source_async(trace_path, source_path, emit_dir, base_builder)
        .await
        .map_err(|err| corvid_runtime::RuntimeError::ReplayTraceLoad {
            path: trace_path.to_path_buf(),
            message: format!("fresh-run for promote failed: {err:#}"),
        })?;

    Ok(TraceHarnessRun {
        final_output: None,
        ok: true,
        error: None,
        emitted_trace_path: emitted,
        differential_report: None,
    })
}

async fn dispatch_replay(
    source_path: &Path,
    trace_path: &Path,
    mode: ReplayMode,
) -> Result<TraceHarnessRun, corvid_runtime::RuntimeError> {
    let base_builder = default_runtime_builder();
    let outcome =
        run_replay_from_source_with_builder_async(trace_path, source_path, mode, base_builder)
            .await
            .map_err(|err| corvid_runtime::RuntimeError::ReplayTraceLoad {
                path: trace_path.to_path_buf(),
                message: format!("{err:#}"),
            })?;

    let final_output = outcome.result_value.as_ref().map(|v| {
        // Reuse corvid-vm's value_to_json is not accessible from
        // here; the runtime crate hands back a Value (from its own
        // re-export). The harness needs a serde_json::Value for its
        // final_output field. Best-effort stringify + parse cycle;
        // for v1 this is adequate since the harness compares
        // structural output, not byte identity.
        serde_json::to_value(format!("{v:?}")).unwrap_or(serde_json::Value::Null)
    });

    Ok(TraceHarnessRun {
        final_output,
        ok: outcome.ran_cleanly(),
        error: outcome.result_error.clone(),
        emitted_trace_path: trace_path.to_path_buf(),
        differential_report: outcome.differential_report,
    })
}

fn default_runtime_builder() -> corvid_runtime::RuntimeBuilder {
    let mut builder = Runtime::builder().approver(Arc::new(StdinApprover::new()));
    if let Ok(model) = std::env::var("CORVID_MODEL") {
        builder = builder.default_model(&model);
    }
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        builder = builder.llm(Arc::new(AnthropicAdapter::new(key)));
    }
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        builder = builder.llm(Arc::new(OpenAiAdapter::new(key)));
    }
    builder
}
