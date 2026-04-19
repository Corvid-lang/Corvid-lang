use corvid_trace_schema::TraceEvent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReplayDifferentialReport {
    pub llm_divergences: Vec<LlmDivergence>,
    pub substitution_divergences: Vec<SubstitutionDivergence>,
    pub run_completion_divergence: Option<RunCompletionDivergence>,
}

impl ReplayDifferentialReport {
    pub fn is_empty(&self) -> bool {
        self.llm_divergences.is_empty()
            && self.substitution_divergences.is_empty()
            && self.run_completion_divergence.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmDivergence {
    pub step: usize,
    pub prompt: String,
    pub recorded: serde_json::Value,
    pub live: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubstitutionDivergence {
    pub step: usize,
    pub expected: TraceEvent,
    pub got_kind: String,
    pub got_description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunCompletionDivergence {
    pub step: usize,
    pub recorded_ok: bool,
    pub recorded_result: Option<serde_json::Value>,
    pub recorded_error: Option<String>,
    pub live_ok: bool,
    pub live_result: Option<serde_json::Value>,
    pub live_error: Option<String>,
}
