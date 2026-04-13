//! Corvid native runtime.
//!
//! Provides the support library both backends (interpreter + future
//! Cranelift codegen) call into:
//!
//! * **Tool dispatch** via `ToolRegistry` — async handlers keyed by name.
//! * **Approval flow** via the `Approver` trait — stdin or programmatic.
//! * **LLM adapters** via `LlmRegistry` — model-prefix dispatch over a
//!   trait-object adapter list. Slice 2a ships only the mock adapter;
//!   real `claude-*` HTTP dispatch lands in slice 2b.
//! * **Tracing** via `Tracer` — JSONL events to disk, swallowing IO
//!   errors so a broken trace cannot crash an agent.
//!
//! The boundary is JSON: handlers and adapters take and return
//! `serde_json::Value`. The interpreter converts to and from its own
//! `Value` type at the call boundary. This keeps `corvid-runtime`
//! independent of `corvid-vm` and matches the wire format every real
//! tool / LLM uses.
//!
//! See `ARCHITECTURE.md` §6.

#![forbid(unsafe_code)]
#![allow(dead_code)]

pub mod approvals;
pub mod env;
pub mod errors;
pub mod llm;
pub mod redact;
pub mod runtime;
pub mod tools;
pub mod tracing;

pub use approvals::{
    ApprovalDecision, ApprovalRequest, Approver, ProgrammaticApprover, StdinApprover,
};
pub use env::{find_dotenv_walking, load_dotenv, load_dotenv_walking};
pub use errors::RuntimeError;
pub use redact::RedactionSet;
pub use llm::{
    anthropic::AnthropicAdapter, mock::MockAdapter, openai::OpenAiAdapter, LlmAdapter,
    LlmRegistry, LlmRequest, LlmResponse,
};
pub use runtime::{Runtime, RuntimeBuilder};
pub use tools::{ToolHandler, ToolRegistry};
pub use tracing::{fresh_run_id, now_ms, TraceEvent, Tracer};
