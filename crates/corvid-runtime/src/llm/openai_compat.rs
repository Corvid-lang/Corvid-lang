//! OpenAI-compatible adapter — the universal escape hatch for any
//! inference server that exposes `/v1/chat/completions`.
//!
//! Covers (non-exhaustive): OpenRouter, Together AI, Anyscale, Groq,
//! Fireworks, DeepInfra, Mistral's hosted API, Azure OpenAI, llama.cpp
//! server, vLLM, LM Studio, OpenAI itself when accessed via a
//! non-default base URL. One adapter, dozens of backends.
//!
//! Model spec convention in `CORVID_MODEL`:
//!   `openai-compat:<base-url>:<model>`
//!
//! Examples:
//!   `openai-compat:https://api.together.xyz/v1:meta-llama/Llama-3.3-70B-Instruct-Turbo`
//!   `openai-compat:http://localhost:8080/v1:my-local-model`
//!   `openai-compat:https://openrouter.ai/api/v1:anthropic/claude-3.5-sonnet`
//!
//! Auth: API key from `OPENAI_COMPAT_API_KEY` env var. Empty key sends
//! no Authorization header (works for local servers without auth).
//!
//! Why a separate adapter from `openai.rs`: vanilla OpenAI keys off
//! `gpt-*` / `o*-*` model prefixes (no URL needed); compat keys off the
//! `openai-compat:` namespace (URL required per call). Conflating the
//! two would force one of: per-call URL parsing on every gpt-* request
//! (waste), or a magic `openai-compat:` prefix on top of `gpt-*`
//! (confusing). Two adapters with clean handles() splits is the right
//! architectural shape.

use crate::errors::RuntimeError;
use crate::llm::{LlmAdapter, LlmRequestRef, LlmResponse};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::time::Duration;

pub const OPENAI_COMPAT_MODEL_PREFIX: &str = "openai-compat:";

pub struct OpenAiCompatibleAdapter {
    api_key: String,
    client: reqwest::Client,
}

impl OpenAiCompatibleAdapter {
    /// Build with an API key (may be empty for local servers without auth).
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            // Long timeout for local servers running on CPU.
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(180))
                .build()
                .expect("reqwest client builds with default config"),
        }
    }
}

impl LlmAdapter for OpenAiCompatibleAdapter {
    fn name(&self) -> &str {
        "openai-compat"
    }

    fn handles(&self, model: &str) -> bool {
        model.starts_with(OPENAI_COMPAT_MODEL_PREFIX)
    }

    fn call<'a>(
        &'a self,
        req: &'a LlmRequestRef<'a>,
    ) -> BoxFuture<'a, Result<LlmResponse, RuntimeError>> {
        Box::pin(async move {
            let (base_url, model_name) = parse_spec(&req.model)?;
            let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

            let mut body = json!({
                "model": model_name,
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

            let mut request = self
                .client
                .post(&url)
                .header("content-type", "application/json")
                .json(&body);
            if !self.api_key.is_empty() {
                request = request
                    .header("Authorization", format!("Bearer {}", self.api_key));
            }

            let resp = request
                .send()
                .await
                .map_err(|e| RuntimeError::AdapterFailed {
                    adapter: "openai-compat".into(),
                    message: format!("HTTP send failed (server `{base_url}`): {e}"),
                })?;

            let status = resp.status();
            let body_text = resp.text().await.map_err(|e| RuntimeError::AdapterFailed {
                adapter: "openai-compat".into(),
                message: format!("reading response body failed: {e}"),
            })?;
            if !status.is_success() {
                return Err(RuntimeError::AdapterFailed {
                    adapter: "openai-compat".into(),
                    message: format!("HTTP {status} from `{base_url}`: {body_text}"),
                });
            }

            let parsed: Value = serde_json::from_str(&body_text).map_err(|e| {
                RuntimeError::AdapterFailed {
                    adapter: "openai-compat".into(),
                    message: format!(
                        "response body is not JSON: {e} (body: {body_text})"
                    ),
                }
            })?;

            // Reuse the OpenAI extractor — same response shape by
            // definition for any compatible server.
            extract_response_compat(&parsed, req.output_schema.is_some())
        })
    }
}

