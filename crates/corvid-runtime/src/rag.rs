use crate::errors::RuntimeError;
use crate::provenance::GroundedValue;
use crate::tracing::now_ms;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::path::Path;

mod embedders;
mod types;

pub use embedders::{OllamaEmbedder, OpenAiEmbedder, OLLAMA_EMBEDDING_BASE, OPENAI_EMBEDDING_BASE};
pub use types::{
    EmbedderConfig, EmbeddingVector, RagChunk, RagChunkingConfig, RagDocument, RagEmbedder,
    RagEmbeddingRecord, RagSearchHit,
};

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

pub fn load_html(path: impl AsRef<Path>) -> Result<RagDocument, RuntimeError> {
    let path = path.as_ref();
    let html = std::fs::read_to_string(path).map_err(|err| {
        RuntimeError::Other(format!(
            "failed to read html document `{}`: {err}",
            path.display()
        ))
    })?;
    let text = extract_html_text(&html);
    let id = stable_id(path.display().to_string().as_bytes());
    document_from_text(id, path.display().to_string(), "text/html", text)
}

pub fn load_pdf(path: impl AsRef<Path>) -> Result<RagDocument, RuntimeError> {
    let path = path.as_ref();
    let text = pdf_extract::extract_text(path).map_err(|err| {
        RuntimeError::Other(format!(
            "failed to read pdf document `{}`: {err}",
            path.display()
        ))
    })?;
    let id = stable_id(path.display().to_string().as_bytes());
    document_from_text(
        id,
        path.display().to_string(),
        "application/pdf",
        normalize_html_text(&text),
    )
}

pub fn chunk_document(
    document: &RagDocument,
    max_chars: usize,
    overlap_chars: usize,
) -> Result<Vec<RagChunk>, RuntimeError> {
    chunk_document_with_config(document, &RagChunkingConfig::new(max_chars, overlap_chars))
}

