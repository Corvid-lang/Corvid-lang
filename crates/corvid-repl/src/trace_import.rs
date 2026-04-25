//! Trace import: create mock tool/prompt declarations and runtime
//! handlers from a recorded JSONL execution trace. The imported mocks
//! replay recorded results when called with matching inputs, giving
//! the REPL user a live development surface against production data
//! without the production infrastructure.

use corvid_ast::{
    Decl, EffectRow, Field, Ident, Param, PromptDecl, Span, ToolDecl, TypeDecl, TypeRef,
    Visibility,
};
use corvid_runtime::{Runtime, RuntimeError, TraceEvent};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// A recorded tool call with args and result.
#[derive(Debug, Clone)]
pub struct RecordedToolCall {
    pub args: Vec<JsonValue>,
    pub result: JsonValue,
}

/// A recorded prompt call with rendered text and result.
#[derive(Debug, Clone)]
pub struct RecordedPromptCall {
    pub args: Vec<JsonValue>,
    pub rendered: Option<String>,
    pub result: JsonValue,
}

/// Extracted mock data from a trace file.
#[derive(Debug, Clone, Default)]
pub struct TraceMocks {
    pub tools: HashMap<String, Vec<RecordedToolCall>>,
    pub prompts: HashMap<String, Vec<RecordedPromptCall>>,
}

/// Result of importing a trace: declarations to add + a new runtime.
pub struct TraceImportResult {
    pub decls: Vec<Decl>,
    pub runtime: Runtime,
    pub tool_count: usize,
    pub prompt_count: usize,
    pub tool_names: Vec<String>,
    pub prompt_names: Vec<String>,
}

/// Load a JSONL trace file and extract mock data.
pub fn load_trace(path: &Path) -> Result<TraceMocks, String> {
    let body = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read `{}`: {e}", path.display()))?;

    let mut events = Vec::new();
    for (i, line) in body.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let event: TraceEvent = serde_json::from_str(line)
            .map_err(|e| format!("invalid JSONL at line {}: {e}", i + 1))?;
        events.push(event);
    }

    let mut mocks = TraceMocks::default();

    let mut i = 0;
    while i < events.len() {
        match &events[i] {
            TraceEvent::ToolCall { tool, args, .. } => {
                if let Some(TraceEvent::ToolResult { result, tool: rt, .. }) = events.get(i + 1) {
                    if rt == tool {
                        mocks.tools.entry(tool.clone()).or_default().push(RecordedToolCall {
                            args: args.clone(),
                            result: result.clone(),
                        });
                        i += 2;
                        continue;
                    }
                }
                i += 1;
            }
            TraceEvent::LlmCall { prompt, args, rendered, .. } => {
                if let Some(TraceEvent::LlmResult { result, prompt: rp, .. }) = events.get(i + 1) {
                    if rp == prompt {
                        mocks.prompts.entry(prompt.clone()).or_default().push(RecordedPromptCall {
                            args: args.clone(),
                            rendered: rendered.clone(),
                            result: result.clone(),
                        });
                        i += 2;
                        continue;
                    }
                }
                i += 1;
            }
            _ => { i += 1; }
        }
    }

    Ok(mocks)
}

/// Build declarations and a runtime from extracted trace mocks.
pub fn build_import(mocks: &TraceMocks) -> TraceImportResult {
    let sp = Span::new(0, 0);
    let mut decls: Vec<Decl> = Vec::new();
    let mut builder = Runtime::builder();

    let tool_names: Vec<String> = mocks.tools.keys().cloned().collect();
    let prompt_names: Vec<String> = mocks.prompts.keys().cloned().collect();

    // Generate tool declarations and mock handlers.
    for (name, calls) in &mocks.tools {
        let first = &calls[0];

        // Infer param types from the first recorded call's args.
        let mut type_decls = Vec::new();
        let params = infer_params(&first.args, name, "arg", &sp, &mut type_decls);

        // Infer return type from the first recorded result.
        let (return_ty, ret_type_decls) = infer_typeref(&first.result, &to_pascal(name), "Result", &sp);
        type_decls.extend(ret_type_decls);

        for td in type_decls {
            decls.push(Decl::Type(td));
        }

        decls.push(Decl::Tool(ToolDecl {
            name: Ident::new(name.clone(), sp),
            params,
            return_ty,
            return_ownership: None,
            effect: corvid_ast::Effect::Safe,
            effect_row: EffectRow::default(),
            visibility: Visibility::Private,
            span: sp,
        }));

        // Register mock handler.
        let recorded = calls.clone();
        builder = builder.tool(name.clone(), move |args| {
            let recorded = recorded.clone();
            async move {
                for call in &recorded {
                    if json_args_match(&call.args, &args) {
                        return Ok(call.result.clone());
                    }
                }
                // No exact match — return the first recorded result with a
                // warning (better than failing; the user can override via
                // step-through if needed).
                if let Some(first) = recorded.first() {
                    return Ok(first.result.clone());
                }
                Err(RuntimeError::ToolFailed {
                    tool: "trace-mock".into(),
                    message: "no recorded result available".into(),
                })
            }
        });
    }

    // Generate prompt declarations and mock LLM responses.
    for (name, calls) in &mocks.prompts {
        let first = &calls[0];

        let mut type_decls = Vec::new();
        let params = infer_params(&first.args, name, "arg", &sp, &mut type_decls);
        let (return_ty, ret_type_decls) = infer_typeref(&first.result, &to_pascal(name), "Result", &sp);
        type_decls.extend(ret_type_decls);

        for td in type_decls {
            decls.push(Decl::Type(td));
        }

        decls.push(Decl::Prompt(PromptDecl {
            name: Ident::new(name.clone(), sp),
            params,
            return_ty,
            return_ownership: None,
            template: first.rendered.clone().unwrap_or_default(),
            effect_row: EffectRow::default(),
            cites_strictly: None,
            stream: corvid_ast::PromptStreamSettings::default(),
            calibrated: false,
            cacheable: false,
            capability_required: None,
            output_format_required: None,
            route: None,
            progressive: None,
            rollout: None,
            ensemble: None,
            adversarial: None,
            visibility: Visibility::Private,
            span: sp,
        }));
    }

    // Build a mock LLM adapter that replays prompt results.
    let prompt_mocks = mocks.prompts.clone();
    let mock_adapter = corvid_runtime::MockAdapter::new("trace-import");
    let mut mock = mock_adapter;
    for (name, calls) in &prompt_mocks {
        if let Some(first) = calls.first() {
            mock = mock.reply(name.clone(), first.result.clone());
        }
    }
    builder = builder.llm(Arc::new(mock)).default_model("trace-import");

    TraceImportResult {
        decls,
        runtime: builder.build(),
        tool_count: mocks.tools.len(),
        prompt_count: mocks.prompts.len(),
        tool_names,
        prompt_names,
    }
}

