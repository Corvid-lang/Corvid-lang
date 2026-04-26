use crate::errors::RuntimeError;
use futures::future::BoxFuture;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;

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
            let body = response.text().await.map_err(|err| RuntimeError::AdapterFailed {
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
            let body = response.text().await.map_err(|err| RuntimeError::AdapterFailed {
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

pub fn document_from_text(
    id: impl Into<String>,
    source: impl Into<String>,
    media_type: impl Into<String>,
    text: impl Into<String>,
) -> Result<RagDocument, RuntimeError> {
    let id = id.into();
    if id.trim().is_empty() {
        return Err(RuntimeError::Other(
            "std.rag document id must not be empty".to_string(),
        ));
    }
    Ok(RagDocument {
        id,
        source: source.into(),
        media_type: media_type.into(),
        text: text.into(),
    })
}

pub fn load_markdown(path: impl AsRef<Path>) -> Result<RagDocument, RuntimeError> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|err| {
        RuntimeError::Other(format!(
            "failed to read markdown document `{}`: {err}",
            path.display()
        ))
    })?;
    let id = stable_id(path.display().to_string().as_bytes());
    document_from_text(id, path.display().to_string(), "text/markdown", text)
}

pub fn chunk_document(
    document: &RagDocument,
    max_chars: usize,
    overlap_chars: usize,
) -> Result<Vec<RagChunk>, RuntimeError> {
    if max_chars == 0 {
        return Err(RuntimeError::Other(
            "std.rag chunk size must be greater than zero".to_string(),
        ));
    }
    let chars: Vec<(usize, char)> = document.text.char_indices().collect();
    if chars.is_empty() {
        return Ok(Vec::new());
    }
    let overlap = overlap_chars.min(max_chars.saturating_sub(1));
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = (start + max_chars).min(chars.len());
        let start_byte = chars[start].0;
        let end_byte = if end == chars.len() {
            document.text.len()
        } else {
            chars[end].0
        };
        let text = document.text[start_byte..end_byte].to_string();
        let provenance_key = provenance_key(&document.id, start, end, &text);
        chunks.push(RagChunk {
            doc_id: document.id.clone(),
            chunk_id: format!("{}:{}", document.id, chunks.len()),
            source: document.source.clone(),
            text,
            start_char: start,
            end_char: end,
            provenance_key,
        });
        if end == chars.len() {
            break;
        }
        start = end.saturating_sub(overlap);
    }
    Ok(chunks)
}

pub struct RagSqliteIndex {
    conn: Connection,
}

impl RagSqliteIndex {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, RuntimeError> {
        let conn = Connection::open(path.as_ref()).map_err(|err| {
            RuntimeError::Other(format!(
                "failed to open RAG index `{}`: {err}",
                path.as_ref().display()
            ))
        })?;
        let index = Self { conn };
        index.init()?;
        Ok(index)
    }

    pub fn open_in_memory() -> Result<Self, RuntimeError> {
        let conn = Connection::open_in_memory()
            .map_err(|err| RuntimeError::Other(format!("failed to open RAG memory index: {err}")))?;
        let index = Self { conn };
        index.init()?;
        Ok(index)
    }

    pub fn insert_document(&self, document: &RagDocument) -> Result<(), RuntimeError> {
        self.conn
            .execute(
                "insert or replace into rag_documents (id, source, media_type, text)
                 values (?1, ?2, ?3, ?4)",
                params![document.id, document.source, document.media_type, document.text],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to insert RAG document: {err}")))?;
        Ok(())
    }

    pub fn insert_chunks(&mut self, chunks: &[RagChunk]) -> Result<(), RuntimeError> {
        let tx = self
            .conn
            .transaction()
            .map_err(|err| RuntimeError::Other(format!("failed to start RAG chunk insert: {err}")))?;
        for chunk in chunks {
            tx.execute(
                "insert or replace into rag_chunks
                 (chunk_id, doc_id, source, text, start_char, end_char, provenance_key)
                 values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    chunk.chunk_id,
                    chunk.doc_id,
                    chunk.source,
                    chunk.text,
                    chunk.start_char as i64,
                    chunk.end_char as i64,
                    chunk.provenance_key
                ],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to insert RAG chunk: {err}")))?;
        }
        tx.commit()
            .map_err(|err| RuntimeError::Other(format!("failed to commit RAG chunks: {err}")))?;
        Ok(())
    }

    pub fn search_text(&self, query: &str, limit: usize) -> Result<Vec<RagChunk>, RuntimeError> {
        let escaped = query.replace('%', "\\%").replace('_', "\\_");
        let like = format!("%{escaped}%");
        let mut stmt = self
            .conn
            .prepare(
                "select chunk_id, doc_id, source, text, start_char, end_char, provenance_key
                 from rag_chunks
                 where text like ?1 escape '\\'
                 order by chunk_id
                 limit ?2",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to prepare RAG search: {err}")))?;
        let rows = stmt
            .query_map(params![like, limit as i64], |row| {
                Ok(RagChunk {
                    chunk_id: row.get(0)?,
                    doc_id: row.get(1)?,
                    source: row.get(2)?,
                    text: row.get(3)?,
                    start_char: row.get::<_, i64>(4)? as usize,
                    end_char: row.get::<_, i64>(5)? as usize,
                    provenance_key: row.get(6)?,
                })
            })
            .map_err(|err| RuntimeError::Other(format!("failed to query RAG chunks: {err}")))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(
                row.map_err(|err| RuntimeError::Other(format!("failed to read RAG chunk: {err}")))?,
            );
        }
        Ok(out)
    }

    fn init(&self) -> Result<(), RuntimeError> {
        self.conn
            .execute_batch(
                "create table if not exists rag_documents (
                    id text primary key,
                    source text not null,
                    media_type text not null,
                    text text not null
                );
                create table if not exists rag_chunks (
                    chunk_id text primary key,
                    doc_id text not null,
                    source text not null,
                    text text not null,
                    start_char integer not null,
                    end_char integer not null,
                    provenance_key text not null,
                    foreign key(doc_id) references rag_documents(id)
                );
                create index if not exists rag_chunks_doc_id on rag_chunks(doc_id);
                create index if not exists rag_chunks_provenance_key on rag_chunks(provenance_key);",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to initialize RAG index: {err}")))?;
        Ok(())
    }
}

