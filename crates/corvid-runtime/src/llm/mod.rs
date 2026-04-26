//! LLM adapter surface.
//!
//! An adapter is a strategy for turning a `LlmRequest` (model, rendered
//! prompt, optional structured schema) into an `LlmResponse` (a JSON
//! value matching the prompt's declared return type). Adapters are
//! registered into the runtime's `LlmRegistry`, dispatched by model
//! prefix.
//!
//! The runtime always ships a `MockAdapter` for tests and offline demos.
//! Provider-backed adapters are layered on top of the same interface.

pub mod anthropic;
pub mod gemini;
pub mod mock;
pub mod ollama;
pub mod openai;
pub mod openai_compat;

use crate::calibration::CalibrationObservation;
use crate::errors::RuntimeError;
use crate::tracing::now_ms;
use futures::future::BoxFuture;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;

/// Request handed to an adapter.
#[derive(Debug, Clone)]
pub struct LlmRequest {
    /// Name of the prompt declaration (used for tracing + mock keying).
    pub prompt: String,
    /// Model name from runtime config (e.g. `claude-opus-4-6`). May be
    /// empty if the caller relied on the adapter's default.
    pub model: String,
    /// Rendered prompt body — template with parameters substituted in.
    pub rendered: String,
    /// Free-form arguments passed to the prompt, marshalled to JSON.
    /// Adapters that support structured input use these directly; others
    /// can rely on `rendered` alone.
    pub args: Vec<serde_json::Value>,
    /// JSON Schema describing the expected response shape. Adapters use
    /// this to ask the model for structured output (Anthropic via
    /// `tool_use`, OpenAI via `response_format: {type: "json_schema"}`).
    /// `None` means the caller doesn't care about structure — the adapter
    /// returns whatever the model produced.
    pub output_schema: Option<serde_json::Value>,
}

/// Borrowed request shape for hot paths that already hold prompt/model/rendered
/// as borrowed strings and do not need to allocate an owned wrapper just to
/// cross the runtime boundary.
#[derive(Debug, Clone, Copy)]
pub struct LlmRequestRef<'a> {
    pub prompt: &'a str,
    pub model: &'a str,
    pub rendered: &'a str,
    pub args: &'a [serde_json::Value],
    pub output_schema: Option<&'a serde_json::Value>,
}

impl LlmRequest {
    pub fn as_ref(&self) -> LlmRequestRef<'_> {
        LlmRequestRef {
            prompt: &self.prompt,
            model: &self.model,
            rendered: &self.rendered,
            args: &self.args,
            output_schema: self.output_schema.as_ref(),
        }
    }
}

impl<'a> LlmRequestRef<'a> {
    pub fn with_model(self, model: &'a str) -> Self {
        Self { model, ..self }
    }

    pub fn prompt_cow(&self) -> Cow<'a, str> {
        Cow::Borrowed(self.prompt)
    }

    pub fn model_cow(&self) -> Cow<'a, str> {
        Cow::Borrowed(self.model)
    }

    pub fn rendered_cow(&self) -> Cow<'a, str> {
        Cow::Borrowed(self.rendered)
    }
}

/// Response returned by an adapter. The `value` is the JSON shape that
/// the interpreter will marshal back into the prompt's declared return
/// type. For string-returning prompts this is a JSON string; for struct
/// returns it's a JSON object whose keys match the declared fields.
///
/// `usage` records the token counts the provider reports. The runtime
/// aggregates these per process and can feed higher-level budgeting
/// features later. Adapters that
/// don't have token info from the provider report zeros.
#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub value: serde_json::Value,
    pub usage: TokenUsage,
    pub confidence: Option<f64>,
    pub calibration: Option<CalibrationObservation>,
}

impl LlmResponse {
    pub fn new(value: serde_json::Value, usage: TokenUsage) -> Self {
        Self {
            value,
            usage,
            confidence: None,
            calibration: None,
        }
    }

    pub fn with_confidence(
        value: serde_json::Value,
        usage: TokenUsage,
        confidence: f64,
    ) -> Self {
        Self {
            value,
            usage,
            confidence: Some(confidence.clamp(0.0, 1.0)),
            calibration: None,
        }
    }

    pub fn with_calibration(
        value: serde_json::Value,
        usage: TokenUsage,
        actual_correct: bool,
    ) -> Self {
        Self {
            value,
            usage,
            confidence: None,
            calibration: Some(CalibrationObservation { actual_correct }),
        }
    }
}

/// Token counts for a single LLM call. Filled in by adapters from the
/// provider's response (every major API returns these). Zeros mean
/// "the provider didn't report" (e.g., older endpoints, mocks).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderHealth {
    pub adapter: String,
    pub consecutive_failures: u64,
    pub last_success_ms: Option<u64>,
    pub last_failure_ms: Option<u64>,
    pub degraded: bool,
}

impl ProviderHealth {
    fn new(adapter: impl Into<String>) -> Self {
        Self {
            adapter: adapter.into(),
            consecutive_failures: 0,
            last_success_ms: None,
            last_failure_ms: None,
            degraded: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LlmCallOutcome {
    pub adapter: String,
    pub response: LlmResponse,
}

/// Trait every LLM adapter implements.
pub trait LlmAdapter: Send + Sync {
    /// Adapter identifier used for diagnostics and tracing.
    fn name(&self) -> &str;

    /// Whether this adapter handles `model`. The registry uses this to
    /// dispatch — first match wins, in registration order.
    fn handles(&self, model: &str) -> bool;

    /// Perform the call. Implementations may take however long they need;
    /// the interpreter awaits.
    fn call<'a>(
        &'a self,
        req: &'a LlmRequestRef<'a>,
    ) -> BoxFuture<'a, Result<LlmResponse, RuntimeError>>;
}

/// Registry of LLM adapters. Cheap to clone.
#[derive(Clone, Default)]
pub struct LlmRegistry {
    adapters: Vec<Arc<dyn LlmAdapter>>,
    health: Arc<Mutex<BTreeMap<String, ProviderHealth>>>,
}

impl LlmRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, adapter: Arc<dyn LlmAdapter>) {
        self.adapters.push(adapter);
    }