pub fn chunk_document_with_config(
    document: &RagDocument,
    config: &RagChunkingConfig,
) -> Result<Vec<RagChunk>, RuntimeError> {
    if config.max_chars == 0 {
        return Err(RuntimeError::Other(
            "std.rag chunk size must be greater than zero".to_string(),
        ));
    }
    let chars: Vec<(usize, char)> = document.text.char_indices().collect();
    if chars.is_empty() {
        return Ok(Vec::new());
    }
    let overlap = config.overlap_chars.min(config.max_chars.saturating_sub(1));
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let target_end = (start + config.max_chars).min(chars.len());
        let end = choose_chunk_end(&chars, &document.text, start, target_end, config);
        let (start_idx, end_idx) = trim_chunk_window(&chars, &document.text, start, end, config);
        if start_idx >= end_idx {
            start = target_end;
            continue;
        }
        let start_byte = chars[start_idx].0;
        let end_byte = if end_idx == chars.len() {
            document.text.len()
        } else {
            chars[end_idx].0
        };
        let text = document.text[start_byte..end_byte].to_string();
        let provenance_key = provenance_key(&document.id, start_idx, end_idx, &text);
        chunks.push(RagChunk {
            doc_id: document.id.clone(),
            chunk_id: format!("{}:{}", document.id, chunks.len()),
            source: document.source.clone(),
            text,
            start_char: start_idx,
            end_char: end_idx,
            provenance_key,
        });
        if end_idx == chars.len() {
            break;
        }
        start = end_idx.saturating_sub(overlap);
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
        let conn = Connection::open_in_memory().map_err(|err| {
            RuntimeError::Other(format!("failed to open RAG memory index: {err}"))
        })?;
        let index = Self { conn };
        index.init()?;
        Ok(index)
    }

    pub fn insert_document(&self, document: &RagDocument) -> Result<(), RuntimeError> {
        self.conn
            .execute(
                "insert or replace into rag_documents (id, source, media_type, text)
                 values (?1, ?2, ?3, ?4)",
                params![
                    document.id,
                    document.source,
                    document.media_type,
                    document.text
                ],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to insert RAG document: {err}")))?;
        Ok(())
    }

    pub fn insert_chunks(&mut self, chunks: &[RagChunk]) -> Result<(), RuntimeError> {
        let tx = self.conn.transaction().map_err(|err| {
            RuntimeError::Other(format!("failed to start RAG chunk insert: {err}"))
        })?;
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

    pub fn search_grounded_text(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<GroundedValue<RagChunk>>, RuntimeError> {
        let source_name = format!("std.rag.search_text:{query}");
        Ok(self
            .search_text(query, limit)?
            .into_iter()
            .map(|chunk| GroundedValue::new(chunk, grounded_chunk_chain(&source_name)))
            .collect())
    }

    pub fn insert_embedding_vectors(
        &mut self,
        embeddings: &[RagEmbeddingRecord],
    ) -> Result<(), RuntimeError> {
        let tx = self.conn.transaction().map_err(|err| {
            RuntimeError::Other(format!("failed to start RAG embedding insert: {err}"))
        })?;
        for embedding in embeddings {
            let payload = serde_json::to_string(&embedding.values).map_err(|err| {
                RuntimeError::Other(format!("failed to encode RAG embedding vector: {err}"))
            })?;
            tx.execute(
                "insert or replace into rag_chunk_embeddings
                 (chunk_id, dimension, values_json)
                 values (?1, ?2, ?3)",
                params![embedding.chunk_id, embedding.values.len() as i64, payload],
            )
            .map_err(|err| RuntimeError::Other(format!("failed to insert RAG embedding: {err}")))?;
        }
        tx.commit().map_err(|err| {
            RuntimeError::Other(format!("failed to commit RAG embeddings: {err}"))
        })?;
        Ok(())
    }

    pub fn search_embeddings(
        &self,
        query: &[f32],
        limit: usize,
    ) -> Result<Vec<RagSearchHit>, RuntimeError> {
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare(
                "select c.chunk_id, c.doc_id, c.source, c.text, c.start_char, c.end_char, c.provenance_key,
                        e.values_json
                 from rag_chunks c
                 join rag_chunk_embeddings e on e.chunk_id = c.chunk_id
                 where e.dimension = ?1",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to prepare RAG embedding search: {err}")))?;
        let rows = stmt
            .query_map(params![query.len() as i64], |row| {
                Ok((
                    RagChunk {
                        chunk_id: row.get(0)?,
                        doc_id: row.get(1)?,
                        source: row.get(2)?,
                        text: row.get(3)?,
                        start_char: row.get::<_, i64>(4)? as usize,
                        end_char: row.get::<_, i64>(5)? as usize,
                        provenance_key: row.get(6)?,
                    },
                    row.get::<_, String>(7)?,
                ))
            })
            .map_err(|err| RuntimeError::Other(format!("failed to query RAG embeddings: {err}")))?;
        let mut hits = Vec::new();
        for row in rows {
            let (chunk, values_json) = row.map_err(|err| {
                RuntimeError::Other(format!("failed to read RAG embedding row: {err}"))
            })?;
            let values: Vec<f32> = serde_json::from_str(&values_json).map_err(|err| {
                RuntimeError::Other(format!("failed to decode RAG embedding vector: {err}"))
            })?;
            let score = cosine_similarity(query, &values);
            hits.push(RagSearchHit { chunk, score });
        }
        hits.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if hits.len() > limit {
            hits.truncate(limit);
        }
        Ok(hits)
    }

    pub fn search_grounded_embeddings(
        &self,
        query_label: &str,
        query: &[f32],
        limit: usize,
    ) -> Result<Vec<GroundedValue<RagChunk>>, RuntimeError> {
        let source_name = format!("std.rag.search_embeddings:{query_label}");
        Ok(self
            .search_embeddings(query, limit)?
            .into_iter()
            .map(|hit| GroundedValue::new(hit.chunk, grounded_chunk_chain(&source_name)))
            .collect())
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
                create index if not exists rag_chunks_provenance_key on rag_chunks(provenance_key);
                create table if not exists rag_chunk_embeddings (
                    chunk_id text primary key,
                    dimension integer not null,
                    values_json text not null,
                    foreign key(chunk_id) references rag_chunks(chunk_id)
                );
                create index if not exists rag_chunk_embeddings_dimension on rag_chunk_embeddings(dimension);",
            )
            .map_err(|err| RuntimeError::Other(format!("failed to initialize RAG index: {err}")))?;
        Ok(())
    }
}

fn choose_chunk_end(
    chars: &[(usize, char)],
    text: &str,
    start: usize,
    target_end: usize,
    config: &RagChunkingConfig,
) -> usize {
    if !config.prefer_sentence_boundary || target_end >= chars.len() {
        return target_end;
    }
    let min_end = start + ((target_end - start) / 2).max(1);
    for candidate in (min_end..target_end).rev() {
        let boundary = chars[candidate - 1].1;
        if matches!(boundary, '.' | '!' | '?' | '\n') {
            let byte = chars[candidate].0;
            if text[..byte].chars().last().is_some() {
                return candidate;
            }
        }
    }
    target_end
}

