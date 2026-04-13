//! Secret redaction for trace events.
//!
//! Trace events contain user data — tool args, LLM args, approval args.
//! Some of that data may be a secret (an API key the user passed to a
//! tool, a token derived from an env var). We can't catch every leak,
//! but we can catch the obvious one: the value of any env var whose
//! name ends in `_KEY` / `_TOKEN` / `_SECRET` / `_PASSWORD`. Match those
//! values anywhere they appear inside event payloads and replace with
//! `<redacted>`.
//!
//! Built once at runtime startup (so we don't re-scan the environment
//! per event) and stored on the `Tracer`.

use serde_json::Value;
use std::collections::HashSet;

const SUFFIXES: &[&str] = &["_KEY", "_TOKEN", "_SECRET", "_PASSWORD"];

/// Set of secret strings that should be redacted on sight.
#[derive(Clone, Debug, Default)]
pub struct RedactionSet {
    secrets: HashSet<String>,
}

impl RedactionSet {
    /// Build from the current process environment.
    pub fn from_env() -> Self {
        let mut secrets = HashSet::new();
        for (name, value) in std::env::vars() {
            if !value.is_empty() && name_looks_secret(&name) {
                secrets.insert(value);
            }
        }
        Self { secrets }
    }

    /// Empty set — used by tests that want to verify the no-redact path.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Manually mark a value as secret. For testing and for values that
    /// don't follow the suffix convention.
    pub fn add(&mut self, value: impl Into<String>) {
        let v = value.into();
        if !v.is_empty() {
            self.secrets.insert(v);
        }
    }

    /// Whether anything is registered.
    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty()
    }

    /// Walk a JSON value and redact any string equal to a known secret.
    /// Returns a new `Value`; the original is not mutated.
    pub fn redact(&self, value: Value) -> Value {
        if self.secrets.is_empty() {
            return value;
        }
        match value {
            Value::String(s) => {
                if self.secrets.contains(&s) {
                    Value::String("<redacted>".into())
                } else {
                    Value::String(s)
                }
            }
            Value::Array(items) => {
                Value::Array(items.into_iter().map(|v| self.redact(v)).collect())
            }
            Value::Object(map) => {
                let mut out = serde_json::Map::with_capacity(map.len());
                for (k, v) in map {
                    out.insert(k, self.redact(v));
                }
                Value::Object(out)
            }
            other => other,
        }
    }

    /// In-place redaction of a vector of args. Convenience for the
    /// `Tracer::emit` path which holds owned arg arrays.
    pub fn redact_args(&self, args: Vec<Value>) -> Vec<Value> {
        if self.secrets.is_empty() {
            return args;
        }
        args.into_iter().map(|v| self.redact(v)).collect()
    }
}

fn name_looks_secret(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SUFFIXES.iter().any(|s| upper.ends_with(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn name_pattern_matches_common_secret_suffixes() {
        assert!(name_looks_secret("ANTHROPIC_API_KEY"));
        assert!(name_looks_secret("OPENAI_API_KEY"));
        assert!(name_looks_secret("GITHUB_TOKEN"));
        assert!(name_looks_secret("DATABASE_PASSWORD"));
        assert!(name_looks_secret("STRIPE_SECRET"));
        assert!(!name_looks_secret("CORVID_MODEL"));
        assert!(!name_looks_secret("PATH"));
        assert!(!name_looks_secret("HOME"));
    }

    #[test]
    fn empty_set_does_not_modify_values() {
        let s = RedactionSet::empty();
        let v = json!({"x": "anything"});
        assert_eq!(s.redact(v.clone()), v);
    }

    #[test]
    fn known_secret_strings_get_replaced() {
        let mut s = RedactionSet::empty();
        s.add("sk-test-12345");
        s.add("hunter2");
        let v = json!({
            "args": ["sk-test-12345", "ord_42", {"nested": "hunter2"}],
            "ok": true,
        });
        let out = s.redact(v);
        assert_eq!(out["args"][0], "<redacted>");
        assert_eq!(out["args"][1], "ord_42");
        assert_eq!(out["args"][2]["nested"], "<redacted>");
        assert_eq!(out["ok"], true);
    }

    #[test]
    fn from_env_picks_up_suffixed_vars() {
        let key = "TEST_REDACT_API_KEY";
        let value = "this-is-secret-for-testing";
        std::env::set_var(key, value);
        let s = RedactionSet::from_env();
        assert!(s.secrets.contains(value));
        std::env::remove_var(key);
    }

    #[test]
    fn redact_args_returns_redacted_vector() {
        let mut s = RedactionSet::empty();
        s.add("topsecret");
        let args = vec![json!("topsecret"), json!("plain")];
        let out = s.redact_args(args);
        assert_eq!(out, vec![json!("<redacted>"), json!("plain")]);
    }
}
