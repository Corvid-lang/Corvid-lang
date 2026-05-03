use crate::Diagnostic;
use corvid_vm::TestExecution;
use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum EvalRunnerError {
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
}

impl fmt::Display for EvalRunnerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => {
                write!(f, "failed to access `{}`: {error}", path.display())
            }
        }
    }
}

impl std::error::Error for EvalRunnerError {}

#[derive(Debug, Clone)]
pub struct CorvidEvalReport {
    pub source_path: PathBuf,
    pub compile_diagnostics: Vec<Diagnostic>,
    pub evals: Vec<TestExecution>,
    pub html_report_path: PathBuf,
    pub regression: EvalRegressionReport,
    pub trace: EvalTraceReport,
}

impl CorvidEvalReport {
    pub fn passed(&self) -> bool {
        self.compile_diagnostics.is_empty()
            && !self.evals.is_empty()
            && self.evals.iter().all(TestExecution::passed)
    }

    pub fn exit_code(&self) -> u8 {
        if self.passed() {
            0
        } else {
            1
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EvalTraceReport {
    pub trace_count: usize,
    pub replay_compatible_count: usize,
    pub invalid_traces: Vec<String>,
    pub value_assertions_passed: usize,
    pub value_assertions_total: usize,
    pub process_assertions_passed: usize,
    pub process_assertions_total: usize,
    pub approval_assertions_passed: usize,
    pub approval_assertions_total: usize,
    pub tool_calls: usize,
    pub prompt_calls: usize,
    pub approval_events: usize,
    pub grounded_edges: usize,
    pub total_cost_usd: f64,
    pub total_latency_ms: u64,
    pub prompts: Vec<String>,
    pub prompt_renders: Vec<EvalPromptRender>,
    pub model_routes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalPromptRender {
    pub prompt: String,
    pub rendered: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvalRegressionReport {
    pub prior_path: PathBuf,
    pub current_path: PathBuf,
    pub regressions: Vec<EvalRegression>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalRegression {
    pub eval: String,
    pub assertion: Option<String>,
    pub before: String,
    pub after: String,
}
