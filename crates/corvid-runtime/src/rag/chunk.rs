use super::{encode_hex, RagChunk, RagChunkingConfig, RagDocument};
use crate::errors::RuntimeError;
use sha2::{Digest, Sha256};
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

fn provenance_key(doc_id: &str, start: usize, end: usize, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(doc_id.as_bytes());
    hasher.update(start.to_le_bytes());
    hasher.update(end.to_le_bytes());
    hasher.update(text.as_bytes());
    format!("rag_{}", encode_hex(&hasher.finalize()))
}
