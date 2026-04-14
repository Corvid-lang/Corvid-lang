//! Ollama adapter — local-first LLM inference.
//!
//! Speaks `POST /api/chat` against the local Ollama server (default
//! `http://localhost:11434`). Phase 15's local-first story: users
//! running Ollama get full Corvid functionality with no API key, no
//! network egress, no per-token cost. Critical for privacy-sensitive
//! deployments and offline development.
//!
//! Model spec convention: `ollama:<model-name>` in `CORVID_MODEL`. The
//! `ollama:` prefix routes to this adapter; `<model-name>` is what
//! Ollama itself sees (e.g. `llama3.2`, `qwen2.5-coder:7b`,
//! `mistral-nemo`). Override the server URL with `OLLAMA_BASE_URL` if
//! you're running a non-default Ollama instance.
//!
//! Structured output: Ollama's chat API supports `format: "json"` and
//! more recently a `format: <json-schema>` field for stricter output.
//! Phase 15 uses the schema field when an `output_schema` is provided
//! (matches OpenAI's structured-output behaviour); falls back to
//! plain text completion otherwise.
//!
//! Out of scope: streaming responses, multi-modal inputs, custom
//! sampling parameters. Phase 20 + 31 territory.

use crate::errors::RuntimeError;
use crate::llm::{LlmAdapter, LlmRequest, LlmResponse, TokenUsage};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::time::Duration;

pub const OLLAMA_DEFAULT_BASE: &str = "http://localhost:11434";
pub const OLLAMA_MODEL_PREFIX: &str = "ollama:";

pub struct OllamaAdapter {
    base_url: String,
    client: reqwest::Client,
}

impl OllamaAdapter {
    /// Build with the default base URL (`http://localhost:11434`).
    /// Override via env: `OLLAMA_BASE_URL=http://my-ollama:11434`.
    pub fn new() -> Self {
        let base = std::env::var("OLLAMA_BASE_URL")
            .unwrap_or_else(|_| OLLAMA_DEFAULT_BASE.to_string());
        Self {
            base_url: base,
            // Long timeout — local models on CPU can take 30s+ for
            // a single response on modest hardware.
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(180))
                .build()
                .expect("reqwest client builds with default config"),
        }
    }

    pub fn with_base_url(mut self, base: impl Into<String>) -> Self {
        self.base_url = base.into();
        self
    }
}

impl Default for OllamaAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmAdapter for OllamaAdapter {
    fn name(&self) -> &str {
        "ollama"
    }

    fn handles(&self, model: &str) -> bool {
        model.starts_with(OLLAMA_MODEL_PREFIX)
    }

    fn call<'a>(
        &'a self,
        req: &'a LlmRequest,
    ) -> BoxFuture<'a, Result<LlmResponse, RuntimeError>> {
        Box::pin(async move {
            let model_name = req
                .model
                .strip_prefix(OLLAMA_MODEL_PREFIX)
                .unwrap_or(&req.model);
            let url = format!("{}/api/chat", self.base_url.trim_end_matches('/'));

            let mut body = json!({
                "model": model_name,
                // `stream: false` returns the full response in one
                // JSON object; streaming is Phase 20 work.
                "stream": false,
                "messages": [
                    {"role": "user", "content": req.rendered}
                ],
            });
            if let Some(schema) = &req.output_schema {
                // Newer Ollama (≥ 0.5.x) accepts a JSON schema
                // directly in `format`. Older versions accepted
                // only `format: "json"`. We send the schema; Ollama
                // ignores unknown fields gracefully on older
                // versions, falling through to JSON-mode behaviour.
                body["format"] = schema.clone();
            }

            let resp = self
                .client
                .post(&url)
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| RuntimeError::AdapterFailed {
                    adapter: "ollama".into(),
                    message: format!(
                        "HTTP send failed (is Ollama running at {}?): {e}",
                        self.base_url
                    ),
                })?;

            let status = resp.status();
            let body_text = resp.text().await.map_err(|e| RuntimeError::AdapterFailed {
                adapter: "ollama".into(),
                message: format!("reading response body failed: {e}"),
            })?;
            if !status.is_success() {
                return Err(RuntimeError::AdapterFailed {
                    adapter: "ollama".into(),
                    message: format!("HTTP {status}: {body_text}"),
                });
            }

            let parsed: Value = serde_json::from_str(&body_text).map_err(|e| {
                RuntimeError::AdapterFailed {
                    adapter: "ollama".into(),
                    message: format!("response body is not JSON: {e} (body: {body_text})"),
                }
            })?;
            extract_response(&parsed, req.output_schema.is_some())
        })
    }
}

fn extract_response(
    parsed: &Value,
    expect_structured: bool,
) -> Result<LlmResponse, RuntimeError> {
    // `/api/chat` non-streaming shape:
    //   { "message": { "role": "assistant", "content": "..." },
    //     "prompt_eval_count": N, "eval_count": N, ... }
    let content = parsed
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| RuntimeError::AdapterFailed {
            adapter: "ollama".into(),
            message: "response missing `message.content`".into(),
        })?;

    let usage = TokenUsage {
        prompt_tokens: parsed
            .get("prompt_eval_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        completion_tokens: parsed
            .get("eval_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        total_tokens: 0, // filled below
    };
    let usage = TokenUsage {
        total_tokens: usage.prompt_tokens + usage.completion_tokens,
        ..usage
    };

    if expect_structured {
        let value: Value = serde_json::from_str(content).map_err(|e| {
            RuntimeError::AdapterFailed {
                adapter: "ollama".into(),
                message: format!(
                    "structured response was not valid JSON: {e} (content: {content})"
                ),
            }
        })?;
        Ok(LlmResponse { value, usage })
    } else {
        Ok(LlmResponse {
            value: Value::String(content.to_string()),
            usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_ollama_prefix() {
        let a = OllamaAdapter::new();
        assert!(a.handles("ollama:llama3.2"));
        assert!(a.handles("ollama:qwen2.5-coder:7b"));
        assert!(!a.handles("llama3.2"));
        assert!(!a.handles("gpt-4o-mini"));
        assert!(!a.handles("claude-opus-4-6"));
    }

    #[test]
    fn extract_handles_message_content() {
        let parsed = json!({
            "message": {"role": "assistant", "content": "hello world"},
            "prompt_eval_count": 12,
            "eval_count": 3
        });
        let r = extract_response(&parsed, false).unwrap();
        assert_eq!(r.value, json!("hello world"));
        assert_eq!(r.usage.prompt_tokens, 12);
        assert_eq!(r.usage.completion_tokens, 3);
        assert_eq!(r.usage.total_tokens, 15);
    }

    #[test]
    fn extract_parses_structured_json_content() {
        let parsed = json!({
            "message": {"role": "assistant", "content": "{\"x\": 42}"},
        });
        let r = extract_response(&parsed, true).unwrap();
        assert_eq!(r.value, json!({"x": 42}));
    }

    #[test]
    fn extract_errors_on_missing_message() {
        let parsed = json!({});
        let err = extract_response(&parsed, false).unwrap_err();
        assert!(matches!(err, RuntimeError::AdapterFailed { .. }));
    }
}
