//! The `TraceEvent` taxonomy — every nondeterministic input or
//! dispatch decision the runtime records into a trace.
//!
//! The tag field is `kind`, and each variant has a flat struct
//! shape so the wire form reads naturally in JSONL:
//!
//! ```jsonl
//! {"kind":"schema_header","version":1,"writer":"corvid-vm","commit_sha":null,"ts_ms":0,"run_id":"r-1"}
//! {"kind":"run_started","ts_ms":1,"run_id":"r-1","agent":"demo","args":[]}
//! ```
//!
//! Every new event variant should be additive: old readers must
//! skip unknown variants rather than fail. Existing variants must
//! not change shape; evolving a variant means bumping
//! `SCHEMA_VERSION` and teaching readers how to upgrade old traces.

use serde::{Deserialize, Serialize};

/// Every event emitted by the runtime. Serialized one-per-line in
/// JSONL. A trace file typically starts with a `SchemaHeader`,
/// followed by exactly one `RunStarted`, then interleaved
/// `ToolCall` / `LlmCall` / `ApprovalResponse` / `SeedRead` /
/// `ClockRead` / dispatch events, and closes with a single
/// `RunCompleted`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TraceEvent {
    /// First event in every trace. Identifies the schema version
    /// the file was written against so readers can refuse
    /// incompatible traces or apply version-specific upgrade
    /// logic.
    SchemaHeader {
        version: u32,
        /// Identifier for the recording tier (`"corvid-vm"`,
        /// `"corvid-codegen-cl"`, etc.) — useful for cross-tier
        /// debugging when the runtime is ambiguous.
        writer: String,
        /// Git commit SHA the recording binary was built from, if
        /// known. `None` in tests and local dev builds without
        /// version injection.
        #[serde(default)]
        commit_sha: Option<String>,
        /// Path to the Corvid source file the recording ran against,
        /// relative to whatever anchor the recorder chose (typically
        /// the repo root or the CWD at record time). `None` for
        /// pre-schema-v2 traces and for run modes where no source
        /// file exists (REPL, ad-hoc bytecode). Present in v2+ traces
        /// so `corvid replay <trace>` can locate the source without a
        /// sidecar — the trace is self-describing.
        #[serde(default)]
        source_path: Option<String>,
        ts_ms: u64,
        run_id: String,
    },
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
        #[serde(default)]
        model: Option<String>,
        result: serde_json::Value,
    },
    ApprovalRequest {
        ts_ms: u64,
        run_id: String,
        label: String,
        args: Vec<serde_json::Value>,
    },
    ApprovalDecision {
        ts_ms: u64,
        run_id: String,
        site: String,
        args: Vec<serde_json::Value>,
        accepted: bool,
        decider: String,
        #[serde(default)]
        rationale: Option<String>,
    },
    ApprovalResponse {
        ts_ms: u64,
        run_id: String,
        label: String,
        approved: bool,
    },
    HostEvent {
        ts_ms: u64,
        run_id: String,
        name: String,
        payload: serde_json::Value,
    },
    /// A pseudo-random number read. Recorded per draw so replay
    /// can reproduce the exact sequence even when the seeded PRNG
    /// runs through different call paths.
    SeedRead {
        ts_ms: u64,
        run_id: String,
        /// Human-readable reason for the draw (`"rollout_cohort"`,
        /// `"retry_jitter"`, etc.). Useful for debugging a
        /// divergent replay — the kind names tell you which draw
        /// went missing.
        purpose: String,
        /// Raw PRNG output, u64. Consumers that need a different
        /// shape (bool, f64, range) re-derive from this via the
        /// same transformation used at record time.
        value: u64,
    },
    /// A read from a clock source. Every such read must be
    /// replayable for `@replayable` agents to compile.
    ClockRead {
        ts_ms: u64,
        run_id: String,
        /// `"wall"` (epoch-ms), `"monotonic"` (ns since process
        /// start), or `"system_start"` (epoch-ms of process boot).
        /// Named `source` not `kind` to avoid colliding with the
        /// enum's serde tag field.
        source: String,
        /// Raw clock value. Units depend on `source`; the
        /// consumer is responsible for matching units on replay.
        value: i64,
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
        #[serde(default)]
        stage_index: Option<usize>,
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
    /// A provenance edge in the run's Grounded<T> dataflow graph.
    /// Emitted whenever the runtime constructs a Grounded<T> value
    /// from one or more upstream Grounded<T> inputs — lets
    /// `corvid trace dag` render the exact dataflow a posteriori
    /// without the renderer needing to understand runtime internals.
    ///
    /// Additive in schema v2 — old readers (v1 + v2-pre-provenance)
    /// skip unknown `kind` values, so no version bump required.
    ProvenanceEdge {
        ts_ms: u64,
        run_id: String,
        /// Stable identifier for the value this edge produces.
        /// Must be unique within a run. Recorder convention: a
        /// monotonic counter prefixed by the op kind
        /// (`"tool:17"`, `"llm:4"`, `"approve:2"`) so the same
        /// value has the same id across record + replay.
        node_id: String,
        /// Upstream `node_id`s whose values flowed into this one.
        /// Empty for root inputs (tool results with no
        /// Grounded<T> arguments, LLM calls with no grounded
        /// prompt parts).
        #[serde(default)]
        parents: Vec<String>,
        /// Operation that produced this value, in
        /// `<kind>:<name>` form. Examples: `"tool_call:get_order"`,
        /// `"llm:classify"`, `"approve:IssueRefund"`,
        /// `"literal:42"`.
        op: String,
        /// Optional human-readable label for the DAG renderer.
        /// `None` when the node_id is already self-describing.
        #[serde(default)]
        label: Option<String>,
    },
}
