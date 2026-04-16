//! Anthropic Messages API adapter.
//!
//! Speaks `POST /v1/messages` directly — no Anthropic SDK on the path.
//! Structured output is requested via Anthropic's `tool_use` mechanism:
//! we declare a synthetic tool whose `input_schema` matches the prompt's
//! return type and force the model to call it, then extract the tool
//! input as the result. This is the idiomatic way to get parseable JSON
//! out of Claude.
//!
//! Auth: `x-api-key` header. Version: pinned to the latest stable
//! API version we've validated against (`anthropic-version: 2023-06-01`).
//!
//! Out of scope (deferred): streaming, prompt caching, vision, batch.

use crate::errors::RuntimeError;
use crate::llm::{LlmAdapter, LlmRequest, LlmResponse};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::time::Duration;

/// Default base URL. Tests override via `with_base_url`.
pub const ANTHROPIC_DEFAULT_BASE: &str = "https://api.anthropic.com";

/// Anthropic API version header value.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Default max tokens cap. Most use cases fit; per-call override is future work.
const DEFAULT_MAX_TOKENS: u32 = 4096;

pub struct AnthropicAdapter {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
}

impl AnthropicAdapter {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: ANTHROPIC_DEFAULT_BASE.to_string(),
            client: build_client(),
        }
    }

    /// Override the base URL — used by integration tests that point the
    /// adapter at a `wiremock` server.
    pub fn with_base_url(mut self, base: impl Into<String>) -> Self {
        self.base_url = base.into();
        self
    }
}

fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .expect("reqwest client builds with default config")
}

impl LlmAdapter for AnthropicAdapter {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn handles(&self, model: &str) -> bool {
        model.starts_with("claude-")
    }

    fn call<'a>(
        &'a self,
        req: &'a LlmRequest,
    ) -> BoxFuture<'a, Result<LlmResponse, RuntimeError>> {
        Box::pin(async move {
            let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
            let body = build_request_body(req);
            let resp = self
                .client
                .post(&url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| RuntimeError::AdapterFailed {
                    adapter: "anthropic".into(),
                    message: format!("HTTP send failed: {e}"),
                })?;

            let status = resp.status();
            let body_text = resp.text().await.map_err(|e| RuntimeError::AdapterFailed {
                adapter: "anthropic".into(),
                message: format!("reading response body failed: {e}"),
            })?;

            if !status.is_success() {
                return Err(RuntimeError::AdapterFailed {
                    adapter: "anthropic".into(),
                    message: format!("HTTP {status}: {body_text}"),
                });
            }

            let parsed: Value = serde_json::from_str(&body_text).map_err(|e| {
                RuntimeError::AdapterFailed {
                    adapter: "anthropic".into(),
                    message: format!("response body is not JSON: {e} (body: {body_text})"),
                }
            })?;
            extract_response(&parsed, req.output_schema.is_some())
        })
    }
}

fn build_request_body(req: &LlmRequest) -> Value {
    let mut body = json!({
        "model": req.model,
        "max_tokens": DEFAULT_MAX_TOKENS,
        "messages": [
            {"role": "user", "content": req.rendered}
        ],
    });

    if let Some(schema) = &req.output_schema {
        let tool_name = format!("respond_with_{}", req.prompt);
        body["tools"] = json!([
            {
                "name": tool_name,
                "description": format!(
                    "Respond by calling this tool with the structured result for prompt `{}`.",
                    req.prompt
                ),
                "input_schema": schema,
            }
        ]);
        body["tool_choice"] = json!({ "type": "tool", "name": tool_name });
    }

    body
}

