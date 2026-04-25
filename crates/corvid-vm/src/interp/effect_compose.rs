use super::Interpreter;
use crate::errors::{InterpError, InterpErrorKind};
use crate::value::{GroundedValue, Value};
use crate::ProvenanceChain;
use crate::value_to_json;
use corvid_ast::{BackpressurePolicy, Span};
use corvid_ir::IrPrompt;

pub(super) fn overflow(span: Span) -> InterpError {
    InterpError::new(
        InterpErrorKind::Arithmetic("integer overflow".into()),
        span,
    )
}

pub(super) fn composed_confidence(args: &[Value]) -> f64 {
    let mut min_conf = 1.0_f64;
    for arg in args {
        if let Value::Grounded(g) = arg {
            if g.confidence < min_conf {
                min_conf = g.confidence;
            }
        }
    }
    min_conf
}

#[allow(dead_code)]
pub(super) fn force_use(i: &Interpreter<'_>) {
    let _ = &i.ir;
    let _ = &i.types_by_id;
}

pub(super) fn default_stream_backpressure() -> BackpressurePolicy {
    BackpressurePolicy::Bounded(16)
}

pub(super) fn estimate_tokens(text: &str) -> u64 {
    let chars = text.chars().count() as u64;
    if chars == 0 {
        0
    } else {
        (chars / 4).max(1)
    }
}

pub(super) fn prompt_backpressure(prompt: &IrPrompt) -> BackpressurePolicy {
    prompt
        .backpressure
        .clone()
        .unwrap_or_else(default_stream_backpressure)
}

pub(super) fn prompt_effective_confidence(prompt: &IrPrompt, value: &Value) -> f64 {
    let value_confidence = match value {
        Value::Grounded(g) => g.confidence,
        _ => 1.0,
    };
    prompt.effect_confidence.min(value_confidence)
}

pub(super) fn stream_start_is_retryable(value: &Value) -> bool {
    matches!(value, Value::ResultErr(_) | Value::OptionNone)
}

pub(super) fn vote_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.to_string(),
        Value::Grounded(g) => vote_text(&g.inner.get()),
        other => value_to_json(other).to_string(),
    }
}

pub(super) fn with_value_confidence(value: Value, confidence: f64) -> Value {
    match value {
        Value::Grounded(g) => Value::Grounded(GroundedValue::with_confidence(
            g.inner.get(),
            g.provenance.clone(),
            confidence,
        )),
        other if confidence < 1.0 => {
            Value::Grounded(GroundedValue::with_confidence(
                other,
                ProvenanceChain::new(),
                confidence,
            ))
        }
        other => other,
    }
}
