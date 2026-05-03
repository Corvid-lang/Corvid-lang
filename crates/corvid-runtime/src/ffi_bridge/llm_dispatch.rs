use super::*;
use crate::llm::LlmRequestRef;
use corvid_trace_schema::TraceEvent;
use std::sync::atomic::Ordering;
use std::sync::OnceLock;

pub(super) fn trace_mock_llm_attempt(
    state: &BridgeState,
    prompt_name: &str,
    model: &str,
    rendered: &str,
    args: &[serde_json::Value],
    result: serde_json::Value,
) {
    let runtime = state.corvid_runtime();
    let tracer = runtime.tracer();
    if !tracer.is_enabled() {
        return;
    }
    let effective_model = if model.is_empty() {
        runtime.default_model()
    } else {
        model
    };
    tracer.emit(TraceEvent::LlmCall {
        ts_ms: crate::tracing::now_ms(),
        run_id: tracer.run_id().to_string(),
        prompt: prompt_name.to_string(),
        model: if effective_model.is_empty() {
            None
        } else {
            Some(effective_model.to_string())
        },
        model_version: runtime.model_version(effective_model),
        rendered: Some(rendered.to_string()),
        args: args.to_vec(),
    });
    tracer.emit(TraceEvent::LlmResult {
        ts_ms: crate::tracing::now_ms(),
        run_id: tracer.run_id().to_string(),
        prompt: prompt_name.to_string(),
        model: if effective_model.is_empty() {
            None
        } else {
            Some(effective_model.to_string())
        },
        model_version: runtime.model_version(effective_model),
        result,
    });
}

/// Default retry count when `CORVID_PROMPT_MAX_RETRIES` env is unset.
const DEFAULT_PROMPT_MAX_RETRIES: u32 = 3;

pub(super) fn prompt_max_retries() -> u32 {
    static VALUE: OnceLock<u32> = OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("CORVID_PROMPT_MAX_RETRIES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_PROMPT_MAX_RETRIES)
    })
}

/// Parse helpers — tolerant of common LLM quirks (surrounding quotes,
/// whitespace, code-fence wrappers).
fn strip_response(s: &str) -> &str {
    let t = s.trim();
    // Strip a single layer of code-fence: ```...```, ```rust...```, ```\n...```
    if t.starts_with("```") && t.ends_with("```") && t.len() >= 6 {
        let inner = &t[3..t.len() - 3];
        // Trim a leading language tag like ```rust\n...
        let after_lang = inner.find('\n').map(|nl| &inner[nl + 1..]).unwrap_or(inner);
        return after_lang.trim();
    }
    t
}

pub(super) fn parse_int(s: &str) -> Option<i64> {
    let t = strip_response(s);
    let t = t.trim_matches(|c: char| c == '"' || c == '\'').trim();
    t.parse::<i64>().ok()
}

pub(super) fn parse_bool(s: &str) -> Option<bool> {
    let t = strip_response(s)
        .trim()
        .trim_matches(|c: char| c == '"' || c == '\'');
    match t.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

pub(super) fn parse_float(s: &str) -> Option<f64> {
    let t = strip_response(s)
        .trim()
        .trim_matches(|c: char| c == '"' || c == '\'');
    t.parse::<f64>().ok()
}

/// Format-instruction text per return type. Sent in the system prompt.
pub(super) fn format_instruction_int() -> &'static str {
    "Output only a single integer literal — no quotes, no explanation, no formatting, no thousands separators. Examples: 42, -7, 0."
}

pub(super) fn format_instruction_bool() -> &'static str {
    "Output only the word `true` or `false` — lowercase, no quotes, no explanation, no surrounding text."
}

pub(super) fn format_instruction_float() -> &'static str {
    "Output only a single decimal number — no quotes, no explanation, no scientific notation prefix beyond what `f64::parse` accepts. Examples: 3.14, -0.5, 42.0."
}

pub(super) fn using_env_mock_llm() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    let enabled =
        *ENABLED.get_or_init(|| std::env::var("CORVID_TEST_MOCK_LLM").ok().as_deref() == Some("1"));
    enabled
        && !BRIDGE.load(Ordering::Acquire).is_null()
        && !bridge().corvid_runtime().is_replay_mode()
}

/// Build the system prompt sent to the LLM. Encodes the function
/// signature + return-type instruction + (after retries) escalating
/// reminders. `attempt` is 0-indexed; `last_failure` is `Some(text)`
/// on retry attempts and contains the LLM's previous (unparseable)
/// response.
pub(super) fn build_system_prompt(
    signature: &str,
    format_instruction: &str,
    attempt: u32,
    last_failure: Option<&str>,
) -> String {
    let mut sys = format!(
        "You are a function with signature `{signature}`. The user message contains the rendered prompt body. Compute and return the appropriate value, formatted as follows.\n\nFormat: {format_instruction}"
    );
    if attempt > 0 {
        if let Some(prev) = last_failure {
            sys.push_str(&format!(
                "\n\nIMPORTANT: Your previous response `{prev}` could not be parsed. Respond with ONLY the value in the exact format described above — nothing else, no surrounding text, no explanation."
            ));
        }
        if attempt >= 2 {
            sys.push_str("\n\nThis is your last attempt. The format requirements are absolute.");
        }
    }
    sys
}

/// Single LLM call within the retry loop. Returns the response text
/// (not the parsed value — parsing happens per-return-type in each
/// bridge).
pub(super) fn call_llm_once(
    state: &BridgeState,
    prompt_name: &str,
    model: &str,
    rendered: &str,
    args: &[serde_json::Value],
    system_prompt: &str,
) -> Result<String, String> {
    let runtime = state.corvid_runtime();
    let combined =
        if using_env_mock_llm() || (runtime.is_replay_mode() && !runtime.replay_uses_live_llm()) {
            rendered.to_owned()
        } else {
            // Combine system prompt + user-side rendered prompt with two
            // newlines. Adapters that have native system-prompt support could
            // separate these later; for now the concat is universal.
            let mut combined = String::with_capacity(system_prompt.len() + 2 + rendered.len());
            combined.push_str(system_prompt);
            combined.push_str("\n\n");
            combined.push_str(rendered);
            combined
        };
    let req = LlmRequestRef {
        prompt: prompt_name,
        model,
        rendered: &combined,
        args,
        output_schema: None,
    };
    let resp = state.tokio_handle().block_on(async move {
        runtime
            .call_llm_ref_with_trace_rendered(req, Some(rendered))
            .await
    });
    match resp {
        Ok(r) => match r.value {
            serde_json::Value::String(s) => Ok(s),
            other => Ok(other.to_string()),
        },
        Err(e) => {
            panic_if_replay_runtime_error(
                &format!("corvid prompt `{prompt_name}` (model `{model}`) replay failed"),
                &e,
            );
            Err(format!("{e}"))
        }
    }
}
