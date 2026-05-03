use super::types::{EmbedderConfig, EmbeddingVector, RagEmbedder};
use crate::errors::RuntimeError;
use futures::future::BoxFuture;
use std::time::Duration;

pub const OPENAI_EMBEDDING_BASE: &str = "https://api.openai.com";
pub const OLLAMA_EMBEDDING_BASE: &str = "http://localhost:11434";

pub struct OpenAiEmbedder {
    api_key: String,
    config: EmbedderConfig,
    base_url: String,
    client: reqwest::Client,
}

impl OpenAiEmbedder {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            config: EmbedderConfig::openai(model),
            base_url: OPENAI_EMBEDDING_BASE.to_string(),
            client: build_embed_client(),
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn config(&self) -> &EmbedderConfig {
        &self.config
    }
}

impl RagEmbedder for OpenAiEmbedder {
    fn provider(&self) -> &str {
        &self.config.provider
    }

    fn model(&self) -> &str {
        &self.config.model
    }

    fn embed<'a>(
        &'a self,
        texts: &'a [String],
    ) -> BoxFuture<'a, Result<Vec<EmbeddingVector>, RuntimeError>> {
        Box::pin(async move {
            if texts.is_empty() {
                return Ok(Vec::new());
            }
            let url = format!("{}/v1/embeddings", self.base_url.trim_end_matches('/'));
            let response = self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("content-type", "application/json")
                .json(&serde_json::json!({
                    "model": self.config.model,
                    "input": texts,
                }))
                .send()
                .await
                .map_err(|err| RuntimeError::AdapterFailed {
                    adapter: "std.rag.openai".to_string(),
                    message: format!("HTTP send failed: {err}"),
                })?;
            let status = response.status();
            let body = response
                .text()
                .await
                .map_err(|err| RuntimeError::AdapterFailed {
                    adapter: "std.rag.openai".to_string(),
                    message: format!("reading response body failed: {err}"),
                })?;
            if !status.is_success() {
                return Err(RuntimeError::AdapterFailed {
                    adapter: "std.rag.openai".to_string(),
                    message: format!("HTTP {status}: {body}"),
                });
            }
            parse_openai_embeddings(&body)
        })
    }
}

pub struct OllamaEmbedder {
    config: EmbedderConfig,
    client: reqwest::Client,
}

impl OllamaEmbedder {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            config: EmbedderConfig::ollama(model, OLLAMA_EMBEDDING_BASE),
            client: build_embed_client(),
        }
    }

    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.config.endpoint = Some(endpoint.into());
        self
    }

    pub fn config(&self) -> &EmbedderConfig {
        &self.config
    }
}

impl RagEmbedder for OllamaEmbedder {
    fn provider(&self) -> &str {
        &self.config.provider
    }

    fn model(&self) -> &str {
        &self.config.model
    }

    fn embed<'a>(
        &'a self,
        texts: &'a [String],
    ) -> BoxFuture<'a, Result<Vec<EmbeddingVector>, RuntimeError>> {
        Box::pin(async move {
            if texts.is_empty() {
                return Ok(Vec::new());
            }
            let endpoint = self
                .config
                .endpoint
                .as_deref()
                .unwrap_or(OLLAMA_EMBEDDING_BASE);
            let url = format!("{}/api/embed", endpoint.trim_end_matches('/'));
            let response = self
                .client
                .post(&url)
                .header("content-type", "application/json")
                .json(&serde_json::json!({
                    "model": self.config.model,
                    "input": texts,
                }))
                .send()
                .await
                .map_err(|err| RuntimeError::AdapterFailed {
                    adapter: "std.rag.ollama".to_string(),
                    message: format!("HTTP send failed: {err}"),
                })?;
            let status = response.status();
            let body = response
                .text()
                .await
                .map_err(|err| RuntimeError::AdapterFailed {
                    adapter: "std.rag.ollama".to_string(),
                    message: format!("reading response body failed: {err}"),
                })?;
            if !status.is_success() {
                return Err(RuntimeError::AdapterFailed {
                    adapter: "std.rag.ollama".to_string(),
                    message: format!("HTTP {status}: {body}"),
                });
            }
            parse_ollama_embeddings(&body)
        })
    }
}

fn build_embed_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .expect("reqwest client builds with default config")
}

fn parse_openai_embeddings(body: &str) -> Result<Vec<EmbeddingVector>, RuntimeError> {
    let parsed: serde_json::Value =
        serde_json::from_str(body).map_err(|err| RuntimeError::AdapterFailed {
            adapter: "std.rag.openai".to_string(),
            message: format!("response body is not JSON: {err} (body: {body})"),
        })?;
    let data = parsed
        .get("data")
        .and_then(|value| value.as_array())
        .ok_or_else(|| RuntimeError::AdapterFailed {
            adapter: "std.rag.openai".to_string(),
            message: "response missing `data` embedding array".to_string(),
        })?;
    let mut embeddings = Vec::with_capacity(data.len());
    for item in data {
        let index = item
            .get("index")
            .and_then(|value| value.as_u64())
            .ok_or_else(|| RuntimeError::AdapterFailed {
                adapter: "std.rag.openai".to_string(),
                message: "embedding entry missing numeric `index`".to_string(),
            })? as usize;
        let values = parse_embedding_values(
            item.get("embedding"),
            "std.rag.openai",
            "embedding entry missing numeric `embedding` values",
        )?;
        embeddings.push(EmbeddingVector { index, values });
    }
    embeddings.sort_by_key(|embedding| embedding.index);
    Ok(embeddings)
}

fn parse_ollama_embeddings(body: &str) -> Result<Vec<EmbeddingVector>, RuntimeError> {
    let parsed: serde_json::Value =
        serde_json::from_str(body).map_err(|err| RuntimeError::AdapterFailed {
            adapter: "std.rag.ollama".to_string(),
            message: format!("response body is not JSON: {err} (body: {body})"),
        })?;
    if let Some(embeddings) = parsed.get("embeddings").and_then(|value| value.as_array()) {
        let mut out = Vec::with_capacity(embeddings.len());
        for (index, item) in embeddings.iter().enumerate() {
            let values = parse_embedding_values(
                Some(item),
                "std.rag.ollama",
                "response missing numeric `embeddings` values",
            )?;
            out.push(EmbeddingVector { index, values });
        }
        return Ok(out);
    }
    if let Some(item) = parsed.get("embedding") {
        return Ok(vec![EmbeddingVector {
            index: 0,
            values: parse_embedding_values(
                Some(item),
                "std.rag.ollama",
                "response missing numeric `embedding` values",
            )?,
        }]);
    }
    Err(RuntimeError::AdapterFailed {
        adapter: "std.rag.ollama".to_string(),
        message: "response missing `embeddings` or `embedding`".to_string(),
    })
}

fn parse_embedding_values(
    value: Option<&serde_json::Value>,
    adapter: &str,
    missing_message: &str,
) -> Result<Vec<f32>, RuntimeError> {
    let array =
        value
            .and_then(|value| value.as_array())
            .ok_or_else(|| RuntimeError::AdapterFailed {
                adapter: adapter.to_string(),
                message: missing_message.to_string(),
            })?;
    let mut values = Vec::with_capacity(array.len());
    for item in array {
        let number = item.as_f64().ok_or_else(|| RuntimeError::AdapterFailed {
            adapter: adapter.to_string(),
            message: missing_message.to_string(),
        })?;
        values.push(number as f32);
    }
    Ok(values)
}
