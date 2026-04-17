//! Marshalling between Corvid `Value` and `serde_json::Value`.
//!
//! Tools and LLM adapters cross the runtime boundary as JSON. This module
//! is the only place that translation lives. The interpreter calls
//! `value_to_json` to prepare arguments for a tool/LLM call, and
//! `json_to_value` to build a `Value` from the JSON the runtime returned.
//!
//! The inbound direction (JSON → Value) needs the *expected* `Type` so
//! struct results can recover their `type_id` and `type_name`. The
//! interpreter passes the called tool's / prompt's declared return type.

use crate::value::{BoxedValue, ListValue, StructValue, Value};
use corvid_ir::IrType;
use corvid_resolve::DefId;
use corvid_types::Type;
use std::collections::HashMap;
use std::sync::Arc;

/// Convert a `Value` to a `serde_json::Value`. Lossless for primitives;
/// structs become JSON objects (the type name is dropped — the receiving
/// tool doesn't need it).
pub fn value_to_json(v: &Value) -> serde_json::Value {
    match v {
        Value::Int(n) => serde_json::Value::from(*n),
        Value::Float(f) => serde_json::Value::from(*f),
        Value::String(s) => serde_json::Value::String(s.to_string()),
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Nothing => serde_json::Value::Null,
        Value::Struct(s) => {
            let mut obj = serde_json::Map::new();
            s.with_fields(|fields| {
                for (k, v) in fields {
                    obj.insert(k.clone(), value_to_json(v));
                }
            });
            serde_json::Value::Object(obj)
        }
        Value::List(items) => {
            serde_json::Value::Array(items.iter_cloned().iter().map(value_to_json).collect())
        }
        Value::Weak(w) => match w.upgrade() {
            Some(value) => serde_json::json!({ "tag": "weak", "value": value_to_json(&value) }),
            None => serde_json::json!({ "tag": "weak", "value": serde_json::Value::Null }),
        },
        Value::ResultOk(v) => serde_json::json!({ "tag": "ok", "ok": value_to_json(&v.get()) }),
        Value::ResultErr(v) => serde_json::json!({ "tag": "err", "err": value_to_json(&v.get()) }),
        Value::OptionSome(v) => serde_json::json!({ "tag": "some", "value": value_to_json(&v.get()) }),
        Value::OptionNone => serde_json::json!({ "tag": "none" }),
        Value::Grounded(g) => {
            let inner = value_to_json(&g.inner.get());
            let sources: Vec<serde_json::Value> = g.provenance.entries.iter().map(|e| {
                serde_json::json!({
                    "kind": e.kind.label(),
                    "name": e.name,
                    "timestamp_ms": e.timestamp_ms,
                })
            }).collect();
            serde_json::json!({ "tag": "grounded", "value": inner, "sources": sources })
        }
    }
}