fn extract_response(parsed: &Value, expect_structured: bool) -> Result<LlmResponse, RuntimeError> {
    let content = parsed
        .get("content")
        .and_then(|v| v.as_array())
        .ok_or_else(|| RuntimeError::AdapterFailed {
            adapter: "anthropic".into(),
            message: "response missing `content` array".into(),
        })?;

    let usage = extract_usage(parsed);

    if expect_structured {
        for block in content {
            if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                if let Some(input) = block.get("input") {
                    return Ok(LlmResponse {
                        value: input.clone(),
                        usage,
                    });
                }
            }
        }
        Err(RuntimeError::AdapterFailed {
            adapter: "anthropic".into(),
            message: "expected a `tool_use` content block but none was returned".into(),
        })
    } else {
        // Unstructured: concatenate all text blocks.
        let mut buf = String::new();
        for block in content {
            if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                    buf.push_str(text);
                }
            }
        }
        Ok(LlmResponse {
            value: Value::String(buf),
            usage,
        })
    }
}

/// Pull token-usage out of an Anthropic response. Shape:
///   `{"usage": {"input_tokens": N, "output_tokens": N}}`
/// Note Anthropic uses `input_tokens`/`output_tokens` whereas OpenAI
/// uses `prompt_tokens`/`completion_tokens` — same concept,
/// different field names. Returns zeros when the provider doesn't
/// include the block (unusual but possible on edge cases).
fn extract_usage(parsed: &Value) -> crate::llm::TokenUsage {
    let usage = parsed.get("usage");
    let prompt_tokens = usage
        .and_then(|u| u.get("input_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let completion_tokens = usage
        .and_then(|u| u.get("output_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    crate::llm::TokenUsage {
        prompt_tokens,
        completion_tokens,
        total_tokens: prompt_tokens + completion_tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_only_claude_prefixed_models() {
        let a = AnthropicAdapter::new("k");
        assert!(a.handles("claude-opus-4-6"));
        assert!(a.handles("claude-haiku-4-5"));
        assert!(!a.handles("gpt-4o-mini"));
        assert!(!a.handles(""));
    }

    #[test]
    fn body_includes_tools_when_schema_present() {
        let req = LlmRequest {
            prompt: "decide".into(),
            model: "claude-opus-4-6".into(),
            rendered: "decide pls".into(),
            args: vec![],
            output_schema: Some(json!({
                "type": "object",
                "properties": {"x": {"type": "boolean"}},
                "required": ["x"],
            })),
        };
        let body = build_request_body(&req);
        assert_eq!(body["model"], "claude-opus-4-6");
        assert_eq!(body["messages"][0]["content"], "decide pls");
        assert_eq!(body["tools"][0]["name"], "respond_with_decide");
        assert_eq!(body["tool_choice"]["name"], "respond_with_decide");
        assert!(body["tools"][0]["input_schema"].is_object());
    }

    #[test]
    fn body_omits_tools_when_no_schema() {
        let req = LlmRequest {
            prompt: "chat".into(),
            model: "claude-opus-4-6".into(),
            rendered: "say hi".into(),
            args: vec![],
            output_schema: None,
        };
        let body = build_request_body(&req);
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
    }

    #[test]
    fn extract_picks_tool_use_input_for_structured() {
        let parsed = json!({
            "content": [
                {"type": "tool_use", "id": "x", "name": "respond_with_decide",
                 "input": {"should_refund": true}}
            ]
        });
        let resp = extract_response(&parsed, true).unwrap();
        assert_eq!(resp.value, json!({"should_refund": true}));
    }

    #[test]
    fn extract_concatenates_text_blocks_for_unstructured() {
        let parsed = json!({
            "content": [
                {"type": "text", "text": "hello "},
                {"type": "text", "text": "world"}
            ]
        });
        let resp = extract_response(&parsed, false).unwrap();
        assert_eq!(resp.value, json!("hello world"));
    }

    #[test]
    fn extract_errors_when_structured_response_missing_tool_use() {
        let parsed = json!({
            "content": [
                {"type": "text", "text": "I refuse to use the tool."}
            ]
        });
        let err = extract_response(&parsed, true).unwrap_err();
        assert!(matches!(err, RuntimeError::AdapterFailed { .. }));
    }
}