/// Infer Corvid parameters from recorded JSON args.
fn infer_params(
    args: &[JsonValue],
    decl_name: &str,
    prefix: &str,
    sp: &Span,
    type_decls: &mut Vec<TypeDecl>,
) -> Vec<Param> {
    args.iter()
        .enumerate()
        .map(|(i, arg)| {
            let param_name = if args.len() == 1 {
                prefix.to_string()
            } else {
                format!("{prefix}{}", i + 1)
            };
            let type_stem = format!("{}{}", to_pascal(decl_name), to_pascal(&param_name));
            let (ty, new_types) = infer_typeref(arg, &type_stem, "", sp);
            type_decls.extend(new_types);
            Param {
                name: Ident::new(param_name, *sp),
                ty,
                ownership: None,
                span: *sp,
            }
        })
        .collect()
}

/// Infer a TypeRef from a JSON value, generating type declarations for objects.
fn infer_typeref(
    val: &JsonValue,
    type_stem: &str,
    suffix: &str,
    sp: &Span,
) -> (TypeRef, Vec<TypeDecl>) {
    let mut type_decls = Vec::new();

    let ty = match val {
        JsonValue::Null => TypeRef::Named { name: Ident::new("Nothing", *sp), span: *sp },
        JsonValue::Bool(_) => TypeRef::Named { name: Ident::new("Bool", *sp), span: *sp },
        JsonValue::Number(n) => {
            if n.is_f64() && !n.is_i64() {
                TypeRef::Named { name: Ident::new("Float", *sp), span: *sp }
            } else {
                TypeRef::Named { name: Ident::new("Int", *sp), span: *sp }
            }
        }
        JsonValue::String(_) => TypeRef::Named { name: Ident::new("String", *sp), span: *sp },
        JsonValue::Array(items) => {
            let inner = if let Some(first) = items.first() {
                let (inner_ty, inner_decls) = infer_typeref(first, type_stem, "Item", sp);
                type_decls.extend(inner_decls);
                inner_ty
            } else {
                TypeRef::Named { name: Ident::new("String", *sp), span: *sp }
            };
            TypeRef::Generic {
                name: Ident::new("List", *sp),
                args: vec![inner],
                span: *sp,
            }
        }
        JsonValue::Object(map) => {
            let type_name = format!("{type_stem}{suffix}");
            let fields: Vec<Field> = map.iter().map(|(key, val)| {
                let (field_ty, field_decls) = infer_typeref(val, &format!("{type_name}{}", to_pascal(key)), "", sp);
                type_decls.extend(field_decls);
                Field {
                    name: Ident::new(key.clone(), *sp),
                    ty: field_ty,
                    span: *sp,
                }
            }).collect();

            type_decls.push(TypeDecl {
                name: Ident::new(type_name.clone(), *sp),
                fields,
                visibility: Visibility::Private,
                span: *sp,
            });

            TypeRef::Named { name: Ident::new(type_name, *sp), span: *sp }
        }
    };

    (ty, type_decls)
}

fn json_args_match(recorded: &[JsonValue], actual: &[JsonValue]) -> bool {
    if recorded.len() != actual.len() {
        return false;
    }
    recorded.iter().zip(actual).all(|(r, a)| r == a)
}

fn to_pascal(s: &str) -> String {
    s.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => {
                    let mut out = c.to_uppercase().to_string();
                    out.extend(chars);
                    out
                }
            }
        })
        .collect()
}
