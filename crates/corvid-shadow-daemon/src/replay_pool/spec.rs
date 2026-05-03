use super::*;

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
        self.shadow_events
            .iter()
            .map(normalize_event_json)
            .collect()
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
