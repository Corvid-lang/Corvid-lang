use crate::errors::RuntimeError;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct CacheKeyInput {
    pub namespace: String,
    pub subject: String,
    pub model: Option<String>,
    pub effect_key: Option<String>,
    pub provenance_key: Option<String>,
    pub version: Option<String>,
    pub args: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheKey {
    pub namespace: String,
    pub subject: String,
    pub fingerprint: String,
    pub effect_key: Option<String>,
    pub provenance_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntryMetadata {
    pub key: CacheKey,
    pub replay_safe: bool,
    pub invalidation_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CacheEntry {
    pub metadata: CacheEntryMetadata,
    pub value: Value,
}

#[derive(Debug, Default, Clone)]
pub struct CacheRuntime {
    entries: HashMap<String, CacheEntry>,
}

impl CacheRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn put(
        &mut self,
        key: CacheKey,
        value: Value,
        replay_safe: bool,
        invalidation_key: Option<String>,
    ) -> CacheEntry {
        let entry = CacheEntry {
            metadata: cache_entry_metadata(key.clone(), replay_safe, invalidation_key),
            value,
        };
        self.entries.insert(entry_id(&key), entry.clone());
        entry
    }

    pub fn get(&self, key: &CacheKey) -> Option<&CacheEntry> {
        self.entries.get(&entry_id(key))
    }

    pub fn invalidate_invalidation_key(&mut self, invalidation_key: &str) -> usize {
        self.retain(|entry| entry.metadata.invalidation_key.as_deref() != Some(invalidation_key))
    }

    pub fn invalidate_provenance_key(&mut self, provenance_key: &str) -> usize {
        self.retain(|entry| entry.metadata.key.provenance_key.as_deref() != Some(provenance_key))
    }

    fn retain(&mut self, predicate: impl Fn(&CacheEntry) -> bool) -> usize {
        let before = self.entries.len();
        self.entries.retain(|_, entry| predicate(entry));
        before - self.entries.len()
    }
}

pub fn build_cache_key(input: CacheKeyInput) -> Result<CacheKey, RuntimeError> {
    if input.namespace.trim().is_empty() {
        return Err(RuntimeError::Other(
            "std.cache key namespace must not be empty".to_string(),
        ));
    }
    if input.subject.trim().is_empty() {
        return Err(RuntimeError::Other(
            "std.cache key subject must not be empty".to_string(),
        ));
    }
    let canonical = serde_json::json!({
        "namespace": input.namespace,
        "subject": input.subject,
        "model": input.model,
        "effect_key": input.effect_key,
        "provenance_key": input.provenance_key,
        "version": input.version,
        "args": input.args,
    });
    let namespace = canonical["namespace"].as_str().unwrap_or_default().to_string();
    let subject = canonical["subject"].as_str().unwrap_or_default().to_string();
    let effect_key = canonical["effect_key"].as_str().map(str::to_string);
    let provenance_key = canonical["provenance_key"].as_str().map(str::to_string);
    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string().as_bytes());
    Ok(CacheKey {
        namespace,
        subject,
        fingerprint: encode_hex(&hasher.finalize()),
        effect_key,
        provenance_key,
    })
}

pub fn cache_entry_metadata(
    key: CacheKey,
    replay_safe: bool,
    invalidation_key: Option<String>,
) -> CacheEntryMetadata {
    CacheEntryMetadata {
        key,
        replay_safe,
        invalidation_key,
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

fn entry_id(key: &CacheKey) -> String {
    format!("{}:{}:{}", key.namespace, key.subject, key.fingerprint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_is_stable_and_effect_aware() {
        let input = CacheKeyInput {
            namespace: "prompt".to_string(),
            subject: "answer".to_string(),
            model: Some("gpt".to_string()),
            effect_key: Some("llm:hosted".to_string()),
            provenance_key: Some("doc:abc".to_string()),
            version: Some("v1".to_string()),
            args: serde_json::json!({"q": "hello"}),
        };

        let first = build_cache_key(input.clone()).unwrap();
        let second = build_cache_key(input).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.fingerprint.len(), 64);
        assert_eq!(first.effect_key.as_deref(), Some("llm:hosted"));
        assert_eq!(first.provenance_key.as_deref(), Some("doc:abc"));
    }

    #[test]
    fn cache_key_rejects_empty_namespace() {
        let err = build_cache_key(CacheKeyInput {
            namespace: "".to_string(),
            subject: "answer".to_string(),
            model: None,
            effect_key: None,
            provenance_key: None,
            version: None,
            args: serde_json::json!(null),
        })
        .unwrap_err();

        assert!(err.to_string().contains("namespace"));
    }

    #[test]
    fn cache_runtime_preserves_provenance_and_invalidates_by_policy_key() {
        let key = build_cache_key(CacheKeyInput {
            namespace: "prompt".to_string(),
            subject: "answer".to_string(),
            model: Some("gpt".to_string()),
            effect_key: Some("llm:hosted".to_string()),
            provenance_key: Some("doc:123".to_string()),
            version: Some("v1".to_string()),
            args: serde_json::json!({"q": "hello"}),
        })
        .unwrap();
        let mut cache = CacheRuntime::new();

        let entry = cache.put(
            key.clone(),
            serde_json::json!({"text": "hi"}),
            true,
            Some("prompt:v1".to_string()),
        );

        assert_eq!(
            cache
                .get(&key)
                .and_then(|stored| stored.metadata.key.provenance_key.as_deref()),
            Some("doc:123")
        );
        assert_eq!(entry.metadata.invalidation_key.as_deref(), Some("prompt:v1"));

        let removed = cache.invalidate_invalidation_key("prompt:v1");
        assert_eq!(removed, 1);
        assert!(cache.get(&key).is_none());
    }

    #[test]
    fn cache_runtime_invalidates_by_provenance_key() {
        let key = build_cache_key(CacheKeyInput {
            namespace: "tool".to_string(),
            subject: "lookup".to_string(),
            model: None,
            effect_key: Some("network".to_string()),
            provenance_key: Some("doc:stale".to_string()),
            version: None,
            args: serde_json::json!({"id": 1}),
        })
        .unwrap();
        let mut cache = CacheRuntime::new();
        cache.put(key.clone(), serde_json::json!({"ok": true}), false, None);

        let removed = cache.invalidate_provenance_key("doc:stale");
        assert_eq!(removed, 1);
        assert!(cache.get(&key).is_none());
    }
}
