//! Replay orchestration — from a (trace path, source path, mode)
//! triple to a typed `ReplayOutcome`.
//!
//! Three modes:
//!
//! - [`ReplayMode::Plain`] — byte-identical reproduction. The
//!   `Runtime` substitutes every recorded response verbatim.
//!   Runtime seam from `21-C-replay-interp` (Dev B).
//! - [`ReplayMode::Differential`] — LLM-swap. The `Runtime`
//!   issues live calls against the target model in place of the
//!   recorded `LlmResult` substitution; every other axis
//!   (tool/approval/seed/clock) replays strict. Runtime seam from
//!   `21-inv-B-adapter` (Dev B).
//! - [`ReplayMode::Mutation`] — counterfactual. One recorded
//!   response (at the given 1-based step among substitutable
//!   events) is replaced with a user-supplied JSON value; every
//!   other axis replays strict. Runtime seam from
//!   `21-inv-D-runtime` (Dev B).
//!
//! CLI consumers (`corvid replay --model <id>`, `corvid replay
//! --mutate STEP JSON`, and eventually `corvid replay` plain) all
//! call [`run_replay_from_source`] with the right mode and render
//! the [`ReplayOutcome`].
//!
//! Agent resolution: the trace's `RunStarted` event names the
//! agent and carries its original `args: Vec<serde_json::Value>`.
//! This helper matches the agent name against the compiled IR,
//! converts each recorded JSON arg into a typed `Value` using the
//! agent's declared parameter types, and threads them through
//! [`run_ir_with_runtime`]. That keeps replay round-tripping
//! behavior honest — agents that actually took arguments in prod
//! get those exact arguments back in the shadow run.

use super::{
    compile_to_ir_with_config, load_corvid_config_for, run_ir_with_runtime, RunError,
};
use anyhow::{anyhow, Context, Result};
use corvid_ir::{IrAgent, IrFile, IrType};
use corvid_resolve::DefId;
use corvid_runtime::{
    load_dotenv_walking, AnthropicAdapter, OpenAiAdapter, ReplayDifferentialReport,
    ReplayMutationReport, Runtime, RuntimeBuilder, StdinApprover,
};
use corvid_trace_schema::{read_events_from_path, validate_supported_schema, TraceEvent};
use corvid_vm::{json_to_value, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Which replay mode to run in. Matches the user-visible
/// `corvid replay` flags one-to-one.
#[derive(Debug, Clone)]
pub enum ReplayMode {
    /// Byte-identical reproduction.
    Plain,
    /// Differential replay against the named model.
    Differential(String),
    /// Counterfactual: override the substitutable-event at 1-based
    /// `step` with `replacement`.
    Mutation {
        step_1based: usize,
        replacement: serde_json::Value,
    },
}

/// Result of a replay execution.
///
/// Exactly one of `differential_report` / `mutation_report` is
/// `Some`, by construction of [`ReplayMode`]. Plain mode carries
/// neither — plain replay has no divergence concept; if the run
/// fails to reproduce, the runtime surfaces a typed
/// `ReplayDivergence` error at [`result_error`] rather than a
/// report.
#[derive(Debug)]
pub struct ReplayOutcome {
    /// The agent name that ran. Extracted from the trace's
    /// `RunStarted` event.
    pub agent_name: String,
    /// The agent's return value, or `None` if the run errored.
    pub result_value: Option<Value>,
    /// Human-readable error if the run did not complete cleanly.
    pub result_error: Option<String>,
    /// Differential-replay report, when `mode == Differential`.
    pub differential_report: Option<ReplayDifferentialReport>,
    /// Mutation-replay report, when `mode == Mutation`.
    pub mutation_report: Option<ReplayMutationReport>,
}

impl ReplayOutcome {
    /// True iff the run reached `RunCompleted` without a typed
    /// runtime error. Divergence reports may still carry data
    /// even on successful completion — "clean run + divergences"
    /// is the expected shape for a differential replay where the
    /// live model disagreed with the recording.
    pub fn ran_cleanly(&self) -> bool {
        self.result_error.is_none()
    }
}

/// Run `<trace>` against `<source>` in `<mode>`, using the default
/// CLI-style runtime scaffolding: env-driven LLM adapters
/// (`ANTHROPIC_API_KEY` / `OPENAI_API_KEY`), optional
/// `CORVID_MODEL` default, and `StdinApprover`. For callers that
/// need to inject mock adapters or other test-mode state, use
/// [`run_replay_from_source_with_builder`] directly.
pub fn run_replay_from_source(
    trace_path: &Path,
    source_path: &Path,
    mode: ReplayMode,
) -> Result<ReplayOutcome> {
    let mut base = Runtime::builder().approver(Arc::new(StdinApprover::new()));
    if let Ok(model) = std::env::var("CORVID_MODEL") {
        base = base.default_model(&model);
    }
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        base = base.llm(Arc::new(AnthropicAdapter::new(key)));
    }
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        base = base.llm(Arc::new(OpenAiAdapter::new(key)));
    }
    run_replay_from_source_with_builder(trace_path, source_path, mode, base)
}

