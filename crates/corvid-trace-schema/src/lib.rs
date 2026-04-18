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
}