/// Convert a `serde_json::Value` to a `Value`, guided by the `expected`
/// type. The type table `types_by_id` is consulted when the expected
/// type is a struct so the rebuilt `StructValue` carries the right
/// `type_id` and `type_name`.
pub fn json_to_value(
    json: serde_json::Value,
    expected: &Type,
    types_by_id: &HashMap<DefId, &IrType>,
) -> Result<Value, ConvError> {
    use serde_json::Value as J;
    match (expected, json) {
        (Type::Int, J::Number(n)) => n
            .as_i64()
            .map(Value::Int)
            .ok_or_else(|| ConvError::TypeMismatch {
                expected: "Int".into(),
                got: "non-integer number".into(),
            }),
        // Float absorbs both JSON floats and JSON integers (LLMs often
        // emit `1` where a float field is declared).
        (Type::Float, J::Number(n)) => n
            .as_f64()
            .map(Value::Float)
            .ok_or_else(|| ConvError::TypeMismatch {
                expected: "Float".into(),
                got: "non-float number".into(),
            }),
        (Type::String, J::String(s)) => Ok(Value::String(Arc::from(s))),
        (Type::Bool, J::Bool(b)) => Ok(Value::Bool(b)),
        (Type::Nothing, J::Null) => Ok(Value::Nothing),
        // Some tools/LLMs return `null` for any "absent" value. Honour it
        // for `Nothing` returns; reject elsewhere.
        (_, J::Null) => Err(ConvError::TypeMismatch {
            expected: type_label(expected),
            got: "null".into(),
        }),
        (Type::List(elem_ty), J::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(json_to_value(item, elem_ty, types_by_id)?);
            }
            Ok(Value::List(ListValue::new(out)))
        }
        (Type::Option(inner_ty), J::Object(map)) => match map.get("tag").and_then(|v| v.as_str()) {
            Some("some") => {
                let raw = map.get("value").cloned().ok_or_else(|| ConvError::TypeMismatch {
                    expected: "Option::Some payload".into(),
                    got: "missing `value` field".into(),
                })?;
                Ok(Value::OptionSome(BoxedValue::new(json_to_value(raw, inner_ty, types_by_id)?)))
            }
            Some("none") => Ok(Value::OptionNone),
            _ => Err(ConvError::TypeMismatch {
                expected: type_label(expected),
                got: "object".into(),
            }),
        },
        (Type::Result(ok_ty, err_ty), J::Object(map)) => {
            match map.get("tag").and_then(|v| v.as_str()) {
                Some("ok") => {
                    let raw = map.get("ok").cloned().ok_or_else(|| ConvError::TypeMismatch {
                        expected: "Result::Ok payload".into(),
                        got: "missing `ok` field".into(),
                    })?;
                    Ok(Value::ResultOk(BoxedValue::new(json_to_value(raw, ok_ty, types_by_id)?)))
                }
                Some("err") => {
                    let raw = map.get("err").cloned().ok_or_else(|| ConvError::TypeMismatch {
                        expected: "Result::Err payload".into(),
                        got: "missing `err` field".into(),
                    })?;
                    Ok(Value::ResultErr(BoxedValue::new(json_to_value(raw, err_ty, types_by_id)?)))
                }
                _ => Err(ConvError::TypeMismatch {
                    expected: type_label(expected),
                    got: "object".into(),
                }),
            }
        }
        (Type::Struct(def_id), J::Object(map)) => {
            let ir_type = types_by_id
                .get(def_id)
                .copied()
                .ok_or(ConvError::UnknownStructType(*def_id))?;
            let mut fields = HashMap::new();
            for field in &ir_type.fields {
                let raw = map
                    .get(&field.name)
                    .cloned()
                    .ok_or_else(|| ConvError::MissingField {
                        struct_name: ir_type.name.clone(),
                        field: field.name.clone(),
                    })?;
                let v = json_to_value(raw, &field.ty, types_by_id)?;
                fields.insert(field.name.clone(), v);
            }
            Ok(Value::Struct(StructValue::new(
                ir_type.id,
                ir_type.name.clone(),
                fields,
            )))
        }
        // `Unknown` accepts any JSON, lossy. Used as a graceful fallback.
        (Type::Unknown, json) => Ok(json_to_value_loose(json)),
        (expected, got) => Err(ConvError::TypeMismatch {
            expected: type_label(expected),
            got: json_kind(&got).into(),
        }),
    }
}

/// Best-effort JSON → Value conversion when the expected type is unknown.
/// Used as a fallback path; never produces structs (no type_id available).
fn json_to_value_loose(json: serde_json::Value) -> Value {
    use serde_json::Value as J;
    match json {
        J::Null => Value::Nothing,
        J::Bool(b) => Value::Bool(b),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(f) = n.as_f64() {
                Value::Float(f)
            } else {
                Value::Nothing
            }
        }
        J::String(s) => Value::String(Arc::from(s)),
        J::Array(items) => Value::List(ListValue::new(items.into_iter().map(json_to_value_loose).collect::<Vec<_>>())),
        J::Object(_) => {
            // Without a type, we can't rebuild a Struct. Drop to Nothing
            // and let the interpreter surface a clean error if the value
            // is used.
            Value::Nothing
        }
    }
}

