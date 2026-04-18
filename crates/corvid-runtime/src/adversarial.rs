use crate::RuntimeError;

/// Extract the compiler-guaranteed `contradiction: Bool` field from an
/// adjudicator verdict encoded as JSON.
pub fn contradiction_flag(
    prompt: &str,
    verdict: &serde_json::Value,
) -> Result<bool, RuntimeError> {
    let serde_json::Value::Object(fields) = verdict else {
        return Err(RuntimeError::InvalidAdversarialVerdict {
            prompt: prompt.to_string(),
            message: format!("expected struct-like JSON object, got {verdict}"),
        });
    };
    let Some(flag) = fields.get("contradiction") else {
        return Err(RuntimeError::InvalidAdversarialVerdict {
            prompt: prompt.to_string(),
            message: "missing `contradiction` field".to_string(),
        });
    };
    let Some(flag) = flag.as_bool() else {
        return Err(RuntimeError::InvalidAdversarialVerdict {
            prompt: prompt.to_string(),
            message: format!("`contradiction` must be Bool, got {flag}"),
        });
    };
    Ok(flag)
}

/// Human-readable text for trace payloads. Strings stay unquoted;
/// everything else falls back to JSON.
pub fn trace_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{contradiction_flag, trace_text};
    use crate::RuntimeError;
    use serde_json::json;

    #[test]
    fn contradiction_flag_reads_boolean_field() {
        let verdict = json!({ "contradiction": true, "rationale": "bad" });
        assert!(contradiction_flag("verify", &verdict).unwrap());
    }

    #[test]
    fn contradiction_flag_rejects_missing_field() {
        let verdict = json!({ "rationale": "bad" });
        assert!(matches!(
            contradiction_flag("verify", &verdict),
            Err(RuntimeError::InvalidAdversarialVerdict { .. })
        ));
    }

    #[test]
    fn trace_text_preserves_strings() {
        assert_eq!(trace_text(&json!("hello")), "hello");
        assert_eq!(trace_text(&json!({"x": 1})), "{\"x\":1}");
    }
}