/// Caller-supplied-builder variant of [`run_replay_from_source`].
/// The caller configures the `RuntimeBuilder` with whatever
/// adapters + approver they need (for tests: `MockAdapter` +
/// `ProgrammaticApprover`; for production: real API-key-backed
/// adapters + `StdinApprover`); this function threads the replay
/// mode onto that builder and executes.
///
/// Rationale for the split: the default wiring pulls API keys
/// from the environment, which makes end-to-end tests impossible
/// without mocking the environment. The builder-variant lets the
/// test inject mock adapters as first-class runtime state while
/// keeping the production path a single-argument function.
pub fn run_replay_from_source_with_builder(
    trace_path: &Path,
    source_path: &Path,
    mode: ReplayMode,
    base_builder: RuntimeBuilder,
) -> Result<ReplayOutcome> {
    let tokio_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime for replay")?;
    tokio_rt.block_on(run_replay_from_source_with_builder_async(
        trace_path,
        source_path,
        mode,
        base_builder,
    ))
}

/// Async variant of [`run_replay_from_source_with_builder`].
///
/// Runs entirely on the caller's tokio runtime — no nested runtime
/// construction. Required for callers already inside an async
/// context (e.g. [`crate::run_prod_as_test_suite`]'s harness
/// runner closure, which runs inside `run_test_from_traces`'s
/// tokio context).
pub async fn run_replay_from_source_with_builder_async(
    trace_path: &Path,
    source_path: &Path,
    mode: ReplayMode,
    base_builder: RuntimeBuilder,
) -> Result<ReplayOutcome> {
    // Env discovery for API keys. Same walk-upward behavior as
    // `corvid run` — honors `.env` files next to the source and at
    // the cwd.
    if let Some(parent) = source_path.parent() {
        let _ = load_dotenv_walking(parent);
    }
    let _ = load_dotenv_walking(
        &std::env::current_dir().unwrap_or_else(|_| Path::new(".").into()),
    );

    // Load + schema-validate trace.
    let events = read_events_from_path(trace_path)
        .with_context(|| format!("failed to load trace at `{}`", trace_path.display()))?;
    if events.is_empty() {
        anyhow::bail!("trace `{}` is empty", trace_path.display());
    }
    validate_supported_schema(&events).with_context(|| {
        format!("trace `{}` uses an unsupported schema", trace_path.display())
    })?;

    // Compile source → IR.
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
            "trace `{}` has no `RunStarted` event; cannot determine which agent ran",
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
                 possible reasons: the source has been renamed, the trace was recorded against a \
                 different file, or the agent was removed",
                source_path.display()
            )
        })?;
    let args = convert_json_args(agent, &ir, &json_args)?;

    // Thread the replay mode onto the caller's builder and build.
    let runtime = configure_replay_mode(base_builder, trace_path, &mode).build();

    let run_result = run_ir_with_runtime(&ir, Some(&agent_name), args, &runtime).await;

    let (result_value, result_error) = match run_result {
        Ok(value) => (Some(value), None),
        Err(RunError::Interp(err)) => (None, Some(err.to_string())),
        Err(other) => (None, Some(other.to_string())),
    };

    Ok(ReplayOutcome {
        agent_name,
        result_value,
        result_error,
        differential_report: runtime.replay_differential_report(),
        mutation_report: runtime.replay_mutation_report(),
    })
}

