//! Fresh-record-from-trace-metadata: run the current source against
//! the agent + args recorded in a trace, writing a *new* trace into
//! a caller-supplied directory.
//!
//! Used by the prod-as-test-suite `--promote` path. When the
//! regression harness decides an old trace should be replaced by the
//! current behavior, it asks the CLI runner for a `RecordCurrent`
//! dispatch; this helper provides that dispatch. The live run reuses
//! the real LLM / tool / approver stack (env-driven, not mock), so
//! the promoted trace is an honest recording of the current code.
//!
//! Why this is in `corvid-driver`, not `corvid-runtime`:
//!
//! - Agent resolution needs the compiled IR to match the trace's
//!   `RunStarted.agent` against a live `IrAgent`, which only the
//!   driver has (the runtime operates on already-constructed IR).
//! - JSON-arg-to-typed-`Value` conversion reuses the helper built
//!   for [`super::replay::run_replay_from_source_with_builder_async`]
//!   so arity mismatch and type-coercion failures surface with the
//!   same quality of error as the replay path.
//!
//! Shape mirrors `run_replay_from_source_with_builder_async`: a sync
//! wrapper delegates to the async variant so callers inside an
//! existing tokio context (the harness runner closure in
//! `corvid test --from-traces`) can skip the nested-`block_on`
//! problem the replay helper solved.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use corvid_runtime::{load_dotenv_walking, RuntimeBuilder};
use corvid_trace_schema::{read_events_from_path, validate_supported_schema, TraceEvent};

use super::{compile_to_ir_with_config, load_corvid_config_for, run_ir_with_runtime};

/// Run the current source under `source_path` against the agent +
/// args recorded in `trace_path`, emitting the fresh trace into
/// `emit_dir`. Returns the path of the newly-written `.jsonl`.
///
/// The caller-supplied `base_builder` is expected to already carry
/// the real LLM adapters, approver, and any tool registrations the
/// host wants honored during the fresh run. This helper only layers
/// `.trace_to(emit_dir)` on top; the runtime flushes the trace when
/// dropped.
///
/// Errors: trace load / schema check failure, missing
/// `RunStarted` event, source compile failure, agent-name-not-found
/// in compiled source, arg arity or type mismatch, runtime execution
/// failure, or no fresh `.jsonl` under `emit_dir` after the run
/// (which would indicate the runtime failed to flush — treat as a
/// bug, not a divergence).
pub async fn run_fresh_from_source_async(
    trace_path: &Path,
    source_path: &Path,
    emit_dir: &Path,
    base_builder: RuntimeBuilder,
) -> Result<PathBuf> {
    // Env discovery for API keys — same walk-upward behavior as
    // `corvid run` and the replay helper.
    if let Some(parent) = source_path.parent() {
        let _ = load_dotenv_walking(parent);
    }
    let _ = load_dotenv_walking(
        &std::env::current_dir().unwrap_or_else(|_| Path::new(".").into()),
    );

    // Load + schema-validate the source trace.
    let events = read_events_from_path(trace_path)
        .with_context(|| format!("failed to load trace at `{}`", trace_path.display()))?;
    if events.is_empty() {
        anyhow::bail!("trace `{}` is empty", trace_path.display());
    }
    validate_supported_schema(&events).with_context(|| {
        format!("trace `{}` uses an unsupported schema", trace_path.display())
    })?;

    // Compile source → IR (fresh each time — the whole point of
    // promote is that the source has changed).
    let source = std::fs::read_to_string(source_path).with_context(|| {
        format!("failed to read source at `{}`", source_path.display())
    })?;
    let config = load_corvid_config_for(source_path);
    let ir = compile_to_ir_with_config(&source, config.as_ref()).map_err(|diags| {
        anyhow!(
            "source `{}` failed to compile: {} diagnostic(s)",
            source_path.display(),
            diags.len()
        )
    })?;

    // Extract recorded agent + args from RunStarted.
    let (agent_name, json_args) = find_run_started(&events).ok_or_else(|| {
        anyhow!(
            "trace `{}` has no `RunStarted` event; cannot determine which agent to run fresh",
            trace_path.display()
        )
    })?;
    let agent = ir
        .agents
        .iter()
        .find(|a| a.name == agent_name)
        .ok_or_else(|| {
            anyhow!(
                "trace's recorded agent `{agent_name}` is not present in compiled source `{}` — \
                 cannot promote a trace whose entrypoint no longer exists",
                source_path.display()
            )
        })?;
    let args = super::replay::convert_json_args_for_promote(agent, &ir, &json_args)?;

    // Ensure emit_dir exists before the runtime tries to write into
    // it — the harness allocates a fresh path but doesn't mkdir.
    std::fs::create_dir_all(emit_dir).with_context(|| {
        format!("failed to create promote emit directory `{}`", emit_dir.display())
    })?;

    // Layer trace_to onto the caller's builder and build.
    let runtime = base_builder.trace_to(emit_dir).build();

    let _run_result = run_ir_with_runtime(&ir, Some(&agent_name), args, &runtime).await;

    // Flush the tracer — Runtime's Drop writes the trailing
    // RunCompleted + closes the file. Explicit drop so we can scan
    // emit_dir immediately afterward.
    drop(runtime);

    find_emitted_trace(emit_dir).ok_or_else(|| {
        anyhow!(
            "fresh run under `{}` produced no `.jsonl` — the runtime may have failed to flush; \
             this is a bug rather than a promotable divergence",
            emit_dir.display()
        )
    })
}

fn find_run_started(events: &[TraceEvent]) -> Option<(String, Vec<serde_json::Value>)> {
    events.iter().find_map(|e| match e {
        TraceEvent::RunStarted { agent, args, .. } => Some((agent.clone(), args.clone())),
        _ => None,
    })
}

fn find_emitted_trace(dir: &Path) -> Option<PathBuf> {
    let mut found: Option<PathBuf> = None;
    for entry in std::fs::read_dir(dir).ok()? {
        let path = entry.ok()?.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            // Deterministic pick: lexicographically-first path among
            // any siblings. The harness allocates a fresh `emit_dir`
            // per request so the usual case is exactly one file.
            match &found {
                None => found = Some(path),
                Some(current) if &path < current => found = Some(path),
                _ => {}
            }
        }
    }
    found
}
