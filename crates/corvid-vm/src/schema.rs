//! Convert a Corvid `Type` into a JSON Schema fragment.
//!
//! Used at LLM call sites: the interpreter derives the prompt's return
//! type's schema and hands it to the adapter via
//! `LlmRequest.output_schema`. Adapters (Anthropic via `tool_use`,
//! OpenAI via `response_format: json_schema`) request that the model
//! produce output matching the schema.
//!
//! The schema lives in `corvid-vm` (not `corvid-runtime`) because it
//! needs the language's `Type` enum. Runtime stays type-agnostic.

use corvid_ir::IrType;
use corvid_resolve::DefId;
use corvid_types::Type;
use serde_json::{json, Value};
use std::collections::HashMap;

/// Build a JSON Schema (Draft 2020-12 compatible subset) for `ty`.
///
/// `types_by_id` is consulted for struct types so the schema includes
/// nested object definitions inline (no `$ref`s — keeps things simple
/// and matches what providers' structured-output APIs accept best).
pub fn schema_for(ty: &Type, types_by_id: &HashMap<DefId, &IrType>) -> Value {
    schema_for_inner(ty, types_by_id, &mut Vec::new())
}

fn schema_for_inner(
    ty: &Type,
    types_by_id: &HashMap<DefId, &IrType>,
    visiting: &mut Vec<DefId>,
) -> Value {
    match ty {
        Type::Int => json!({ "type": "integer" }),
        Type::Float => json!({ "type": "number" }),
        Type::String => json!({ "type": "string" }),
        Type::Bool => json!({ "type": "boolean" }),
        Type::Nothing => json!({ "type": "null" }),
        Type::List(elem) => json!({
            "type": "array",
            "items": schema_for_inner(elem, types_by_id, visiting),
        }),
        Type::Option(inner) => json!({
            "oneOf": [
                {
                    "type": "object",
                    "properties": {
                        "tag": { "const": "some" },
                        "value": schema_for_inner(inner, types_by_id, visiting),
                    },
                    "required": ["tag", "value"],
                    "additionalProperties": false,
                },
                {
                    "type": "object",
                    "properties": {
                        "tag": { "const": "none" },
                    },
                    "required": ["tag"],
                    "additionalProperties": false,
                }
            ]
        }),
        Type::Result(ok, err) => json!({
            "oneOf": [
                {
                    "type": "object",
                    "properties": {
                        "tag": { "const": "ok" },
                        "ok": schema_for_inner(ok, types_by_id, visiting),
                    },
                    "required": ["tag", "ok"],
                    "additionalProperties": false,
                },
                {
                    "type": "object",
                    "properties": {
                        "tag": { "const": "err" },
                        "err": schema_for_inner(err, types_by_id, visiting),
                    },
                    "required": ["tag", "err"],
                    "additionalProperties": false,
                }
            ]
        }),
        Type::Weak(inner, _) => json!({
            "type": "object",
            "properties": {
                "tag": { "const": "weak" },
                "value": schema_for_inner(inner, types_by_id, visiting),
            },
            "required": ["tag", "value"],
            "additionalProperties": false,
        }),
        Type::Struct(def_id) => {
            // Cycle guard: if we're already building this struct's schema
            // higher up the stack, emit an empty object placeholder. The
            // type system doesn't actually permit recursive types in
            // v0.5, so this is defensive only.
            if visiting.contains(def_id) {
                return json!({ "type": "object" });
            }
            let Some(ir_type) = types_by_id.get(def_id).copied() else {
                return json!({ "type": "object" });
            };
            visiting.push(*def_id);
            let mut properties = serde_json::Map::new();
            let mut required = Vec::new();
            for field in &ir_type.fields {
                properties.insert(
                    field.name.clone(),
                    schema_for_inner(&field.ty, types_by_id, visiting),
                );
                required.push(Value::String(field.name.clone()));
            }
            visiting.pop();
            json!({
                "type": "object",
                "properties": properties,
                "required": required,
                "additionalProperties": false,
            })
        }
        // `Function` and `Unknown` shouldn't appear as prompt return
        // types in well-typed programs. Emit a permissive schema so the
        // adapter doesn't fail catastrophically; the type checker is the
        // real backstop.
        Type::Function { .. } => json!({}),
        Type::Unknown => json!({}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_ast::Span;
    use corvid_ir::{IrField, IrType as IrT};

    fn empty() -> HashMap<DefId, &'static IrT> {
        HashMap::new()
    }

    #[test]
    fn primitives_match_json_schema_types() {
        let by_id = empty();
        assert_eq!(schema_for(&Type::Int, &by_id), json!({"type": "integer"}));
        assert_eq!(schema_for(&Type::Float, &by_id), json!({"type": "number"}));
        assert_eq!(schema_for(&Type::String, &by_id), json!({"type": "string"}));
        assert_eq!(schema_for(&Type::Bool, &by_id), json!({"type": "boolean"}));
        assert_eq!(schema_for(&Type::Nothing, &by_id), json!({"type": "null"}));
    }

    #[test]
    fn list_emits_array_with_items() {
        let by_id = empty();
        let s = schema_for(&Type::List(Box::new(Type::String)), &by_id);
        assert_eq!(s, json!({"type": "array", "items": {"type": "string"}}));
    }

    #[test]
    fn struct_emits_object_with_required_fields() {
        let id = DefId(11);
        let ir_type: IrT = IrT {
            id,
            name: "Decision".into(),
            fields: vec![
                IrField {
                    name: "should_refund".into(),
                    ty: Type::Bool,
                    span: Span::new(0, 0),
                },
                IrField {
                    name: "reason".into(),
                    ty: Type::String,
                    span: Span::new(0, 0),
                },
            ],
            span: Span::new(0, 0),
        };
        // Leak via Box::leak so the &'static reference works in this
        // narrow test scope without lifetime gymnastics.
        let leaked: &'static IrT = Box::leak(Box::new(ir_type));
        let mut by_id: HashMap<DefId, &IrT> = HashMap::new();
        by_id.insert(id, leaked);
        let s = schema_for(&Type::Struct(id), &by_id);
        let obj = s.as_object().unwrap();
        assert_eq!(obj["type"], "object");
        assert_eq!(obj["additionalProperties"], false);
        assert_eq!(
            obj["properties"]["should_refund"],
            json!({"type": "boolean"})
        );
        assert_eq!(obj["properties"]["reason"], json!({"type": "string"}));
        let required = obj["required"].as_array().unwrap();
        assert!(required.contains(&json!("should_refund")));
        assert!(required.contains(&json!("reason")));
    }

    #[test]
    fn nested_struct_inlines_subschema() {
        let inner_id = DefId(20);
        let outer_id = DefId(21);
        let inner: &'static IrT = Box::leak(Box::new(IrT {
            id: inner_id,
            name: "Order".into(),
            fields: vec![IrField {
                name: "id".into(),
                ty: Type::String,
                span: Span::new(0, 0),
            }],
            span: Span::new(0, 0),
        }));
        let outer: &'static IrT = Box::leak(Box::new(IrT {
            id: outer_id,
            name: "Wrap".into(),
            fields: vec![IrField {
                name: "order".into(),
                ty: Type::Struct(inner_id),
                span: Span::new(0, 0),
            }],
            span: Span::new(0, 0),
        }));
        let mut by_id: HashMap<DefId, &IrT> = HashMap::new();
        by_id.insert(inner_id, inner);
        by_id.insert(outer_id, outer);
        let s = schema_for(&Type::Struct(outer_id), &by_id);
        let order_schema = &s["properties"]["order"];
        assert_eq!(order_schema["type"], "object");
        assert_eq!(order_schema["properties"]["id"], json!({"type": "string"}));
    }
}