/// Walk trace events for the first `RunStarted` and return its
/// `(agent, args)`. A well-formed trace has exactly one; a
/// malformed trace may have zero (caller handles the `None`) or
/// more than one (this helper keeps the first for determinism).
fn find_run_started(events: &[TraceEvent]) -> Option<(String, Vec<serde_json::Value>)> {
    events.iter().find_map(|e| match e {
        TraceEvent::RunStarted { agent, args, .. } => Some((agent.clone(), args.clone())),
        _ => None,
    })
}

/// Convert the JSON-valued args recorded in the trace into typed
/// `Value`s, using the agent's declared parameter types. Arity
/// mismatch or type coercion failure surfaces as a typed error
/// so the caller can point at the exact parameter that broke.
///
/// Exposed at `pub(crate)` so the promote-mode fresh-record helper
/// in [`super::trace_fresh`] reuses the same conversion quality as
/// replay — a promoted trace's fresh run must consume the recorded
/// args identically to how the original replay would.
pub(crate) fn convert_json_args_for_promote(
    agent: &IrAgent,
    ir: &IrFile,
    json_args: &[serde_json::Value],
) -> Result<Vec<Value>> {
    convert_json_args(agent, ir, json_args)
}

fn convert_json_args(
    agent: &IrAgent,
    ir: &IrFile,
    json_args: &[serde_json::Value],
) -> Result<Vec<Value>> {
    if json_args.len() != agent.params.len() {
        anyhow::bail!(
            "recorded args arity {} does not match agent `{}` parameter count {}; \
             the trace may have been recorded against a different signature of this agent",
            json_args.len(),
            agent.name,
            agent.params.len()
        );
    }
    let types_by_id: HashMap<DefId, &IrType> =
        ir.types.iter().map(|t| (t.id, t)).collect();
    let mut out = Vec::with_capacity(json_args.len());
    for (json, param) in json_args.iter().zip(agent.params.iter()) {
        let value = json_to_value(json.clone(), &param.ty, &types_by_id).map_err(|err| {
            anyhow!(
                "failed to convert recorded arg for `{}: {:?}`: {err}",
                param.name,
                param.ty
            )
        })?;
        out.push(value);
    }
    Ok(out)
}

/// Thread a replay mode onto a pre-configured `RuntimeBuilder`
/// without building the runtime. Caller has already installed
/// adapters + approver + tracer; this helper only layers the
/// replay-specific state (`replay_trace`, `replay_model_swap`,
/// `replay_mutation`) on top.
///
/// Split out from [`run_replay_from_source_with_builder`] so
/// advanced callers that want to inspect the pre-build state
/// (e.g. tests asserting adapter registration) can build their
/// own runtime.
pub fn configure_replay_mode(
    mut builder: RuntimeBuilder,
    trace_path: &Path,
    mode: &ReplayMode,
) -> RuntimeBuilder {
    match mode {
        ReplayMode::Plain => {
            builder = builder.replay_from(trace_path);
        }
        ReplayMode::Differential(model_id) => {
            builder = builder.differential_replay_from(trace_path, model_id.as_str());
        }
        ReplayMode::Mutation {
            step_1based,
            replacement,
        } => {
            builder = builder.mutation_replay_from(
                trace_path,
                *step_1based,
                replacement.clone(),
            );
        }
    }
    builder
}
