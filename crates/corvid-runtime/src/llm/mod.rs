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

use crate::errors::RuntimeError;
use futures::future::BoxFuture;
use std::sync::Arc;

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
        req: &'a LlmRequest,
    ) -> BoxFuture<'a, Result<LlmResponse, RuntimeError>>;
}

/// Registry of LLM adapters. Cheap to clone.
#[derive(Clone, Default)]
pub struct LlmRegistry {
    adapters: Vec<Arc<dyn LlmAdapter>>,
}

impl LlmRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, adapter: Arc<dyn LlmAdapter>) {
        self.adapters.push(adapter);
    }

    /// Dispatch `req` to the first adapter whose `handles` returns true.
    pub async fn call(&self, req: &LlmRequest) -> Result<LlmResponse, RuntimeError> {
        let model = if req.model.is_empty() {
            return Err(RuntimeError::NoModelConfigured);
        } else {
            req.model.as_str()
        };
        let adapter = self
            .adapters
            .iter()
            .find(|a| a.handles(model))
            .ok_or_else(|| RuntimeError::NoAdapter(model.to_string()))?
            .clone();
        adapter.call(req).await
    }

    /// Names of all registered adapters, in registration order.
    pub fn names(&self) -> Vec<String> {
        self.adapters.iter().map(|a| a.name().to_string()).collect()
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
        let resp = reg.call(&req).await.unwrap();
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
        let err = reg.call(&req).await.unwrap_err();
        assert!(matches!(err, RuntimeError::NoAdapter(ref m) if m == "claude-opus-4-6"));
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
        let err = reg.call(&req).await.unwrap_err();
        assert!(matches!(err, RuntimeError::NoModelConfigured));
    }
}
