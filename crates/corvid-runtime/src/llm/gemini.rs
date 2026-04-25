//! Google Gemini adapter.
//!
//! Speaks `POST /v1beta/models/<model>:generateContent?key=<api-key>`
//! against `https://generativelanguage.googleapis.com`. Auth is via
//! query-parameter API key (Gemini's standard for the simple key-based
//! tier; Application Default Credentials live behind a separate code
//! path we do not ship yet).
//!
//! Model spec convention: any `gemini-*` model name in `CORVID_MODEL`
//! routes here. Examples: `gemini-2.0-flash`, `gemini-1.5-pro`.
//!
//! Structured output: Gemini supports `responseMimeType: "application/json"`
//! plus an optional `responseSchema` field. Corvid sets both when
//! the caller provides an `output_schema`.
//!
//! Out of scope: streaming (`:streamGenerateContent`), function-calling,
//! grounding, system instructions beyond what's pre-pended to the user
//! message, multi-modal inputs.

use crate::errors::RuntimeError;
use crate::llm::{LlmAdapter, LlmRequestRef, LlmResponse, TokenUsage};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::time::Duration;

pub const GEMINI_DEFAULT_BASE: &str = "https://generativelanguage.googleapis.com";

pub struct GeminiAdapter {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
}

impl GeminiAdapter {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: GEMINI_DEFAULT_BASE.to_string(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .expect("reqwest client builds with default config"),
        }
    }

    pub fn with_base_url(mut self, base: impl Into<String>) -> Self {
        self.base_url = base.into();
        self
    }
}

impl LlmAdapter for GeminiAdapter {
    fn name(&self) -> &str {
        "gemini"
    }

    fn handles(&self, model: &str) -> bool {
        model.starts_with("gemini-")
    }

    fn call<'a>(
        &'a self,
        req: &'a LlmRequestRef<'a>,
    ) -> BoxFuture<'a, Result<LlmResponse, RuntimeError>> {
        Box::pin(async move {
            let url = format!(
                "{}/v1beta/models/{}:generateContent?key={}",
                self.base_url.trim_end_matches('/'),
                req.model,
                self.api_key
            );

            let mut body = json!({
                "contents": [
                    {
                        "role": "user",
                        "parts": [{"text": req.rendered}]
                    }
                ]
            });
            if let Some(schema) = &req.output_schema {
                body["generationConfig"] = json!({
                    "responseMimeType": "application/json",
                    "responseSchema": schema,
                });
            }

            let resp = self
                .client
                .post(&url)
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| RuntimeError::AdapterFailed {
                    adapter: "gemini".into(),
                    message: format!("HTTP send failed: {e}"),
                })?;

            let status = resp.status();
            let body_text = resp.text().await.map_err(|e| RuntimeError::AdapterFailed {
                adapter: "gemini".into(),
                message: format!("reading response body failed: {e}"),
            })?;
            if !status.is_success() {
                return Err(RuntimeError::AdapterFailed {
                    adapter: "gemini".into(),
                    message: format!("HTTP {status}: {body_text}"),
                });
            }

            let parsed: Value = serde_json::from_str(&body_text).map_err(|e| {
                RuntimeError::AdapterFailed {
                    adapter: "gemini".into(),
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
    // Gemini shape:
    //   { "candidates": [
    //       {"content": {"parts": [{"text": "..."}], "role": "model"},
    //        "finishReason": ..., ...}
    //     ],
    //     "usageMetadata": {"promptTokenCount": N, "candidatesTokenCount": N, "totalTokenCount": N}
    //   }
    let text = parsed
        .get("candidates")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.as_array())
        .and_then(|arr| arr.first())
        .and_then(|p| p.get("text"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| RuntimeError::AdapterFailed {
            adapter: "gemini".into(),
            message: "response missing `candidates[0].content.parts[0].text`".into(),
        })?;

    let usage = extract_usage(parsed);

    if expect_structured {
        let value: Value = serde_json::from_str(text).map_err(|e| {
            RuntimeError::AdapterFailed {
                adapter: "gemini".into(),
                message: format!(
                    "structured response was not valid JSON: {e} (content: {text})"
                ),
            }
        })?;
        Ok(LlmResponse::new(value, usage))
    } else {
        Ok(LlmResponse::new(Value::String(text.to_string()), usage))
    }
}

fn extract_usage(parsed: &Value) -> TokenUsage {
    let meta = parsed.get("usageMetadata");
    let prompt_tokens = meta
        .and_then(|m| m.get("promptTokenCount"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let completion_tokens = meta
        .and_then(|m| m.get("candidatesTokenCount"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let total_tokens = meta
        .and_then(|m| m.get("totalTokenCount"))
        .and_then(|v| v.as_u64())
        .unwrap_or((prompt_tokens + completion_tokens) as u64)
        as u32;
    TokenUsage {
        prompt_tokens,
        completion_tokens,
        total_tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_gemini_prefix_only() {
        let a = GeminiAdapter::new("k");
        assert!(a.handles("gemini-2.0-flash"));
        assert!(a.handles("gemini-1.5-pro"));
        assert!(!a.handles("gpt-4o-mini"));
        assert!(!a.handles("claude-opus-4-6"));
        assert!(!a.handles("ollama:llama3.2"));
    }

    #[test]
    fn extract_text_and_usage() {
        let parsed = json!({
            "candidates": [
                {"content": {"role": "model", "parts": [{"text": "hello"}]}}
            ],
            "usageMetadata": {
                "promptTokenCount": 7,
                "candidatesTokenCount": 1,
                "totalTokenCount": 8
            }
        });
        let r = extract_response(&parsed, false).unwrap();
        assert_eq!(r.value, json!("hello"));
        assert_eq!(r.usage.total_tokens, 8);
    }

    #[test]
    fn extract_parses_structured_json() {
        let parsed = json!({
            "candidates": [
                {"content": {"role": "model", "parts": [{"text": "{\"k\": 1}"}]}}
            ]
        });
        let r = extract_response(&parsed, true).unwrap();
        assert_eq!(r.value, json!({"k": 1}));
    }
}
