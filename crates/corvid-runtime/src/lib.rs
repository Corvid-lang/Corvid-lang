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
pub mod approval_authorization;
pub mod approval_policy;
pub mod approval_queue;
pub mod approval_ui;
pub mod approvals;
pub mod approver_bridge;
pub(crate) mod attestation_store;
pub mod auth;
pub mod calibration;
pub mod cache;
pub mod capability_contract;
pub mod catalog;
pub mod catalog_c_api;
pub mod citation;
pub mod db;
pub mod effect_filter;
pub mod ensemble;
pub mod env;
pub mod errors;
pub mod ffi_bridge;
pub mod grounded_handles;
pub mod human;
pub mod http;
pub mod io;
pub mod lineage;
pub mod lineage_render;
pub mod llm;
pub mod models;
mod native_trace;
pub mod observation_handles;
pub mod observe;
pub mod prompt_cache;
pub mod provenance;
#[cfg(feature = "python")]
pub mod python_ffi;
pub mod queue;
pub mod rag;
pub mod record;
pub mod redact;
pub mod replay;
pub mod replay_dispatch;
pub mod runtime;
pub mod secrets;
pub mod store;
pub mod test_from_traces;
pub mod tools;
pub mod tracing;
pub mod usage;

// Re-exports consumed by `corvid-macros`-expanded code. The proc-macro
// emits `::corvid_runtime::inventory::submit! { ... }` and
// `::corvid_runtime::ToolMetadata { ... }`; users never write these
// paths by hand.
pub use abi::{
    registered_tool_count, CorvidGroundedBoolReturn, CorvidGroundedFloatReturn,
    CorvidGroundedHandle, CorvidGroundedIntReturn, CorvidGroundedStringReturn,
    CorvidObservationHandle, CorvidString, ToolMetadata, CORVID_NULL_GROUNDED_HANDLE,
    CORVID_NULL_OBSERVATION_HANDLE,
};
pub use inventory;

/// Path to the C-runtime staticlib (`corvid_c_runtime.lib` / `.a`)
/// that corvid-runtime's build.rs compiled. Used by corvid-codegen-cl's
/// `link.rs` when assembling a Corvid binary outside the cargo
/// link-step machinery (cargo's `rustc-link-lib=static=...` only flows
/// through cargo-managed builds).
pub mod c_runtime {
    include!(concat!(env!("OUT_DIR"), "/c_runtime_path.rs"));
}