fn trim_chunk_window(
    chars: &[(usize, char)],
    text: &str,
    start: usize,
    end: usize,
    config: &RagChunkingConfig,
) -> (usize, usize) {
    if !config.trim_whitespace || start >= end {
        return (start, end);
    }
    let mut trimmed_start = start;
    let mut trimmed_end = end;
    while trimmed_start < trimmed_end && chars[trimmed_start].1.is_whitespace() {
        trimmed_start += 1;
    }
    while trimmed_end > trimmed_start {
        let byte = if trimmed_end == chars.len() {
            text.len()
        } else {
            chars[trimmed_end].0
        };
        let prev = text[..byte].chars().next_back().unwrap_or_default();
        if prev.is_whitespace() {
            trimmed_end -= 1;
        } else {
            break;
        }
    }
    (trimmed_start, trimmed_end)
}

fn grounded_chunk_chain(source_name: &str) -> crate::provenance::ProvenanceChain {
    crate::provenance::ProvenanceChain::with_retrieval(source_name, now_ms())
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut left_norm = 0.0f32;
    let mut right_norm = 0.0f32;
    for (lhs, rhs) in left.iter().zip(right.iter()) {
        dot += lhs * rhs;
        left_norm += lhs * lhs;
        right_norm += rhs * rhs;
    }
    if left_norm == 0.0 || right_norm == 0.0 {
        0.0
    } else {
        dot / (left_norm.sqrt() * right_norm.sqrt())
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

fn extract_html_text(html: &str) -> String {
    let stripped = strip_html_blocks(html, "script");
    let stripped = strip_html_blocks(&stripped, "style");
    let mut out = String::with_capacity(stripped.len());
    let mut in_tag = false;
    let mut tag_name = String::new();
    for ch in stripped.chars() {
        if in_tag {
            if ch == '>' {
                let tag = tag_name
                    .trim()
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("");
                if matches!(
                    tag,
                    "br" | "p"
                        | "div"
                        | "li"
                        | "tr"
                        | "section"
                        | "article"
                        | "header"
                        | "footer"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                ) {
                    out.push('\n');
                }
                tag_name.clear();
                in_tag = false;
            } else {
                tag_name.push(ch.to_ascii_lowercase());
            }
            continue;
        }
        if ch == '<' {
            in_tag = true;
            continue;
        }
        out.push(ch);
    }
    normalize_html_text(&decode_html_entities(&out))
}

fn strip_html_blocks(html: &str, tag: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0;
    while let Some(relative_start) = lower[cursor..].find(&open) {
        let start = cursor + relative_start;
        out.push_str(&html[cursor..start]);
        let after_start = match lower[start..].find('>') {
            Some(offset) => start + offset + 1,
            None => {
                cursor = html.len();
                break;
            }
        };
        let block_end = match lower[after_start..].find(&close) {
            Some(offset) => after_start + offset + close.len(),
            None => {
                cursor = html.len();
                break;
            }
        };
        cursor = block_end;
    }
    if cursor < html.len() {
        out.push_str(&html[cursor..]);
    }
    out
}

fn decode_html_entities(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn normalize_html_text(text: &str) -> String {
    let mut out = String::new();
    let mut pending_space = false;
    let mut previous_was_newline = false;
    for ch in text.chars() {
        if ch == '\r' {
            continue;
        }
        if ch == '\n' {
            if !out.is_empty() && !previous_was_newline {
                out.push('\n');
            }
            pending_space = false;
            previous_was_newline = true;
            continue;
        }
        if ch.is_whitespace() {
            pending_space = !previous_was_newline;
            continue;
        }
        if pending_space && !out.is_empty() && !previous_was_newline {
            out.push(' ');
        }
        out.push(ch);
        pending_space = false;
        previous_was_newline = false;
    }
    out.trim().to_string()
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
    fn chunk_document_prefers_sentence_boundaries_and_trims_whitespace() {
        let doc = document_from_text(
            "doc1",
            "memory",
            "text/plain",
            "Alpha sentence.  Beta sentence.\nGamma sentence.",
        )
        .unwrap();
        let chunks = chunk_document_with_config(&doc, &RagChunkingConfig::new(20, 4)).unwrap();

        assert!(chunks.len() >= 2);
        assert_eq!(chunks[0].text, "Alpha sentence.");
        assert!(!chunks[1].text.starts_with(' '));
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

    #[test]
    fn sqlite_index_returns_grounded_hits() {
        let doc = document_from_text("doc1", "memory", "text/plain", "alpha beta gamma").unwrap();
        let chunks = chunk_document(&doc, 8, 0).unwrap();
        let mut index = RagSqliteIndex::open_in_memory().unwrap();
        index.insert_document(&doc).unwrap();
        index.insert_chunks(&chunks).unwrap();

        let hits = index.search_grounded_text("alpha", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].has_retrieval());
        assert_eq!(hits[0].value.doc_id, "doc1");
    }

    #[test]
    fn sqlite_index_searches_embeddings_by_similarity() {
        let doc =
            document_from_text("doc1", "memory", "text/plain", "alpha beta gamma delta").unwrap();
        let chunks = chunk_document(&doc, 6, 0).unwrap();
        let mut index = RagSqliteIndex::open_in_memory().unwrap();
        index.insert_document(&doc).unwrap();
        index.insert_chunks(&chunks).unwrap();
        index
            .insert_embedding_vectors(&[
                RagEmbeddingRecord {
                    chunk_id: chunks[0].chunk_id.clone(),
                    values: vec![1.0, 0.0],
                },
                RagEmbeddingRecord {
                    chunk_id: chunks[1].chunk_id.clone(),
                    values: vec![0.8, 0.2],
                },
                RagEmbeddingRecord {
                    chunk_id: chunks[2].chunk_id.clone(),
                    values: vec![0.0, 1.0],
                },
            ])
            .unwrap();

        let hits = index.search_embeddings(&[0.9, 0.1], 2).unwrap();
        assert_eq!(hits.len(), 2);
        assert!(hits[0].score >= hits[1].score);
        assert_eq!(hits[0].chunk.chunk_id, chunks[0].chunk_id);

        let grounded = index
            .search_grounded_embeddings("alpha-ish", &[0.9, 0.1], 1)
            .unwrap();
        assert_eq!(grounded.len(), 1);
        assert!(grounded[0].has_retrieval());
        assert_eq!(grounded[0].value.chunk_id, chunks[0].chunk_id);
    }

    #[test]
    fn load_html_extracts_readable_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("page.html");
        std::fs::write(
            &path,
            "<html><head><style>.x{color:red}</style></head><body><h1>Title</h1><p>Hello <b>world</b> &amp; friends</p><script>ignored()</script><div>Next line</div></body></html>",
        )
        .unwrap();

        let doc = load_html(&path).unwrap();

        assert_eq!(doc.media_type, "text/html");
        assert_eq!(doc.text, "Title\nHello world & friends\nNext line");
    }

    #[test]
    fn load_pdf_extracts_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.pdf");
        std::fs::write(&path, minimal_pdf_bytes("Hello PDF")).unwrap();

        let doc = load_pdf(&path).unwrap();

        assert_eq!(doc.media_type, "application/pdf");
        assert!(doc.text.contains("Hello PDF"));
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

    fn minimal_pdf_bytes(text: &str) -> Vec<u8> {
        let content = format!(
            "BT\n/F1 24 Tf\n72 72 Td\n({}) Tj\nET",
            escape_pdf_text(text)
        );
        let objects = vec![
            "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_string(),
            "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_string(),
            "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 144] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n".to_string(),
            format!(
                "4 0 obj\n<< /Length {} >>\nstream\n{}\nendstream\nendobj\n",
                content.len(),
                content
            ),
            "5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n"
                .to_string(),
        ];
        let mut pdf = String::from("%PDF-1.4\n");
        let mut offsets = vec![0usize];
        for object in &objects {
            offsets.push(pdf.len());
            pdf.push_str(object);
        }
        let xref_offset = pdf.len();
        pdf.push_str(&format!("xref\n0 {}\n", objects.len() + 1));
        pdf.push_str("0000000000 65535 f \n");
        for offset in offsets.iter().skip(1) {
            pdf.push_str(&format!("{offset:010} 00000 n \n"));
        }
        pdf.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            objects.len() + 1,
            xref_offset
        ));
        pdf.into_bytes()
    }

    fn escape_pdf_text(text: &str) -> String {
        text.replace('\\', "\\\\")
            .replace('(', "\\(")
            .replace(')', "\\)")
    }
}
