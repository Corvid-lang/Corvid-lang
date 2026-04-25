//! OpenAI Chat Completions adapter.
//!
//! Speaks `POST /v1/chat/completions` directly. Structured output uses
//! `response_format: {type: "json_schema", json_schema: {...}}` with
//! `strict: true`. The JSON Schema we build (`crate::schema_for`)
//! already meets OpenAI strict-mode requirements: every property listed
//! in `required`, `additionalProperties: false`.
//!
//! Auth: `Authorization: Bearer <key>`.
//!
//! Out of scope (deferred): streaming, function calling, vision, batch,
//! reasoning-effort knobs for o-series.

use crate::errors::RuntimeError;
use crate::llm::{LlmAdapter, LlmRequestRef, LlmResponse, TokenUsage};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::time::Duration;

pub const OPENAI_DEFAULT_BASE: &str = "https://api.openai.com";

pub struct OpenAiAdapter {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
}

impl OpenAiAdapter {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: OPENAI_DEFAULT_BASE.to_string(),
            client: build_client(),
        }
    }

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

impl LlmAdapter for OpenAiAdapter {
    fn name(&self) -> &str {
        "openai"
    }

    fn handles(&self, model: &str) -> bool {
        model.starts_with("gpt-")
            || model.starts_with("o1-")
            || model.starts_with("o3-")
            || model.starts_with("o4-")
            || model == "o1"
            || model == "o3"
            || model == "o4"
    }

    fn call<'a>(
        &'a self,
        req: &'a LlmRequestRef<'a>,
    ) -> BoxFuture<'a, Result<LlmResponse, RuntimeError>> {
        Box::pin(async move {
            let url = format!(
                "{}/v1/chat/completions",
                self.base_url.trim_end_matches('/')
            );
            let body = build_request_body(req);
            let resp = self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| RuntimeError::AdapterFailed {
                    adapter: "openai".into(),
                    message: format!("HTTP send failed: {e}"),
                })?;

            let status = resp.status();
            let body_text = resp.text().await.map_err(|e| RuntimeError::AdapterFailed {
                adapter: "openai".into(),
                message: format!("reading response body failed: {e}"),
            })?;

            if !status.is_success() {
                return Err(RuntimeError::AdapterFailed {
                    adapter: "openai".into(),
                    message: format!("HTTP {status}: {body_text}"),
                });
            }

            let parsed: Value = serde_json::from_str(&body_text).map_err(|e| {
                RuntimeError::AdapterFailed {
                    adapter: "openai".into(),
                    message: format!("response body is not JSON: {e} (body: {body_text})"),
                }
            })?;
            extract_response(&parsed, req.output_schema.is_some())
        })
    }
}

fn build_request_body(req: &LlmRequestRef<'_>) -> Value {
    let mut body = json!({
        "model": req.model,
        "messages": [
            {"role": "user", "content": req.rendered}
        ],
    });

    if let Some(schema) = &req.output_schema {
        body["response_format"] = json!({
            "type": "json_schema",
            "json_schema": {
                "name": format!("respond_with_{}", req.prompt),
                "strict": true,
                "schema": schema,
            }
        });
    }

    body
}

fn extract_response(parsed: &Value, expect_structured: bool) -> Result<LlmResponse, RuntimeError> {
    let content = parsed
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| RuntimeError::AdapterFailed {
            adapter: "openai".into(),
            message: "response missing `choices[0].message.content`".into(),
        })?;

    let usage = extract_usage(parsed);

    if expect_structured {
        let value: Value = serde_json::from_str(content).map_err(|e| {
            RuntimeError::AdapterFailed {
                adapter: "openai".into(),
                message: format!(
                    "structured response was not valid JSON: {e} (content: {content})"
                ),
            }
        })?;
        Ok(LlmResponse::new(value, usage))
    } else {
        Ok(LlmResponse::new(Value::String(content.to_string()), usage))
    }
}

/// Pull the `usage` block out of an OpenAI response. Standard shape:
///   `{"usage": {"prompt_tokens": N, "completion_tokens": N, "total_tokens": N}}`
/// Returns zeros if missing — older endpoints + some compat servers
/// don't report usage; treating absence as "unknown" is safer than
/// failing the call.
pub(crate) fn extract_usage(parsed: &Value) -> TokenUsage {
    let usage = parsed.get("usage");
    let prompt_tokens = usage
        .and_then(|u| u.get("prompt_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let completion_tokens = usage
        .and_then(|u| u.get("completion_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let total_tokens = usage
        .and_then(|u| u.get("total_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or((prompt_tokens + completion_tokens) as u64) as u32;
    TokenUsage {
        prompt_tokens,
        completion_tokens,
        total_tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmRequest;

    #[test]
    fn handles_gpt_and_o_series_models() {
        let a = OpenAiAdapter::new("k");
        assert!(a.handles("gpt-4o-mini"));
        assert!(a.handles("gpt-4.1"));
        assert!(a.handles("o1-preview"));
        assert!(a.handles("o3-mini"));
        assert!(a.handles("o4-mini"));
        assert!(!a.handles("claude-opus-4-6"));
        assert!(!a.handles("gemini-pro"));
    }

    #[test]
    fn body_includes_response_format_when_schema_present() {
        let req = LlmRequest {
            prompt: "decide".into(),
            model: "gpt-4o-mini".into(),
            rendered: "decide pls".into(),
            args: vec![],
            output_schema: Some(json!({
                "type": "object",
                "properties": {"x": {"type": "boolean"}},
                "required": ["x"],
                "additionalProperties": false,
            })),
        };
        let body = build_request_body(&req.as_ref());
        let rf = &body["response_format"];
        assert_eq!(rf["type"], "json_schema");
        assert_eq!(rf["json_schema"]["name"], "respond_with_decide");
        assert_eq!(rf["json_schema"]["strict"], true);
        assert!(rf["json_schema"]["schema"].is_object());
    }

    #[test]
    fn body_omits_response_format_when_no_schema() {
        let req = LlmRequest {
            prompt: "chat".into(),
            model: "gpt-4o-mini".into(),
            rendered: "say hi".into(),
            args: vec![],
            output_schema: None,
        };
        let body = build_request_body(&req.as_ref());
        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn extract_parses_json_content_for_structured() {
        let parsed = json!({
            "choices": [
                {"message": {"role": "assistant", "content": "{\"should_refund\": true}"}}
            ]
        });
        let resp = extract_response(&parsed, true).unwrap();
        assert_eq!(resp.value, json!({"should_refund": true}));
    }

    #[test]
    fn extract_returns_raw_string_for_unstructured() {
        let parsed = json!({
            "choices": [
                {"message": {"role": "assistant", "content": "hello world"}}
            ]
        });
        let resp = extract_response(&parsed, false).unwrap();
        assert_eq!(resp.value, json!("hello world"));
    }

    #[test]
    fn extract_errors_on_invalid_json_in_structured_response() {
        let parsed = json!({
            "choices": [
                {"message": {"role": "assistant", "content": "this is not json"}}
            ]
        });
        let err = extract_response(&parsed, true).unwrap_err();
        assert!(matches!(err, RuntimeError::AdapterFailed { .. }));
    }
}
