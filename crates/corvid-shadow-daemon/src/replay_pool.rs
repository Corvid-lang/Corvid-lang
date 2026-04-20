use anyhow::{Context, Result};
use async_trait::async_trait;
use corvid_ast::{AgentAttribute, Decl, Effect};
use corvid_driver::{compile_to_ir_with_config, load_corvid_config_for};
use corvid_ir::IrFile;
use corvid_runtime::{
    AnthropicAdapter, EnvVarMockAdapter, OpenAiAdapter, ProgrammaticApprover,
    RedactionSet, ReplayDifferentialReport, ReplayDivergence, ReplayMutationReport, Runtime,
    RuntimeError, TraceEvent, Tracer, WRITER_INTERPRETER,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrustTier {
    Autonomous,
    HumanRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DimensionSnapshot {
    pub cost: f64,
    pub latency_ms: u64,
    pub trust_tier: Option<TrustTier>,
    pub budget_declared: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProvenanceSnapshot {
    pub nodes: BTreeSet<String>,
    pub root_sources: BTreeSet<String>,
    pub has_chain: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DangerousToolSpec {
    pub tool: String,
    pub approval_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentInvariantInfo {
    pub agent: String,
    pub replayable: bool,
    pub deterministic: bool,
    pub grounded_return: bool,
    pub budget_declared: Option<f64>,
    pub dangerous_tools: Vec<DangerousToolSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationSpec {
    pub step_1based: usize,
    pub replacement: serde_json::Value,
    pub label: String,
}

#[derive(Debug, Clone)]
pub enum ShadowExecutionMode {
    Replay,
    Differential { model: String },
    Mutation(MutationSpec),
}

#[derive(Debug, Clone)]
pub struct ShadowReplayOutcome {
    pub trace_path: PathBuf,
    pub run_id: String,
    pub agent: String,
    pub recorded_events: Vec<TraceEvent>,
    pub shadow_trace_path: PathBuf,
    pub shadow_events: Vec<TraceEvent>,
    pub recorded_output: Option<serde_json::Value>,
    pub shadow_output: Option<serde_json::Value>,
    pub replay_divergence: Option<ReplayDivergence>,
    pub differential_report: Option<ReplayDifferentialReport>,
    pub mutation_report: Option<ReplayMutationReport>,
    pub recorded_dimensions: DimensionSnapshot,
    pub shadow_dimensions: DimensionSnapshot,
    pub recorded_provenance: ProvenanceSnapshot,
    pub shadow_provenance: ProvenanceSnapshot,
    pub metadata: AgentInvariantInfo,
    pub mode: String,
    pub ok: bool,
    pub error: Option<String>,
}

impl ShadowReplayOutcome {
    pub fn normalized_recorded_events(&self) -> Vec<serde_json::Value> {
        self.recorded_events
            .iter()
            .map(normalize_event_json)
            .collect()
    }

    pub fn normalized_shadow_events(&self) -> Vec<serde_json::Value> {
        self.shadow_events.iter().map(normalize_event_json).collect()
    }

    pub fn traces_match(&self) -> bool {
        self.normalized_recorded_events() == self.normalized_shadow_events()
    }

    pub fn seed_clock_positions(events: &[TraceEvent]) -> Vec<(usize, &'static str)> {
        events
            .iter()
            .enumerate()
            .filter_map(|(idx, event)| match event {
                TraceEvent::SeedRead { .. } => Some((idx, "seed")),
                TraceEvent::ClockRead { .. } => Some((idx, "clock")),
                _ => None,
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
pub enum ShadowExecutorError {
    Io(String),
    Compile(String),
    TraceLoad(String),
    Runtime(RuntimeError),
    Interp(String),
    UnsupportedProgramPath(PathBuf),
}

impl std::fmt::Display for ShadowExecutorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(msg) => f.write_str(msg),
            Self::Compile(msg) => f.write_str(msg),
            Self::TraceLoad(msg) => f.write_str(msg),
            Self::Runtime(err) => err.fmt(f),
            Self::Interp(msg) => f.write_str(msg),
            Self::UnsupportedProgramPath(path) => write!(
                f,
                "shadow daemon v1 requires `daemon.ir_path` to point at a `.cor` source file; `{}` is not supported yet",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ShadowExecutorError {}

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

#[derive(Clone)]
pub struct InterpreterShadowExecutor {
    source_path: PathBuf,
    ir: Arc<IrFile>,
    metadata: Arc<HashMap<String, AgentInvariantInfo>>,
}

impl InterpreterShadowExecutor {
    pub fn from_program_path(path: &Path) -> Result<Self, ShadowExecutorError> {
        if path.extension().and_then(|ext| ext.to_str()) != Some("cor") {
            return Err(ShadowExecutorError::UnsupportedProgramPath(path.to_path_buf()));
        }
        let (ir, metadata) = parse_program_source(path)
            .map_err(|err| ShadowExecutorError::Compile(err.to_string()))?;
        Ok(Self {
            source_path: path.to_path_buf(),
            ir: Arc::new(ir),
            metadata: Arc::new(metadata),
        })
    }

    fn build_runtime(&self, trace_path: &Path, mode: &ShadowExecutionMode) -> Runtime {
        let emit_dir = std::env::temp_dir().join(format!(
            "corvid-shadow-{}",
            corvid_runtime::fresh_run_id()
        ));
        let _ = std::fs::create_dir_all(&emit_dir);
        let tracer = Tracer::open(&emit_dir, corvid_runtime::fresh_run_id())
            .with_redaction(RedactionSet::empty());

        let mut builder = Runtime::builder()
            .tracer(tracer)
            .approver(Arc::new(ProgrammaticApprover::always_yes()))
            .llm(Arc::new(EnvVarMockAdapter::from_env()));

        if let Ok(model) = std::env::var("CORVID_MODEL") {
            builder = builder.default_model(&model);
        }
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            builder = builder.llm(Arc::new(AnthropicAdapter::new(key)));
        }
        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            builder = builder.llm(Arc::new(OpenAiAdapter::new(key)));
        }

        match mode {
            ShadowExecutionMode::Replay => builder.replay_from(trace_path).build(),
            ShadowExecutionMode::Differential { model } => {
                builder.differential_replay_from(trace_path, model.clone()).build()
            }
            ShadowExecutionMode::Mutation(spec) => builder
                .mutation_replay_from(trace_path, spec.step_1based, spec.replacement.clone())
                .build(),
        }
    }
}

#[async_trait]
impl ShadowReplayExecutor for InterpreterShadowExecutor {
    async fn execute(
        &self,
        trace_path: &Path,
        mode: ShadowExecutionMode,
    ) -> Result<ShadowReplayOutcome, ShadowExecutorError> {
        let recorded_events = read_events_from_path(trace_path).map_err(|err| {
            ShadowExecutorError::TraceLoad(format!(
                "failed to load recorded trace `{}`: {err}",
                trace_path.display()
            ))
        })?;
        validate_supported_schema(&recorded_events).map_err(|err| {
            ShadowExecutorError::TraceLoad(format!(
                "unsupported trace schema for `{}`: {err}",
                trace_path.display()
            ))
        })?;

        let writer = recorded_writer(&recorded_events).unwrap_or_default();
        if writer != WRITER_INTERPRETER {
            return Err(ShadowExecutorError::Runtime(
                RuntimeError::CrossTierReplayUnsupported {
                    recorded_writer: writer,
                    replay_writer: WRITER_INTERPRETER.into(),
                },
            ));
        }

        let (agent_name, run_args_json) = recorded_events
            .iter()
            .find_map(|event| match event {
                TraceEvent::RunStarted { agent, args, .. } => Some((agent.clone(), args.clone())),
                _ => None,
            })
            .ok_or_else(|| {
                ShadowExecutorError::TraceLoad(format!(
                    "recorded trace `{}` has no RunStarted event",
                    trace_path.display()
                ))
            })?;

        let recorded_output = recorded_events.iter().find_map(|event| match event {
            TraceEvent::RunCompleted { result, .. } => result.clone(),
            _ => None,
        });
        let run_id = recorded_events
            .iter()
            .find_map(|event| match event {
                TraceEvent::SchemaHeader { run_id, .. }
                | TraceEvent::RunStarted { run_id, .. }
                | TraceEvent::RunCompleted { run_id, .. } => Some(run_id.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "<unknown>".into());

        let agent = self
            .ir
            .agents
            .iter()
            .find(|candidate| candidate.name == agent_name)
            .ok_or_else(|| {
                ShadowExecutorError::Compile(format!(
                    "current program `{}` has no agent named `{agent_name}`",
                    self.source_path.display(),
                ))
            })?;

        let types_by_id = self
            .ir
            .types
            .iter()
            .map(|ty| (ty.id, ty))
            .collect::<HashMap<_, _>>();
        let args = run_args_json
            .iter()
            .zip(agent.params.iter())
            .map(|(json, param)| {
                json_to_value(json.clone(), &param.ty, &types_by_id)
                    .map_err(|err| ShadowExecutorError::Interp(err.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let runtime = self.build_runtime(trace_path, &mode);
        let shadow_trace_path = runtime.tracer().path().to_path_buf();
        let run_result = run_agent(&self.ir, &agent.name, args, &runtime).await;
        let shadow_output = run_result.as_ref().ok().map(value_to_json);
        let error = run_result.as_ref().err().map(|err| err.to_string());
        let ok = run_result.is_ok();
        let replay_divergence = match run_result.as_ref().err().map(|err| &err.kind) {
            Some(corvid_vm::InterpErrorKind::Runtime(RuntimeError::ReplayDivergence(divergence))) => {
                Some(divergence.clone())
            }
            Some(corvid_vm::InterpErrorKind::Runtime(err)) => {
                return Err(ShadowExecutorError::Runtime(err.clone()));
            }
            Some(other) => return Err(ShadowExecutorError::Interp(other.to_string())),
            None => None,
        };

        let shadow_events = read_events_from_path(&shadow_trace_path).map_err(|err| {
            ShadowExecutorError::TraceLoad(format!(
                "failed to load shadow trace `{}`: {err}",
                shadow_trace_path.display()
            ))
        })?;

        let metadata = self
            .metadata
            .get(&agent.name)
            .cloned()
            .unwrap_or_else(|| AgentInvariantInfo {
                agent: agent.name.clone(),
                ..AgentInvariantInfo::default()
            });

        Ok(ShadowReplayOutcome {
            trace_path: trace_path.to_path_buf(),
            run_id,
            agent: agent.name.clone(),
            recorded_dimensions: dimensions_from_trace(&recorded_events, metadata.budget_declared),
            shadow_dimensions: dimensions_from_trace(&shadow_events, metadata.budget_declared),
            recorded_provenance: provenance_from_trace(&recorded_events),
            shadow_provenance: provenance_from_trace(&shadow_events),
            recorded_events,
            shadow_trace_path,
            shadow_events,
            recorded_output,
            shadow_output,
            replay_divergence,
            differential_report: runtime.replay_differential_report(),
            mutation_report: runtime.replay_mutation_report(),
            metadata,
            mode: match mode {
                ShadowExecutionMode::Replay => "replay".into(),
                ShadowExecutionMode::Differential { .. } => "differential".into(),
                ShadowExecutionMode::Mutation(_) => "mutation".into(),
            },
            ok,
            error,
        })
    }
}

pub fn parse_program_source(path: &Path) -> Result<(IrFile, HashMap<String, AgentInvariantInfo>)> {
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read `{}`", path.display()))?;
    let config = load_corvid_config_for(path);
    let ir = compile_to_ir_with_config(&source, config.as_ref()).map_err(|diagnostics| {
        anyhow::anyhow!(
            "{}",
            diagnostics
                .into_iter()
                .map(|d| d.message)
                .collect::<Vec<_>>()
                .join("\n")
        )
    })?;

    let tokens = lex(&source).map_err(|errs| {
        anyhow::anyhow!(
            "{}",
            errs.into_iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        )
    })?;
    let (file, parse_errors) = parse_file(&tokens);
    if !parse_errors.is_empty() {
        anyhow::bail!(
            "{}",
            parse_errors
                .into_iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    let dangerous_tools = file
        .decls
        .iter()
        .filter_map(|decl| match decl {
            Decl::Tool(tool) if matches!(tool.effect, Effect::Dangerous) => Some(DangerousToolSpec {
                tool: tool.name.name.clone(),
                approval_label: approval_label_for_tool(&tool.name.name),
            }),
            _ => None,
        })
        .collect::<Vec<_>>();

    let mut metadata = HashMap::new();
    for agent in &ir.agents {
        let attrs = file
            .decls
            .iter()
            .find_map(|decl| match decl {
                Decl::Agent(candidate) if candidate.name.name == agent.name => {
                    Some(candidate.attributes.clone())
                }
                _ => None,
            })
            .unwrap_or_default();
        metadata.insert(
            agent.name.clone(),
            AgentInvariantInfo {
                agent: agent.name.clone(),
                replayable: AgentAttribute::is_replayable(&attrs),
                deterministic: AgentAttribute::is_deterministic(&attrs),
                grounded_return: matches!(agent.return_ty, Type::Grounded(_)),
                budget_declared: agent.cost_budget,
                dangerous_tools: dangerous_tools.clone(),
            },
        );
    }

    Ok((ir, metadata))
}

pub fn approval_label_for_tool(tool_name: &str) -> String {
    tool_name
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<String>()
}

fn recorded_writer(events: &[TraceEvent]) -> Option<String> {
    events.iter().find_map(|event| match event {
        TraceEvent::SchemaHeader { writer, .. } => Some(writer.clone()),
        _ => None,
    })
}

fn dimensions_from_trace(events: &[TraceEvent], budget_declared: Option<f64>) -> DimensionSnapshot {
    let mut cost = 0.0;
    let mut latency_ms = 0;
    let mut start = None;
    let mut end = None;
    let mut trust = None;

    for event in events {
        match event {
            TraceEvent::RunStarted { ts_ms, .. } => start = Some(*ts_ms),
            TraceEvent::RunCompleted { ts_ms, .. } => end = Some(*ts_ms),
            TraceEvent::ModelSelected { cost_estimate, .. } => cost += cost_estimate,
            TraceEvent::ApprovalRequest { .. } | TraceEvent::ApprovalResponse { .. } => {
                trust = Some(TrustTier::HumanRequired)
            }
            _ => {}
        }
    }

    if let (Some(start), Some(end)) = (start, end) {
        latency_ms = end.saturating_sub(start);
    }
    if trust.is_none() {
        trust = Some(TrustTier::Autonomous);
    }

    DimensionSnapshot {
        cost,
        latency_ms,
        trust_tier: trust,
        budget_declared,
    }
}

fn provenance_from_trace(events: &[TraceEvent]) -> ProvenanceSnapshot {
    let mut nodes = BTreeSet::new();
    let mut roots = BTreeSet::new();
    let mut has_chain = false;
    for event in events {
        if let TraceEvent::ProvenanceEdge {
            node_id,
            parents,
            op,
            ..
        } = event
        {
            nodes.insert(node_id.clone());
            if parents.is_empty() {
                roots.insert(op.clone());
            }
            has_chain = true;
        }
    }
    ProvenanceSnapshot {
        nodes,
        root_sources: roots,
        has_chain,
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
            ok,
            result,
            error,
            ..
        } => serde_json::json!({
            "kind": "run_completed",
            "ts_ms": 0,
            "run_id": "<normalized>",
            "ok": ok,
            "result": result,
            "error": error,
        }),
        other => serde_json::to_value(other).unwrap_or_else(|_| {
            serde_json::json!({
                "kind": "serialization_error",
                "debug": format!("{other:?}"),
            })
        }),
    }
}