    /// Dispatch `req` to the first adapter whose `handles` returns true.
    pub async fn call(&self, req: &LlmRequestRef<'_>) -> Result<LlmResponse, RuntimeError> {
        Ok(self.call_with_adapter_name(req).await?.response)
    }

    pub async fn call_with_adapter_name(
        &self,
        req: &LlmRequestRef<'_>,
    ) -> Result<LlmCallOutcome, RuntimeError> {
        let model = if req.model.is_empty() {
            return Err(RuntimeError::NoModelConfigured);
        } else {
            req.model
        };
        let adapter = self
            .adapters
            .iter()
            .find(|a| a.handles(model))
            .ok_or_else(|| RuntimeError::NoAdapter(model.to_string()))?
            .clone();
        let adapter_name = adapter.name().to_string();
        match adapter.call(req).await {
            Ok(response) => {
                self.record_success(&adapter_name);
                Ok(LlmCallOutcome {
                    adapter: adapter_name,
                    response,
                })
            }
            Err(err) => {
                self.record_failure(&adapter_name);
                Err(err)
            }
        }
    }

    /// Names of all registered adapters, in registration order.
    pub fn names(&self) -> Vec<String> {
        self.adapters.iter().map(|a| a.name().to_string()).collect()
    }

    pub fn health(&self) -> Vec<ProviderHealth> {
        let health = self.health.lock().unwrap();
        self.adapters
            .iter()
            .map(|adapter| {
                health
                    .get(adapter.name())
                    .cloned()
                    .unwrap_or_else(|| ProviderHealth::new(adapter.name()))
            })
            .collect()
    }

    fn record_success(&self, adapter: &str) {
        let mut health = self.health.lock().unwrap();
        let entry = health
            .entry(adapter.to_string())
            .or_insert_with(|| ProviderHealth::new(adapter));
        entry.consecutive_failures = 0;
        entry.last_success_ms = Some(now_ms());
        entry.degraded = false;
    }

    fn record_failure(&self, adapter: &str) {
        let mut health = self.health.lock().unwrap();
        let entry = health
            .entry(adapter.to_string())
            .or_insert_with(|| ProviderHealth::new(adapter));
        entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
        entry.last_failure_ms = Some(now_ms());
        entry.degraded = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::mock::MockAdapter;

    #[tokio::test]
    async fn mock_adapter_returns_canned_response() {
        let mut reg = LlmRegistry::new();
        reg.register(Arc::new(MockAdapter::new("mock-1").reply(
            "decide_refund",
            serde_json::json!({"should_refund": true}),
        )));
        let req = LlmRequest {
            prompt: "decide_refund".into(),
            model: "mock-1".into(),
            rendered: "Decide whether to refund.".into(),
            args: vec![],
            output_schema: None,
        };
        let resp = reg.call(&req.as_ref()).await.unwrap();
        assert_eq!(resp.value, serde_json::json!({"should_refund": true}));
    }

    #[tokio::test]
    async fn missing_adapter_errors() {
        let reg = LlmRegistry::new();
        let req = LlmRequest {
            prompt: "x".into(),
            model: "claude-opus-4-6".into(),
            rendered: "".into(),
            args: vec![],
            output_schema: None,
        };
        let err = reg.call(&req.as_ref()).await.unwrap_err();
        assert!(matches!(err, RuntimeError::NoAdapter(ref m) if m == "claude-opus-4-6"));
    }

    #[tokio::test]
    async fn registry_records_adapter_health() {
        let mut reg = LlmRegistry::new();
        reg.register(Arc::new(
            MockAdapter::new("mock-1").reply("ok", serde_json::json!("yes")),
        ));
        let missing = LlmRequest {
            prompt: "missing".into(),
            model: "mock-1".into(),
            rendered: "".into(),
            args: vec![],
            output_schema: None,
        };
        let err = reg.call(&missing.as_ref()).await.unwrap_err();
        assert!(matches!(err, RuntimeError::AdapterFailed { .. }));
        let health = reg.health();
        assert_eq!(health[0].adapter, "mock-1");
        assert_eq!(health[0].consecutive_failures, 1);
        assert!(health[0].degraded);

        let ok = LlmRequest {
            prompt: "ok".into(),
            model: "mock-1".into(),
            rendered: "".into(),
            args: vec![],
            output_schema: None,
        };
        reg.call(&ok.as_ref()).await.unwrap();
        let health = reg.health();
        assert_eq!(health[0].consecutive_failures, 0);
        assert!(!health[0].degraded);
        assert!(health[0].last_success_ms.is_some());
    }

    #[tokio::test]
    async fn empty_model_errors() {
        let reg = LlmRegistry::new();
        let req = LlmRequest {
            prompt: "x".into(),
            model: String::new(),
            rendered: "".into(),
            args: vec![],
            output_schema: None,
        };
        let err = reg.call(&req.as_ref()).await.unwrap_err();
        assert!(matches!(err, RuntimeError::NoModelConfigured));
    }
}
