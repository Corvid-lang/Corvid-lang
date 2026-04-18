use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TraceEvent {
    RunStarted {
        ts_ms: u64,
        run_id: String,
        agent: String,
        #[serde(default)]
        args: Vec<serde_json::Value>,
    },
    RunCompleted {
        ts_ms: u64,
        run_id: String,
        ok: bool,
        #[serde(default)]
        result: Option<serde_json::Value>,
        #[serde(default)]
        error: Option<String>,
    },
    ToolCall {
        ts_ms: u64,
        run_id: String,
        tool: String,
        args: Vec<serde_json::Value>,
    },
    ToolResult {
        ts_ms: u64,
        run_id: String,
        tool: String,
        result: serde_json::Value,
    },
    LlmCall {
        ts_ms: u64,
        run_id: String,
        prompt: String,
        model: Option<String>,
        #[serde(default)]
        rendered: Option<String>,
        #[serde(default)]
        args: Vec<serde_json::Value>,
    },
    LlmResult {
        ts_ms: u64,
        run_id: String,
        prompt: String,
        result: serde_json::Value,
    },
    ApprovalRequest {
        ts_ms: u64,
        run_id: String,
        label: String,
        args: Vec<serde_json::Value>,
    },
    ApprovalResponse {
        ts_ms: u64,
        run_id: String,
        label: String,
        approved: bool,
    },
    ModelSelected {
        ts_ms: u64,
        run_id: String,
        prompt: String,
        model: String,
        #[serde(default)]
        capability_required: Option<String>,
        #[serde(default)]
        capability_picked: Option<String>,
        cost_estimate: f64,
        #[serde(default)]
        arm_index: Option<usize>,
    },
    ProgressiveEscalation {
        ts_ms: u64,
        run_id: String,
        prompt: String,
        from_stage: usize,
        to_stage: usize,
        confidence_observed: f64,
        threshold: f64,
    },
    ProgressiveExhausted {
        ts_ms: u64,
        run_id: String,
        prompt: String,
        stages: Vec<String>,
    },
    AbVariantChosen {
        ts_ms: u64,
        run_id: String,
        prompt: String,
        variant: String,
        baseline: String,
        rollout_pct: f64,
        chosen: String,
    },
    EnsembleVote {
        ts_ms: u64,
        run_id: String,
        prompt: String,
        members: Vec<String>,
        results: Vec<String>,
        winner: String,
        agreement_rate: f64,
        strategy: String,
    },
    AdversarialPipelineCompleted {
        ts_ms: u64,
        run_id: String,
        prompt: String,
        contradiction: bool,
    },
    AdversarialContradiction {
        ts_ms: u64,
        run_id: String,
        prompt: String,
        proposed: String,
        challenge: String,
        verdict: serde_json::Value,
    },
}
