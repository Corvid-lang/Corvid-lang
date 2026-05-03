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
#[cfg(test)]
use crate::llm::LlmRequest;
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

#[cfg(all(test, feature = "python"))]
#[path = "tests/python.rs"]
mod python_tests;

#[cfg(test)]
#[path = "tests/store.rs"]
mod store_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approvals::{ApprovalTokenScope, ProgrammaticApprover};
    use crate::llm::mock::MockAdapter;
    use serde_json::json;

    fn rt() -> Runtime {
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

    #[tokio::test]
    async fn call_tool_routes_through_registry() {
        let r = rt();
        let v = r.call_tool("double", vec![json!(5)]).await.unwrap();
        assert_eq!(v, json!(10));
    }

    #[tokio::test]
    async fn approval_gate_passes_when_approver_says_yes() {
        let r = rt();
        r.approval_gate("Anything", vec![]).await.unwrap();
    }

    #[tokio::test]
    async fn approval_gate_emits_scoped_token_for_approved_request() {
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("approval.jsonl");
        let r = Runtime::builder()
            .approver(Arc::new(ProgrammaticApprover::always_yes()))
            .tracer(Tracer::open_path(&trace_path, "approval-run"))
            .build();

        r.approval_gate("IssueRefund", vec![json!("ord_1"), json!(12.5)])
            .await
            .unwrap();

        let events = corvid_trace_schema::read_events_from_path(&trace_path).unwrap();
        let token = events
            .iter()
            .find_map(|event| match event {
                TraceEvent::ApprovalTokenIssued {
                    token_id,
                    label,
                    args,
                    scope,
                    issued_at_ms,
                    expires_at_ms,
                    ..
                } => Some((token_id, label, args, scope, *issued_at_ms, *expires_at_ms)),
                _ => None,
            })
            .expect("approval token event");

        assert!(token.0.starts_with("apr_"));
        assert_eq!(token.0.len(), 68);
        assert_eq!(token.1, "IssueRefund");
        assert_eq!(token.2, &vec![json!("ord_1"), json!(12.5)]);
        assert_eq!(token.3, "one_time");
        assert_eq!(token.5 - token.4, APPROVAL_TOKEN_TTL_MS);
    }

    #[test]
    fn approval_scope_violation_is_trace_visible() {
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("scope.jsonl");
        let r = Runtime::builder()
            .tracer(Tracer::open_path(&trace_path, "scope-run"))
            .build();
        let mut token = ApprovalToken {
            token_id: "apr_limit".into(),
            label: "ChargeCard".into(),
            args: vec![json!(100.0)],
            scope: ApprovalTokenScope::AmountLimited { max_amount: 100.0 },
            issued_at_ms: 0,
            expires_at_ms: u64::MAX,
            uses_remaining: 1,
        };

        let err = r
            .validate_approval_token_scope(&mut token, "ChargeCard", &[json!(125.0)], None)
            .unwrap_err();
        assert!(matches!(err, RuntimeError::ApprovalFailed(_)));

        let events = corvid_trace_schema::read_events_from_path(&trace_path).unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            TraceEvent::ApprovalScopeViolation {
                token_id,
                label,
                reason,
                ..
            } if token_id == "apr_limit"
                && label == "ChargeCard"
                && reason.contains("exceeds token limit")
        )));
    }

    #[tokio::test]
    async fn approval_gate_blocks_when_approver_says_no() {
        let r = Runtime::builder()
            .approver(Arc::new(ProgrammaticApprover::always_no()))
            .build();
        let err = r.approval_gate("IssueRefund", vec![]).await.unwrap_err();
        assert!(matches!(
            err,
            RuntimeError::ApprovalDenied { ref action } if action == "IssueRefund"
        ));
    }

    #[tokio::test]
    async fn call_llm_uses_default_model_when_request_blank() {
        let r = rt();
        let resp = r
            .call_llm(LlmRequest {
                prompt: "greet".into(),
                model: String::new(),
                rendered: "say hi".into(),
                args: vec![],
                output_schema: None,
            })
            .await
            .unwrap();
        assert_eq!(resp.value, json!("hi"));
    }

    #[tokio::test]
    async fn call_llm_fails_over_to_compatible_model_and_traces_provider_events() {
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("failover.jsonl");
        let r = Runtime::builder()
            .tracer(Tracer::open_path(&trace_path, "failover-run"))
            .llm(Arc::new(MockAdapter::new("primary")))
            .llm(Arc::new(
                MockAdapter::new("fallback").reply("greet", json!("from fallback")),
            ))
            .default_model("primary")
            .model(
                RegisteredModel::new("primary")
                    .provider("openai")
                    .capability("standard")
                    .output_format("strict_json")
                    .privacy_tier("hosted")
                    .jurisdiction("US")
                    .structured_output(true)
                    .cost_per_token_in(0.000002),
            )
            .model(
                RegisteredModel::new("fallback")
                    .provider("anthropic")
                    .capability("expert")
                    .output_format("strict_json")
                    .privacy_tier("hosted")
                    .jurisdiction("US")
                    .structured_output(true)
                    .cost_per_token_in(0.000001),
            )
            .build();

        let resp = r
            .call_llm(LlmRequest {
                prompt: "greet".into(),
                model: String::new(),
                rendered: "say hi".into(),
                args: vec![],
                output_schema: None,
            })
            .await
            .unwrap();
        assert_eq!(resp.value, json!("from fallback"));

        let health = r.provider_health();
        let primary = health
            .iter()
            .find(|entry| entry.adapter == "primary")
            .expect("primary health");
        assert_eq!(primary.consecutive_failures, 1);
        assert!(primary.degraded);

        let events = corvid_trace_schema::read_events_from_path(&trace_path).unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            TraceEvent::HostEvent { name, payload, .. }
                if name == "llm.provider_degraded" && payload["model"] == "primary"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            TraceEvent::HostEvent { name, payload, .. }
                if name == "llm.provider_failover"
                    && payload["from_model"] == "primary"
                    && payload["to_model"] == "fallback"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            TraceEvent::LlmResult { model, result, .. }
                if model.as_deref() == Some("fallback") && result == &json!("from fallback")
        )));
    }

    #[tokio::test]
    async fn call_llm_records_normalized_usage_by_provider() {
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("usage.jsonl");
        let r = Runtime::builder()
            .tracer(Tracer::open_path(&trace_path, "usage-run"))
            .llm(Arc::new(MockAdapter::new("gpt").reply_with_usage(
                "greet",
                json!("hi"),
                crate::llm::TokenUsage {
                    prompt_tokens: 10,
                    completion_tokens: 4,
                    total_tokens: 0,
                },
            )))
            .default_model("gpt")
            .model(
                RegisteredModel::new("gpt")
                    .provider("openai")
                    .privacy_tier("hosted")
                    .cost_per_token_in(0.01)
                    .cost_per_token_out(0.02),
            )
            .build();

        let resp = r
            .call_llm(LlmRequest {
                prompt: "greet".into(),
                model: String::new(),
                rendered: "say hi".into(),
                args: vec![],
                output_schema: None,
            })
            .await
            .unwrap();
        assert_eq!(resp.value, json!("hi"));

        let records = r.llm_usage_records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].provider.as_deref(), Some("openai"));
        assert_eq!(records[0].adapter.as_deref(), Some("gpt"));
        assert_eq!(records[0].total_tokens, 14);
        assert!((records[0].cost_usd - 0.18).abs() < 1e-12);

        let totals = r.llm_usage_totals_by_provider();
        assert_eq!(totals["openai"].calls, 1);
        assert_eq!(totals["openai"].total_tokens, 14);
        assert!((totals["openai"].cost_usd - 0.18).abs() < 1e-12);

        let events = corvid_trace_schema::read_events_from_path(&trace_path).unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            TraceEvent::HostEvent { name, payload, .. }
                if name == "llm.usage"
                    && payload["provider"] == "openai"
                    && payload["total_tokens"] == 14
                    && payload["currency"] == "USD"
        )));
    }

    #[tokio::test]
    async fn observation_summary_aggregates_usage_and_provider_health() {
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("observe.jsonl");
        let r = Runtime::builder()
            .tracer(Tracer::open_path(&trace_path, "observe-run"))
            .llm(Arc::new(MockAdapter::new("gpt").reply_with_usage(
                "summarize",
                json!("ok"),
                crate::llm::TokenUsage {
                    prompt_tokens: 8,
                    completion_tokens: 4,
                    total_tokens: 12,
                },
            )))
            .model(
                RegisteredModel::new("gpt")
                    .provider("openai")
                    .privacy_tier("hosted")
                    .cost_per_token_in(0.001)
                    .cost_per_token_out(0.002),
            )
            .build();

        r.call_llm(LlmRequest {
            prompt: "summarize".into(),
            model: "gpt".into(),
            rendered: "Summarize.".into(),
            args: vec![],
            output_schema: None,
        })
        .await
        .unwrap();

        let summary = r.emit_observation_summary();
        assert_eq!(summary.llm_calls, 1);
        assert_eq!(summary.local_llm_calls, 0);
        assert_eq!(summary.total_tokens, 12);
        assert_eq!(summary.cost_usd, 0.016);
        assert_eq!(summary.provider_count, 1);
        assert_eq!(summary.degraded_provider_count, 0);

        let events = corvid_trace_schema::read_events_from_path(&trace_path).unwrap();
        let event = events
            .iter()
            .find_map(|event| match event {
                TraceEvent::HostEvent { name, payload, .. } if name == "std.observe.summary" => {
                    Some(payload)
                }
                _ => None,
            })
            .expect("std.observe summary event");
        assert_eq!(event["llm_calls"], json!(1));
        assert_eq!(event["total_tokens"], json!(12));
        assert_eq!(event["provider_count"], json!(1));
        assert_eq!(event["degraded_provider_count"], json!(0));
    }

    #[tokio::test]
    async fn http_request_emits_trace_events() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/status"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("http.jsonl");
        let r = Runtime::builder()
            .tracer(Tracer::open_path(&trace_path, "http-run"))
            .build();

        let response = r
            .http_request(crate::http::HttpRequest::get(format!(
                "{}/status",
                server.uri()
            )))
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "ok");

        let events = corvid_trace_schema::read_events_from_path(&trace_path).unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            TraceEvent::HostEvent { name, payload, .. }
                if name == "std.http.request" && payload["method"] == "GET"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            TraceEvent::HostEvent { name, payload, .. }
                if name == "std.http.response" && payload["status"] == 200
        )));
    }

    #[tokio::test]
    async fn file_io_emits_trace_events() {
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("io.jsonl");
        let file_path = dir.path().join("data").join("note.txt");
        let r = Runtime::builder()
            .tracer(Tracer::open_path(&trace_path, "io-run"))
            .build();

        let write = r.write_text_file(&file_path, "hello").await.unwrap();
        assert_eq!(write.bytes, 5);
        let read = r.read_text_file(&file_path).await.unwrap();
        assert_eq!(read.contents, "hello");
        let entries = r.list_dir(file_path.parent().unwrap()).await.unwrap();
        assert_eq!(entries.len(), 1);

        let events = corvid_trace_schema::read_events_from_path(&trace_path).unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            TraceEvent::HostEvent { name, payload, .. }
                if name == "std.io.write.result" && payload["bytes"] == 5
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            TraceEvent::HostEvent { name, payload, .. }
                if name == "std.io.read.result" && payload["bytes"] == 5
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            TraceEvent::HostEvent { name, payload, .. }
                if name == "std.io.list.result" && payload["entries"] == 1
        )));
    }

    #[test]
    fn secret_reads_are_trace_visible_without_secret_value() {
        std::env::set_var("CORVID_TEST_RUNTIME_SECRET", "super-secret");
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("secret.jsonl");
        let r = Runtime::builder()
            .tracer(Tracer::open_path(&trace_path, "secret-run"))
            .build();

        let read = r.read_env_secret("CORVID_TEST_RUNTIME_SECRET").unwrap();
        assert!(read.present);
        assert_eq!(read.value.as_deref(), Some("super-secret"));

        let events = corvid_trace_schema::read_events_from_path(&trace_path).unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            TraceEvent::HostEvent { name, payload, .. }
                if name == "std.secrets.read"
                    && payload["name"] == "CORVID_TEST_RUNTIME_SECRET"
                    && payload["present"] == true
                    && payload.get("value").is_none()
        )));
    }

    #[test]
    fn cache_keys_are_trace_visible_without_cached_payloads() {
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("cache.jsonl");
        let r = Runtime::builder()
            .tracer(Tracer::open_path(&trace_path, "cache-run"))
            .build();

        let key = r
            .cache_key(CacheKeyInput {
                namespace: "tool".to_string(),
                subject: "lookup".to_string(),
                model: None,
                effect_key: Some("io:read".to_string()),
                provenance_key: Some("doc:123".to_string()),
                version: Some("v1".to_string()),
                args: json!({"id": 7}),
            })
            .unwrap();

        assert_eq!(key.namespace, "tool");
        assert_eq!(key.subject, "lookup");
        assert_eq!(key.fingerprint.len(), 64);

        let events = corvid_trace_schema::read_events_from_path(&trace_path).unwrap();
        let event = events
            .iter()
            .find_map(|event| match event {
                TraceEvent::HostEvent { name, payload, .. } if name == "std.cache.key" => {
                    Some(payload)
                }
                _ => None,
            })
            .expect("std.cache key event");
        assert_eq!(event["namespace"], json!("tool"));
        assert_eq!(event["subject"], json!("lookup"));
        assert_eq!(event["effect_key"], json!("io:read"));
        assert_eq!(event["provenance_key"], json!("doc:123"));
        assert_eq!(event.get("value"), None);
        assert_eq!(event.get("payload"), None);
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
