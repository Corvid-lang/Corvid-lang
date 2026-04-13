//! Mock LLM adapter. Used by tests and offline demos.
//!
//! Configured with a model name and a map of prompt-name → canned
//! response. Calls for unknown prompts return an `AdapterFailed` error so
//! tests don't silently pass on missing setup.

use crate::errors::RuntimeError;
use crate::llm::{LlmAdapter, LlmRequest, LlmResponse};
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
            Ok(LlmResponse { value })
        })
    }
}