fn type_label(t: &Type) -> String {
    match t {
        Type::Int => "Int".into(),
        Type::Float => "Float".into(),
        Type::String => "String".into(),
        Type::Bool => "Bool".into(),
        Type::Nothing => "Nothing".into(),
        Type::Struct(_) => "struct".into(),
        Type::List(elem) => format!("List<{}>", type_label(elem)),
        Type::Result(ok, err) => format!("Result<{}, {}>", type_label(ok), type_label(err)),
        Type::Option(inner) => format!("Option<{}>", type_label(inner)),
        Type::Weak(inner, effects) => {
            if effects.is_any() {
                format!("Weak<{}>", type_label(inner))
            } else {
                let effect_names: Vec<&'static str> = effects
                    .effects()
                    .into_iter()
                    .map(|effect| match effect {
                        corvid_ast::WeakEffect::ToolCall => "tool_call",
                        corvid_ast::WeakEffect::Llm => "llm",
                        corvid_ast::WeakEffect::Approve => "approve",
                    })
                    .collect();
                format!("Weak<{}, {{{}}}>", type_label(inner), effect_names.join(", "))
            }
        }
        Type::Grounded(inner) => format!("Grounded<{}>", type_label(inner)),
        Type::Function { .. } => "function".into(),
        Type::Unknown => "<unknown>".into(),
    }
}

fn json_kind(j: &serde_json::Value) -> &'static str {
    match j {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

#[derive(Debug, Clone)]
pub enum ConvError {
    TypeMismatch { expected: String, got: String },
    MissingField { struct_name: String, field: String },
    UnknownStructType(DefId),
}

impl std::fmt::Display for ConvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TypeMismatch { expected, got } => {
                write!(f, "expected `{expected}`, got `{got}`")
            }
            Self::MissingField { struct_name, field } => {
                write!(f, "field `{field}` missing on `{struct_name}`")
            }
            Self::UnknownStructType(id) => {
                write!(f, "no IR type registered for DefId({})", id.0)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn primitives_roundtrip() {
        let cases = [
            (Value::Int(42), json!(42)),
            (Value::Float(1.5), json!(1.5)),
            (Value::String(Arc::from("hi")), json!("hi")),
            (Value::Bool(true), json!(true)),
            (Value::Nothing, json!(null)),
        ];
        let empty: HashMap<DefId, &IrType> = HashMap::new();
        for (v, j) in cases {
            assert_eq!(value_to_json(&v), j.clone());
            let typ = match &v {
                Value::Int(_) => Type::Int,
                Value::Float(_) => Type::Float,
                Value::String(_) => Type::String,
                Value::Bool(_) => Type::Bool,
                Value::Nothing => Type::Nothing,
                _ => unreachable!(),
            };
            assert_eq!(json_to_value(j, &typ, &empty).unwrap(), v);
        }
    }

    #[test]
    fn list_roundtrips() {
        let v = Value::List(ListValue::new(vec![Value::Int(1), Value::Int(2)]));
        let j = value_to_json(&v);
        assert_eq!(j, json!([1, 2]));
        let empty: HashMap<DefId, &IrType> = HashMap::new();
        let back = json_to_value(j, &Type::List(Box::new(Type::Int)), &empty).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn struct_rebuilds_from_json() {
        // Build a fake IrType for `Decision { should_refund: Bool }`.
        let id = DefId(7);
        let ir_type = IrType {
            id,
            name: "Decision".into(),
            fields: vec![corvid_ir::IrField {
                name: "should_refund".into(),
                ty: Type::Bool,
                span: corvid_ast::Span::new(0, 0),
            }],
            span: corvid_ast::Span::new(0, 0),
        };
        let mut by_id = HashMap::new();
        by_id.insert(id, &ir_type);

        let json = json!({"should_refund": true});
        let v = json_to_value(json, &Type::Struct(id), &by_id).unwrap();
        match v {
            Value::Struct(s) => {
                assert_eq!(s.type_name(), "Decision");
                assert_eq!(s.type_id(), id);
                assert_eq!(s.get_field("should_refund").unwrap(), Value::Bool(true));
            }
            other => panic!("expected struct, got {other:?}"),
        }
    }

    #[test]
    fn missing_field_errors() {
        let id = DefId(1);
        let ir_type = IrType {
            id,
            name: "X".into(),
            fields: vec![corvid_ir::IrField {
                name: "needed".into(),
                ty: Type::Int,
                span: corvid_ast::Span::new(0, 0),
            }],
            span: corvid_ast::Span::new(0, 0),
        };
        let mut by_id = HashMap::new();
        by_id.insert(id, &ir_type);
        let err = json_to_value(json!({}), &Type::Struct(id), &by_id).unwrap_err();
        assert!(matches!(err, ConvError::MissingField { .. }));
    }
}
