//! `Runtime` — the top-level glue the interpreter calls into.
//!
//! Bundles the tool registry, LLM registry, approver, and tracer behind
//! one handle. Construct with `Runtime::builder()`, populate with tools
//! and adapters, freeze with `.build()`. Pass `&Runtime` to the
//! interpreter.

#[cfg(test)]
use crate::approvals::ApprovalToken;
use crate::approvals::Approver;
use crate::cache::{build_cache_key, CacheKey, CacheKeyInput};
use crate::calibration::CalibrationStore;
use crate::errors::RuntimeError;
use crate::http::HttpClient;
use crate::human::HumanInteractor;
use crate::io::IoRuntime;
use crate::llm::LlmRegistry;
use crate::models::ModelCatalog;
#[cfg(test)]
use crate::models::RegisteredModel;
use crate::prompt_cache::PromptCache;
#[cfg(feature = "python")]
use crate::python_ffi::{PythonRuntime, PythonSandboxProfile};
use crate::queue::QueueRuntime;
use crate::record::Recorder;
use crate::replay::ReplaySource;
use crate::secrets::SecretRuntime;
use crate::store::StoreManager;
use crate::tools::ToolRegistry;
use crate::tracing::{now_ms, Tracer};
use crate::usage::LlmUsageLedger;
use corvid_trace_schema::TraceEvent;
#[cfg(test)]
use corvid_trace_schema::WRITER_INTERPRETER;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

pub use builder::RuntimeBuilder;

mod builder;
mod io;
mod jobs;
mod llm_dispatch;
mod model_catalog;
mod replay_reports;
mod store_dispatch;

pub(super) const APPROVAL_TOKEN_SCOPE_ONE_TIME: &str = "one_time";
pub(super) const APPROVAL_TOKEN_TTL_MS: u64 = 5 * 60 * 1000;

#[derive(Clone)]
pub struct Runtime {
    tools: ToolRegistry,
    llms: LlmRegistry,
    approver: Arc<dyn Approver>,
    human: Arc<dyn HumanInteractor>,
    tracer: Tracer,
    recorder: Option<Arc<Recorder>>,
    mode: RuntimeMode,
    replay_error: Option<RuntimeError>,
    /// Default model name applied when a prompt call doesn't specify one.
    /// Empty string means "no default; require per-call override".
    default_model: String,
    model_catalog: ModelCatalog,
    model_catalog_error: Option<RuntimeError>,
    rollout_state: Arc<AtomicU64>,
    calibration: CalibrationStore,
    prompt_cache: PromptCache,
    stores: StoreManager,
    usage_ledger: LlmUsageLedger,
    http: HttpClient,
    io: IoRuntime,
    secrets: SecretRuntime,
    queue: QueueRuntime,
}

#[derive(Clone)]
enum RuntimeMode {
    Live,
    Replay(ReplaySource),
}

impl Runtime {
    pub fn builder() -> RuntimeBuilder {
        RuntimeBuilder::default()
    }

    // ---- accessors used by the interpreter ----

    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
    }

    pub fn tracer(&self) -> &Tracer {
        &self.tracer
    }

    pub fn default_model(&self) -> &str {
        &self.default_model
    }

    pub fn cache_key(&self, input: CacheKeyInput) -> Result<CacheKey, RuntimeError> {
        let key = build_cache_key(input)?;
        self.emit_host_event(
            "std.cache.key",
            serde_json::json!({
                "namespace": key.namespace,
                "subject": key.subject,
                "fingerprint": key.fingerprint,
                "effect_key": key.effect_key,
                "provenance_key": key.provenance_key,
            }),
        );
        Ok(key)
    }

    #[cfg(feature = "python")]
    pub fn call_python_function(
        &self,
        module: &str,
        function: &str,
        args: Vec<serde_json::Value>,
    ) -> Result<serde_json::Value, RuntimeError> {
        self.call_python_function_with_policy(
            module,
            function,
            args,
            &PythonSandboxProfile::unsafe_all(),
        )
    }

    #[cfg(feature = "python")]
    pub fn call_python_function_with_policy(
        &self,
        module: &str,
        function: &str,
        args: Vec<serde_json::Value>,
        policy: &PythonSandboxProfile,
    ) -> Result<serde_json::Value, RuntimeError> {
        self.emit_python_event(
            "python.call",
            serde_json::json!({
                "module": module,
                "function": function,
                "args": args,
            }),
        );
        match PythonRuntime::new().call_function_with_policy(module, function, &args, policy) {
            Ok(value) => {
                self.emit_python_event(
                    "python.result",
                    serde_json::json!({
                        "module": module,
                        "function": function,
                        "result": value.clone(),
                    }),
                );
                Ok(value)
            }
            Err(err) => {
                self.emit_python_event(
                    "python.error",
                    serde_json::json!({
                        "module": module,
                        "function": function,
                        "error": err.to_string(),
                    }),
                );
                Err(err)
            }
        }
    }

    #[cfg(feature = "python")]
    fn emit_python_event(&self, name: &str, payload: serde_json::Value) {
        if !self.tracer.is_enabled() {
            return;
        }
        self.tracer.emit(TraceEvent::HostEvent {
            ts_ms: now_ms(),
            run_id: self.tracer.run_id().to_string(),
            name: name.to_string(),
            payload,
        });
    }

    fn emit_host_event(&self, name: &str, payload: serde_json::Value) {
        if !self.tracer.is_enabled() {
            return;
        }
        self.tracer.emit(TraceEvent::HostEvent {
            ts_ms: now_ms(),
            run_id: self.tracer.run_id().to_string(),
            name: name.to_string(),
            payload,
        });
    }
}

#[cfg(test)]
#[path = "tests/secrets_cache.rs"]
mod secrets_cache_tests;

