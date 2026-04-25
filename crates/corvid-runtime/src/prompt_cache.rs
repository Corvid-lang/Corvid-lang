use crate::llm::{LlmRequestRef, LlmResponse, TokenUsage};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// In-process cache for prompts declared `cacheable: true`.
///
/// The key is a stable SHA-256 over the semantic call boundary, not over
/// process-local pointers: prompt name, selected model, rendered text, JSON
/// arguments, and output schema. That keeps cache hits replayable and makes
/// trace fingerprints meaningful across processes.
#[derive(Clone, Default)]
pub struct PromptCache {
    entries: Arc<Mutex<HashMap<String, LlmResponse>>>,
}

impl PromptCache {
    pub fn fingerprint(req: LlmRequestRef<'_>) -> String {
        let canonical = json!({
            "prompt": req.prompt,
            "model": req.model,
            "rendered": req.rendered,
            "args": req.args,
            "output_schema": req.output_schema,
        });
        let mut hasher = Sha256::new();
        hasher.update(canonical.to_string().as_bytes());
        encode_hex(&hasher.finalize())
    }

    pub fn get(&self, fingerprint: &str) -> Option<LlmResponse> {
        self.entries.lock().unwrap().get(fingerprint).cloned()
    }

    pub fn insert(&self, fingerprint: String, response: LlmResponse) {
        self.entries.lock().unwrap().insert(fingerprint, response);
    }

    pub fn cached_response(response: LlmResponse) -> LlmResponse {
        LlmResponse {
            usage: TokenUsage::default(),
            ..response
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_cache_fingerprint_is_stable_sha256_hex() {
        let args = vec![json!("ord_42")];
        let schema = json!({"type": "string"});
        let req = LlmRequestRef {
            prompt: "answer",
            model: "mock",
            rendered: "Answer ord_42",
            args: &args,
            output_schema: Some(&schema),
        };

        let first = PromptCache::fingerprint(req);
        let second = PromptCache::fingerprint(req);
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
