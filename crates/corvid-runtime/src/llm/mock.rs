//! Mock LLM adapter. Used by tests and offline demos.
//!
//! Configured with a model name and a map of prompt-name to canned
//! response. Calls for unknown prompts return an `AdapterFailed` error so
//! tests do not silently pass on missing setup.

use crate::errors::RuntimeError;
use crate::llm::{LlmAdapter, LlmRequest, LlmResponse, TokenUsage};
use futures::future::BoxFuture;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::Instant;

fn profile_enabled() -> bool {
    std::env::var("CORVID_PROFILE_EVENTS").ok().as_deref() == Some("1")
}

fn emit_wait_profile(kind: &str, name: &str, nominal_ms: u64, actual_ms: f64) {
    if !profile_enabled() {
        return;
    }
    let event = serde_json::json!({
        "kind": "wait",
        "source_kind": kind,
        "name": name,
        "nominal_ms": nominal_ms,
        "actual_ms": actual_ms,
    });
    eprintln!("CORVID_PROFILE_JSON={event}");
}

pub struct MockAdapter {
    name: String,
    /// Prompt name to canned JSON. Behind a Mutex so `reply` can be called
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

/// Test-mode mock configured entirely from env vars.
///
/// Legacy mode:
/// - `CORVID_TEST_MOCK_LLM_RESPONSE`: one fallback response for every prompt
///
/// Native benchmark mode:
/// - `CORVID_TEST_MOCK_LLM_REPLIES`: JSON object mapping prompt name to a
///   JSON value or an array of JSON values consumed in FIFO order
/// - `CORVID_TEST_MOCK_LLM_LATENCY_MS`: JSON object mapping prompt name to
///   a sleep duration in milliseconds
pub struct EnvVarMockAdapter {
    name: String,
    fallback: serde_json::Value,
    replies: Mutex<HashMap<String, VecDeque<serde_json::Value>>>,
    latencies_ms: Mutex<HashMap<String, VecDeque<u64>>>,
}

impl EnvVarMockAdapter {
    pub fn from_env() -> Self {
        let raw = std::env::var("CORVID_TEST_MOCK_LLM_RESPONSE").unwrap_or_default();
        let replies = std::env::var("CORVID_TEST_MOCK_LLM_REPLIES")
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&raw).ok())
            .map(|map| {
                map.into_iter()
                    .map(|(prompt, value)| {
                        let queue = match value {
                            serde_json::Value::Array(values) => values.into_iter().collect(),
                            other => VecDeque::from([other]),
                        };
                        (prompt, queue)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let latencies_ms = std::env::var("CORVID_TEST_MOCK_LLM_LATENCY_MS")
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&raw).ok())
            .map(|map| {
                map.into_iter()
                    .map(|(prompt, value)| {
                        let queue = match value {
                            serde_json::Value::Array(values) => values
                                .into_iter()
                                .filter_map(|v| v.as_u64())
                                .collect(),
                            other => other
                                .as_u64()
                                .map(|v| VecDeque::from([v]))
                                .unwrap_or_default(),
                        };
                        (prompt, queue)
                    })
                    .collect()
            })
            .unwrap_or_default();

        Self {
            name: "env-mock-llm".into(),
            fallback: serde_json::Value::String(raw),
            replies: Mutex::new(replies),
            latencies_ms: Mutex::new(latencies_ms),
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
        // providers in test mode.
        true
    }

    fn call<'a>(
        &'a self,
        req: &'a LlmRequest,
    ) -> BoxFuture<'a, Result<LlmResponse, RuntimeError>> {
        let prompt = req.prompt.clone();
        let value = {
            let mut replies = self.replies.lock().unwrap();
            replies
                .get_mut(&prompt)
                .and_then(|queue| queue.pop_front())
                .unwrap_or_else(|| self.fallback.clone())
        };
        let latency_ms = {
            let mut latencies = self.latencies_ms.lock().unwrap();
            latencies
                .get_mut(&prompt)
                .and_then(|queue| queue.pop_front())
                .unwrap_or(0)
        };

        Box::pin(async move {
            if latency_ms > 0 {
                let start = Instant::now();
                tokio::time::sleep(std::time::Duration::from_millis(latency_ms)).await;
                let actual_ms = start.elapsed().as_secs_f64() * 1000.0;
                emit_wait_profile(
                    "prompt",
                    &prompt,
                    latency_ms,
                    actual_ms,
                );
            }
            Ok(LlmResponse {
                value,
                usage: TokenUsage::default(),
            })
        })
    }
}
