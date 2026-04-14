//! Mock LLM adapter. Used by tests and offline demos.
//!
//! Configured with a model name and a map of prompt-name → canned
//! response. Calls for unknown prompts return an `AdapterFailed` error so
//! tests don't silently pass on missing setup.

use crate::errors::RuntimeError;
use crate::llm::{LlmAdapter, LlmRequest, LlmResponse, TokenUsage};
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct MockAdapter {
    name: String,
    /// Prompt name → canned JSON. Behind a Mutex so `reply` can be called
    /// after the adapter has been wrapped in `Arc`.
    replies: Mutex<HashMap<String, serde_json::Value>>,
}

impl MockAdapter {
    pub fn new(model_name: impl Into<String>) -> Self {
        Self {
            name: model_name.into(),
            replies: Mutex::new(HashMap::new()),
        }
    }

    /// Builder-style: register a canned response for `prompt`.
    pub fn reply(self, prompt: impl Into<String>, value: serde_json::Value) -> Self {
        self.replies.lock().unwrap().insert(prompt.into(), value);
        self
    }

    /// Mutating variant for use after the adapter is shared.
    pub fn add_reply(&self, prompt: impl Into<String>, value: serde_json::Value) {
        self.replies.lock().unwrap().insert(prompt.into(), value);
    }
}

impl LlmAdapter for MockAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn handles(&self, model: &str) -> bool {
        model == self.name
    }

    fn call<'a>(
        &'a self,
        req: &'a LlmRequest,
    ) -> BoxFuture<'a, Result<LlmResponse, RuntimeError>> {
        Box::pin(async move {
            let value = self
                .replies
                .lock()
                .unwrap()
                .get(&req.prompt)
                .cloned()
                .ok_or_else(|| RuntimeError::AdapterFailed {
                    adapter: self.name.clone(),
                    message: format!(
                        "no canned reply registered for prompt `{}`",
                        req.prompt
                    ),
                })?;
            Ok(LlmResponse {
                value,
                // Mocks have no real token counts. Zeros are the
                // documented "no usage info" sentinel.
                usage: TokenUsage::default(),
            })
        })
    }
}

/// Test-mode mock that returns the same response for every prompt
/// call. Configured from a single env var by the runtime's bridge
/// when `CORVID_TEST_MOCK_LLM=1`. Useful for parity fixtures that
/// exercise the prompt-dispatch path without needing per-prompt
/// canned data.
pub struct EnvVarMockAdapter {
    name: String,
    response: serde_json::Value,
}

impl EnvVarMockAdapter {
    /// Build from the env. Reads `CORVID_TEST_MOCK_LLM_RESPONSE` for
    /// the canned response (raw string — adapter wraps it in a
    /// `serde_json::Value::String` so it round-trips through the
    /// String path; numeric responses for Int/Bool/Float prompts go
    /// in the same env var as their textual representation, e.g.
    /// `"42"` or `"true"`).
    pub fn from_env() -> Self {
        let raw = std::env::var("CORVID_TEST_MOCK_LLM_RESPONSE").unwrap_or_default();
        Self {
            name: "env-mock-llm".into(),
            response: serde_json::Value::String(raw),
        }
    }
}

impl LlmAdapter for EnvVarMockAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn handles(&self, _model: &str) -> bool {
        // Wildcard match — once registered, this adapter handles
        // every model spec. The bridge only registers it when
        // CORVID_TEST_MOCK_LLM=1, and the registry's first-match
        // dispatch means the env mock takes precedence over real
        // providers in test mode (intentional: avoids real API
        // calls in CI even when API keys leak into the env).
        true
    }

    fn call<'a>(
        &'a self,
        _req: &'a LlmRequest,
    ) -> BoxFuture<'a, Result<LlmResponse, RuntimeError>> {
        let value = self.response.clone();
        Box::pin(async move {
            Ok(LlmResponse {
                value,
                usage: TokenUsage::default(),
            })
        })
    }
}
