use crate::schema::{
    AbiEffects, AbiLatencyMs, AbiMinExpected, AbiProjectedTokens, AbiProjectedUsd,
};
use corvid_ast::DimensionValue;
use corvid_types::{ComposedProfile, EffectProfile};
use serde_json::{Number, Value};

#[allow(dead_code)]
pub fn emit_effects_from_profile(profile: &EffectProfile) -> AbiEffects {
    let mut out = AbiEffects::default();
    for (name, value) in &profile.dimensions {
        apply_dimension(&mut out, name, value);
    }
    out
}

pub fn emit_effects_from_composed(profile: &ComposedProfile) -> AbiEffects {
    let mut out = AbiEffects::default();
    for (name, value) in &profile.dimensions {
        apply_dimension(&mut out, name, value);
    }
    out
}

pub fn emit_effects_from_effect_names(
    names: &[String],
    registry: &corvid_types::EffectRegistry,
) -> AbiEffects {
    let refs = names.iter().map(|name| name.as_str()).collect::<Vec<_>>();
    let composed = registry.compose(&refs);
    emit_effects_from_composed(&composed)
}

fn apply_dimension(out: &mut AbiEffects, name: &str, value: &DimensionValue) {
    match (name, value) {
        ("cost", DimensionValue::Cost(v)) => {
            out.cost = Some(AbiProjectedUsd { projected_usd: *v });
        }
        ("trust", DimensionValue::Name(v)) => {
            out.trust_tier = Some(v.clone());
        }
        ("trust", DimensionValue::ConfidenceGated { above, .. }) => {
            out.trust_tier = Some(above.clone());
        }
        ("latency_ms", DimensionValue::Number(v)) => {
            out.latency_ms = Some(AbiLatencyMs { p99_estimate: *v });
        }
        ("reversibility" | "reversible", DimensionValue::Bool(v)) => {
            out.reversibility = Some(if *v {
                "reversible".into()
            } else {
                "non_reversible".into()
            });
        }
        ("reversibility" | "reversible", DimensionValue::Name(v)) => {
            out.reversibility = Some(v.clone());
        }
        ("data", DimensionValue::Name(v)) => {
            out.data = Some(v.clone());
        }
        ("confidence", DimensionValue::Number(v)) => {
            out.confidence = Some(AbiMinExpected { min_expected: *v });
        }
        ("tokens", DimensionValue::Number(v)) => {
            out.tokens = Some(AbiProjectedTokens { projected: *v });
        }
        (other, value) => {
            out.custom.insert(other.to_string(), dim_to_json(value));
        }
    }
}

pub(crate) fn dim_to_json(value: &DimensionValue) -> Value {
    match value {
        DimensionValue::Bool(v) => Value::Bool(*v),
        DimensionValue::Name(v) => Value::String(v.clone()),
        DimensionValue::Cost(v) | DimensionValue::Number(v) => {
            Value::Number(Number::from_f64(*v).unwrap_or_else(|| Number::from(0)))
        }
        DimensionValue::Streaming { backpressure } => {
            Value::String(format!("streaming:{backpressure:?}").to_lowercase())
        }
        DimensionValue::ConfidenceGated {
            threshold,
            above,
            below,
        } => serde_json::json!({
            "threshold": threshold,
            "above": above,
            "below": below,
        }),
    }
}