fn stable_id(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("doc_{}", encode_hex(&hasher.finalize()[..8]))
}

fn provenance_key(doc_id: &str, start: usize, end: usize, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(doc_id.as_bytes());
    hasher.update(start.to_le_bytes());
    hasher.update(end.to_le_bytes());
    hasher.update(text.as_bytes());
    format!("rag_{}", encode_hex(&hasher.finalize()))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
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
    let array = value
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

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn chunk_document_preserves_provenance_windows() {
        let doc = document_from_text("doc1", "memory", "text/plain", "abcdefghij").unwrap();
        let chunks = chunk_document(&doc, 4, 1).unwrap();

        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].text, "abcd");
        assert_eq!(chunks[1].text, "defg");
        assert_eq!(chunks[2].text, "ghij");
        assert!(chunks[0].provenance_key.starts_with("rag_"));
        assert_ne!(chunks[0].provenance_key, chunks[1].provenance_key);
    }

    #[test]
    fn embedder_configs_cover_openai_and_ollama() {
        let openai = EmbedderConfig::openai("text-embedding-3-small");
        assert_eq!(openai.provider, "openai");
        assert_eq!(openai.endpoint, None);

        let ollama = EmbedderConfig::ollama("nomic-embed-text", "http://localhost:11434");
        assert_eq!(ollama.provider, "ollama");
        assert_eq!(ollama.endpoint.as_deref(), Some("http://localhost:11434"));
    }

    #[test]
    fn sqlite_index_round_trips_chunks_with_provenance() {
        let doc = document_from_text("doc1", "memory", "text/plain", "alpha beta gamma").unwrap();
        let chunks = chunk_document(&doc, 8, 0).unwrap();
        let mut index = RagSqliteIndex::open_in_memory().unwrap();
        index.insert_document(&doc).unwrap();
        index.insert_chunks(&chunks).unwrap();

        let hits = index.search_text("alpha", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].doc_id, "doc1");
        assert!(hits[0].provenance_key.starts_with("rag_"));
    }

    #[tokio::test]
    async fn openai_embedder_posts_embedding_requests() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .and(header("authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {"index": 1, "embedding": [0.3, 0.4]},
                    {"index": 0, "embedding": [0.1, 0.2]}
                ]
            })))
            .mount(&server)
            .await;

        let embedder =
            OpenAiEmbedder::new("test-key", "text-embedding-3-small").with_base_url(server.uri());
        let texts = vec!["alpha".to_string(), "beta".to_string()];
        let embeddings = embedder.embed(&texts).await.unwrap();

        assert_eq!(embedder.provider(), "openai");
        assert_eq!(embedder.model(), "text-embedding-3-small");
        assert_eq!(embeddings.len(), 2);
        assert_eq!(embeddings[0].index, 0);
        assert_eq!(embeddings[0].values, vec![0.1, 0.2]);
        assert_eq!(embeddings[1].index, 1);
    }

    #[tokio::test]
    async fn ollama_embedder_reads_batch_embeddings() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embed"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "embeddings": [
                    [0.5, 0.6],
                    [0.7, 0.8]
                ]
            })))
            .mount(&server)
            .await;

        let embedder = OllamaEmbedder::new("nomic-embed-text").with_endpoint(server.uri());
        let texts = vec!["alpha".to_string(), "beta".to_string()];
        let embeddings = embedder.embed(&texts).await.unwrap();

        assert_eq!(embedder.provider(), "ollama");
        assert_eq!(embedder.model(), "nomic-embed-text");
        assert_eq!(embeddings.len(), 2);
        assert_eq!(embeddings[0].values, vec![0.5, 0.6]);
        assert_eq!(embeddings[1].values, vec![0.7, 0.8]);
    }
}