pub use adversarial::{contradiction_flag, trace_text};
pub use approvals::{
    ApprovalCard, ApprovalCardArgument, ApprovalDecision, ApprovalRequest, ApprovalRisk, Approver,
    ProgrammaticApprover, StdinApprover,
};
pub use approval_authorization::{
    authorize_approval_transition, ApprovalActorContext, ApprovalTransitionKind,
};
pub use approval_queue::{
    ApprovalAuditCoverage, ApprovalContractRecord, ApprovalCreate, ApprovalQueueAuditEvent,
    ApprovalQueueRecord, ApprovalQueueRuntime,
};
pub use approval_policy::{
    validate_approval_contract_policy, validate_approval_contract_policy_at,
    ApprovalContractPolicyReport,
};
pub use approval_ui::{
    approval_ui_payload, check_approval_ui_contract, ApprovalUiAuditEvent, ApprovalUiContractCheck,
    ApprovalUiPayload, ApprovalUiTarget,
};
pub use auth::{
    authorize_trace_permission, hash_api_key_secret, hash_oauth_state, hash_session_secret,
    validate_jwt_verification_contract, verify_api_key_secret, ApiKeyCreate, ApiKeyRecord,
    ApiKeyResolution, AuthActor, AuthAuditEvent, AuthTraceContext, AuthorizationDecision,
    JwtContractDiagnostic, JwtVerificationContract, OAuthCallbackResolution, OAuthStateCreate,
    OAuthStateRecord, PermissionRequirement, SessionAuthRuntime, SessionCreate, SessionRecord,
    SessionResolution,
};
pub use calibration::{CalibrationObservation, CalibrationStats};
pub use cache::{
    build_cache_key, cache_entry_metadata, CacheEntry, CacheEntryMetadata, CacheKey,
    CacheKeyInput, CacheRuntime,
};
pub use capability_contract::{
    CapabilityCheckKind, CapabilityCheckStatus, CapabilityContractCheck, CapabilityContractOptions,
    CapabilityContractReport,
};
pub use catalog::{
    call_agent as catalog_call_agent, descriptor_hash as catalog_descriptor_hash,
    descriptor_json as catalog_descriptor_json, find_agents_where as catalog_find_agents_where,
    list_agents as catalog_list_agents, pre_flight, CorvidAgentHandle, CorvidApprovalDecision,
    CorvidApprovalRequired, CorvidApproverFn, CorvidCallStatus, CorvidFindAgentsResult,
    CorvidPreFlight, CorvidPreFlightStatus, CorvidTrustTier,
};
pub use corvid_trace_schema::{TraceEvent, WRITER_INTERPRETER, WRITER_NATIVE};
pub use db::{
    decode_i64 as db_decode_i64, decode_string as db_decode_string, DbCell, DbDecodeError,
    DbExecuteResult, DbQueryRows, DbValue, PostgresDbRuntime, SqliteDbRuntime,
};
pub use effect_filter::CorvidFindAgentsStatus;
pub use ensemble::{majority_vote, weighted_vote, EnsembleVoteOutcome};
pub use env::{find_dotenv_walking, load_dotenv, load_dotenv_walking};
pub use errors::RuntimeError;
pub use http::{
    record_exchange, request_fingerprint, HttpClient, HttpHeader, HttpRequest, HttpResponse,
    HttpRetryPolicy, RecordedHttpExchange,
};
pub use io::{DirectoryEntry, FileRead, FileSystemEffect, FileWrite, IoRuntime, TextLineStream};
pub use lineage::{
    lineage_span_id, validate_lineage, LineageEvent, LineageKind, LineageStatus, LineageValidation,
    LINEAGE_SCHEMA,
};
pub use lineage_render::render_lineage_tree;
pub use llm::{
    anthropic::AnthropicAdapter,
    gemini::GeminiAdapter,
    mock::{EnvVarMockAdapter, MockAdapter},
    ollama::OllamaAdapter,
    openai::OpenAiAdapter,
    openai_compat::OpenAiCompatibleAdapter,
    LlmAdapter, LlmRegistry, LlmRequest, LlmResponse, ProviderHealth, TokenUsage,
};
pub use models::{ModelCatalog, ModelSelection, RegisteredModel};
pub use observe::{
    approval_summary, latency_histogram, provider_observations, route_summaries,
    runtime_observation_summary, ApprovalObservationSummary, LatencyObservation,
    ProviderObservation, RouteObservationSummary, RuntimeObservationSummary,
};
pub use provenance::{GroundedValue, ProvenanceChain, ProvenanceEntry, ProvenanceKind};
pub use queue::{DurableQueueRuntime, QueueJob, QueueJobStatus, QueueRuntime};
pub use rag::{
    chunk_document, chunk_document_with_config, document_from_text, load_html, load_markdown,
    load_pdf, EmbedderConfig, EmbeddingVector, OllamaEmbedder, OpenAiEmbedder, RagChunk,
    RagChunkingConfig, RagDocument, RagEmbedder, RagEmbeddingRecord, RagSearchHit, RagSqliteIndex,
    OLLAMA_EMBEDDING_BASE, OPENAI_EMBEDDING_BASE,
};
pub use record::Recorder;
pub use redact::RedactionSet;
pub use replay::{
    LlmDivergence, MutationDivergence, ReplayDifferentialReport, ReplayDivergence,
    ReplayMutationReport, ReplaySource, RunCompletionDivergence, SubstitutionDivergence,
};
pub use runtime::{Runtime, RuntimeBuilder};
pub use secrets::{SecretAuditMetadata, SecretRead, SecretRuntime};
#[cfg(feature = "python")]
pub use python_ffi::{PythonRuntime, PythonSandboxProfile};
pub use store::{
    InMemoryStoreBackend, SqliteStoreBackend, StoreBackend, StoreKind, StoreManager,
    StorePolicySet, StoreRecord,
};
pub use test_from_traces::{
    run_test_from_traces, Divergence, FlakeRank, ModelSwapOutcome, PromoteDecision,
    PromotePromptMode, TestFromTracesOptions, TestFromTracesReport, TestFromTracesSummary,
    TraceHarnessMode, TraceHarnessRequest, TraceHarnessRun, TraceOutcome, Verdict,
};
pub use tools::{ToolHandler, ToolRegistry};
pub use tracing::{fresh_run_id, now_ms, Tracer};
pub use usage::{LlmUsageLedger, LlmUsageRecord, LlmUsageTotals};
