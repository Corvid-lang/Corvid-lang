//! Corvid native runtime.
//!
//! Provides the support library both backends (interpreter + future
//! Cranelift codegen) call into:
//!
//! * **Tool dispatch** via `ToolRegistry` — async handlers keyed by name.
//! * **Approval flow** via the `Approver` trait — stdin or programmatic.
//! * **LLM adapters** via `LlmRegistry` — model-prefix dispatch over a
//!   trait-object adapter list, including the mock adapter used by tests
//!   and offline demos.
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

// The native async runtime introduces a C-ABI bridge module
// (`ffi_bridge`) that compiled Corvid binaries call into. That module must
// use `unsafe` to handle raw pointers across the FFI boundary — it's the
// only place in the crate where unsafe is allowed. Every other module
// must stay unsafe-free; the file-level `#![deny(unsafe_code)]` enforces
// that, and `ffi_bridge` opts in explicitly with a module-level allow
// alongside a written rationale.
#![deny(unsafe_code)]
#![allow(dead_code)]

pub mod abi;
pub mod adversarial;
pub mod approvals;
pub mod approver_bridge;
pub mod catalog;
pub mod catalog_c_api;
pub mod effect_filter;
pub mod ensemble;
pub mod env;
pub mod errors;
pub mod ffi_bridge;
pub mod llm;
pub mod models;
mod native_trace;
pub mod record;
pub mod replay;
pub mod replay_dispatch;
pub mod redact;
pub mod runtime;
pub mod test_from_traces;
pub mod tools;
pub mod tracing;

// Re-exports consumed by `corvid-macros`-expanded code. The proc-macro
// emits `::corvid_runtime::inventory::submit! { ... }` and
// `::corvid_runtime::ToolMetadata { ... }`; users never write these
// paths by hand.
pub use abi::{registered_tool_count, CorvidString, ToolMetadata};
pub use inventory;

/// Path to the C-runtime staticlib (`corvid_c_runtime.lib` / `.a`)
/// that corvid-runtime's build.rs compiled. Used by corvid-codegen-cl's
/// `link.rs` when assembling a Corvid binary outside the cargo
/// link-step machinery (cargo's `rustc-link-lib=static=...` only flows
/// through cargo-managed builds).
pub mod c_runtime {
    include!(concat!(env!("OUT_DIR"), "/c_runtime_path.rs"));
}

pub use approvals::{
    ApprovalDecision, ApprovalRequest, Approver, ProgrammaticApprover, StdinApprover,
};
pub use catalog::{
    call_agent as catalog_call_agent, descriptor_hash as catalog_descriptor_hash,
    descriptor_json as catalog_descriptor_json, find_agents_where as catalog_find_agents_where,
    list_agents as catalog_list_agents, pre_flight, CorvidAgentHandle, CorvidApprovalDecision,
    CorvidApprovalRequired, CorvidApproverFn, CorvidCallStatus, CorvidFindAgentsResult,
    CorvidPreFlight, CorvidPreFlightStatus, CorvidTrustTier,
};
pub use effect_filter::CorvidFindAgentsStatus;
pub use adversarial::{contradiction_flag, trace_text};
pub use ensemble::{majority_vote, EnsembleVoteOutcome};
pub use env::{find_dotenv_walking, load_dotenv, load_dotenv_walking};
pub use errors::RuntimeError;
pub use redact::RedactionSet;
pub use llm::{
    anthropic::AnthropicAdapter,
    gemini::GeminiAdapter,
    mock::{EnvVarMockAdapter, MockAdapter},
    ollama::OllamaAdapter,
    openai::OpenAiAdapter,
    openai_compat::OpenAiCompatibleAdapter,
    LlmAdapter, LlmRegistry, LlmRequest, LlmResponse, TokenUsage,
};
pub use models::{ModelCatalog, ModelSelection, RegisteredModel};
pub use record::Recorder;
pub use replay::{
    LlmDivergence, MutationDivergence, ReplayDifferentialReport, ReplayDivergence,
    ReplayMutationReport, ReplaySource, RunCompletionDivergence, SubstitutionDivergence,
};
pub use runtime::{Runtime, RuntimeBuilder};
pub use test_from_traces::{
    run_test_from_traces, Divergence, FlakeRank, ModelSwapOutcome, PromoteDecision,
    PromotePromptMode, TestFromTracesOptions, TestFromTracesReport, TestFromTracesSummary,
    TraceHarnessMode, TraceHarnessRequest, TraceHarnessRun, TraceOutcome, Verdict,
};
pub use tools::{ToolHandler, ToolRegistry};
pub use corvid_trace_schema::{TraceEvent, WRITER_INTERPRETER, WRITER_NATIVE};
pub use tracing::{fresh_run_id, now_ms, Tracer};