/// Parse `openai-compat:<base>:<model>` into `(base, model)`.
///
/// `<base>` may itself contain colons (https URLs do) so we can't
/// split-on-colon naively. Strategy: strip the `openai-compat:`
/// prefix, find the LAST `:` that separates URL from model name.
/// This works for every URL-shaped base because URLs end with a path
/// segment that doesn't contain `:` after the optional `://`. The
/// last colon in the remaining string is the URL/model separator.
fn parse_spec(model: &str) -> Result<(&str, &str), RuntimeError> {
    let rest = model
        .strip_prefix(OPENAI_COMPAT_MODEL_PREFIX)
        .ok_or_else(|| RuntimeError::AdapterFailed {
            adapter: "openai-compat".into(),
            message: format!("model `{model}` is missing the `openai-compat:` prefix"),
        })?;
    let cut = rest.rfind(':').ok_or_else(|| RuntimeError::AdapterFailed {
        adapter: "openai-compat".into(),
        message: format!(
            "model spec `{model}` is malformed — expected `openai-compat:<base-url>:<model>`"
        ),
    })?;
    let (base, model_with_colon) = rest.split_at(cut);
    let model_name = &model_with_colon[1..]; // skip the colon
    if base.is_empty() || model_name.is_empty() {
        return Err(RuntimeError::AdapterFailed {
            adapter: "openai-compat".into(),
            message: format!(
                "model spec `{model}` has empty base-url or model — expected `openai-compat:<base-url>:<model>`"
            ),
        });
    }
    Ok((base, model_name))
}

/// Same shape as `openai.rs::extract_response` since OpenAI-compat
/// servers return the same structure by definition. Inlined here so
/// `openai_compat` doesn't pull in an adapter-internal symbol from
/// `openai`.
fn extract_response_compat(
    parsed: &Value,
    expect_structured: bool,
) -> Result<LlmResponse, RuntimeError> {
    let content = parsed
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| RuntimeError::AdapterFailed {
            adapter: "openai-compat".into(),
            message: "response missing `choices[0].message.content`".into(),
        })?;

    let usage = crate::llm::openai::extract_usage(parsed);

    if expect_structured {
        let value: Value = serde_json::from_str(content).map_err(|e| {
            RuntimeError::AdapterFailed {
                adapter: "openai-compat".into(),
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
    fn handles_compat_prefix_only() {
        let a = OpenAiCompatibleAdapter::new("k");
        assert!(a.handles("openai-compat:http://localhost:8080/v1:my-model"));
        assert!(a.handles("openai-compat:https://api.together.xyz/v1:meta-llama/Llama-3.3-70B"));
        assert!(!a.handles("gpt-4o-mini"));
        assert!(!a.handles("claude-opus-4-6"));
        assert!(!a.handles("ollama:llama3.2"));
    }

    #[test]
    fn parse_spec_basic_url() {
        let (base, model) =
            parse_spec("openai-compat:https://api.together.xyz/v1:meta-llama/Llama-3").unwrap();
        assert_eq!(base, "https://api.together.xyz/v1");
        assert_eq!(model, "meta-llama/Llama-3");
    }

    #[test]
    fn parse_spec_local_with_port() {
        let (base, model) =
            parse_spec("openai-compat:http://localhost:8080/v1:my-model").unwrap();
        assert_eq!(base, "http://localhost:8080/v1");
        assert_eq!(model, "my-model");
    }

    #[test]
    fn parse_spec_rejects_malformed() {
        assert!(parse_spec("openai-compat:nocolon").is_err());
        assert!(parse_spec("openai-compat::model").is_err());
        assert!(parse_spec("openai-compat:url:").is_err());
    }
}
