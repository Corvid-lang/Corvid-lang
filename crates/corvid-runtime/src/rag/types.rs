use crate::errors::RuntimeError;
use futures::future::BoxFuture;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RagDocument {
    pub id: String,
    pub source: String,
    pub media_type: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RagChunk {
    pub doc_id: String,
    pub chunk_id: String,
    pub source: String,
    pub text: String,
    pub start_char: usize,
    pub end_char: usize,
    pub provenance_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RagChunkingConfig {
    pub max_chars: usize,
    pub overlap_chars: usize,
    pub trim_whitespace: bool,
    pub prefer_sentence_boundary: bool,
}

impl RagChunkingConfig {
    pub fn new(max_chars: usize, overlap_chars: usize) -> Self {
        Self {
            max_chars,
            overlap_chars,
            trim_whitespace: true,
            prefer_sentence_boundary: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RagSearchHit {
    pub chunk: RagChunk,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RagEmbeddingRecord {
    pub chunk_id: String,
    pub values: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbedderConfig {
    pub provider: String,
    pub model: String,
    pub endpoint: Option<String>,
}

impl EmbedderConfig {
    pub fn openai(model: impl Into<String>) -> Self {
        Self {
            provider: "openai".to_string(),
            model: model.into(),
            endpoint: None,
        }
    }

    pub fn ollama(model: impl Into<String>, endpoint: impl Into<String>) -> Self {
        Self {
            provider: "ollama".to_string(),
            model: model.into(),
            endpoint: Some(endpoint.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingVector {
    pub index: usize,
    pub values: Vec<f32>,
}

pub trait RagEmbedder: Send + Sync {
    fn provider(&self) -> &str;
    fn model(&self) -> &str;
    fn embed<'a>(
        &'a self,
        texts: &'a [String],
    ) -> BoxFuture<'a, Result<Vec<EmbeddingVector>, RuntimeError>>;
}