#[cfg(test)]
#[path = "tests/io_http.rs"]
mod io_http_tests;

#[cfg(test)]
#[path = "tests/llm.rs"]
mod llm_tests;

#[cfg(test)]
#[path = "tests/approvals.rs"]
mod approvals_tests;

#[cfg(all(test, feature = "python"))]
#[path = "tests/python.rs"]
mod python_tests;

#[cfg(test)]
#[path = "tests/store.rs"]
mod store_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approvals::ProgrammaticApprover;
    use crate::llm::mock::MockAdapter;
    use serde_json::json;
    use std::sync::Arc;

    pub(super) fn rt() -> Runtime {
        Runtime::builder()
            .tool("double", |args| async move {
                let n = args[0].as_i64().unwrap();
                Ok(json!(n * 2))
            })
            .approver(Arc::new(ProgrammaticApprover::always_yes()))
            .llm(Arc::new(
                MockAdapter::new("mock-1").reply("greet", json!("hi")),
            ))
            .default_model("mock-1")
            .build()
    }

    #[test]
    fn queue_jobs_emit_lifecycle_trace_events() {
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("queue.jsonl");
        let r = Runtime::builder()
            .tracer(Tracer::open_path(&trace_path, "queue-run"))
            .build();

        let job = r
            .enqueue_job(
                "embed",
                json!({"doc": "a"}),
                2,
                0.5,
                Some("llm+io".to_string()),
                Some("trace:abc".to_string()),
            )
            .unwrap();
        let canceled = r.cancel_job(&job.id).unwrap();
        assert_eq!(canceled.status, crate::queue::QueueJobStatus::Canceled);

        let events = corvid_trace_schema::read_events_from_path(&trace_path).unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            TraceEvent::HostEvent { name, payload, .. }
                if name == "std.queue.enqueue"
                    && payload["task"] == "embed"
                    && payload["max_retries"] == 2
                    && payload["effect_summary"] == "llm+io"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            TraceEvent::HostEvent { name, payload, .. }
                if name == "std.queue.cancel"
                    && payload["id"] == job.id
                    && payload["status"] == "canceled"
        )));
    }

    #[test]
    fn explicit_runtime_catalog_enables_capability_selection() {
        let runtime = Runtime::builder()
            .model(
                RegisteredModel::new("cheap-basic")
                    .capability("basic")
                    .cost_per_token_in(0.000001)
                    .cost_per_token_out(0.000001),
            )
            .model(
                RegisteredModel::new("cheap-expert")
                    .capability("expert")
                    .cost_per_token_in(0.000002)
                    .cost_per_token_out(0.000001),
            )
            .build();

        let selected = runtime
            .select_cheapest_model_for_capability("expert", 100, 50)
            .unwrap();
        assert_eq!(selected.model, "cheap-expert");
        assert_eq!(selected.capability_picked.as_deref(), Some("expert"));
    }

    #[test]
    fn builder_can_load_model_catalog_from_project_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("corvid.toml"),
            r#"
[llm.models.haiku]
capability = "basic"
cost_per_token_in = 0.00000025
cost_per_token_out = 0.00000125

[llm.models.opus]
capability = "expert"
cost_per_token_in = 0.000015
"#,
        )
        .unwrap();

        let runtime = Runtime::builder().model_catalog_root(dir.path()).build();
        let selected = runtime
            .select_cheapest_model_for_capability("expert", 10, 10)
            .unwrap();
        assert_eq!(selected.model, "opus");
    }

    #[test]
    fn describe_named_model_uses_runtime_catalog_when_available() {
        let runtime = Runtime::builder()
            .model(
                RegisteredModel::new("fast")
                    .capability("basic")
                    .cost_per_token_in(0.25)
                    .cost_per_token_out(0.5),
            )
            .build();

        let selected = runtime.describe_named_model("fast", 2, 3).unwrap();
        assert_eq!(selected.model, "fast");
        assert_eq!(selected.capability_picked.as_deref(), Some("basic"));
        assert!((selected.cost_estimate - 2.0).abs() < 1e-12);
    }

    #[test]
    fn rollout_extremes_choose_expected_variant() {
        let runtime = Runtime::builder().rollout_seed(7).build();
        assert!(!runtime.choose_rollout_variant(0.0).unwrap());
        assert!(runtime.choose_rollout_variant(100.0).unwrap());
    }

    #[test]
    fn rollout_seed_produces_stable_sequence_across_restarts() {
        let runtime_a = Runtime::builder().rollout_seed(12345).build();
        let runtime_b = Runtime::builder().rollout_seed(12345).build();

        let sequence_a: Vec<bool> = (0..8)
            .map(|_| runtime_a.choose_rollout_variant(37.5).unwrap())
            .collect();
        let sequence_b: Vec<bool> = (0..8)
            .map(|_| runtime_b.choose_rollout_variant(37.5).unwrap())
            .collect();

        assert_eq!(sequence_a, sequence_b);
    }

    #[test]
    fn builder_defaults_to_interpreter_trace_writer() {
        let runtime = Runtime::builder().build();
        assert_eq!(runtime.tracer().run_id(), "null");
        assert!(runtime.recorder().is_none());
        let builder = RuntimeBuilder::default();
        assert_eq!(builder.trace_schema_writer, WRITER_INTERPRETER);
    }

    #[test]
    fn differential_replay_builder_exposes_swap_model() {
        let builder = Runtime::builder()
            .differential_replay_from("trace.jsonl", "mock-2")
            .replay_model_swap("mock-3");
        assert_eq!(builder.replay_trace, Some(PathBuf::from("trace.jsonl")));
        assert_eq!(builder.replay_model_swap.as_deref(), Some("mock-3"));
    }
}
