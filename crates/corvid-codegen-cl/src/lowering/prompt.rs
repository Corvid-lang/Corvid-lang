use super::*;

#[derive(Debug)]
enum TemplateSegment<'a> {
    Literal(&'a str),
    Param(usize), // index into the prompt's params
}

fn prompt_constant_arg_text(expr: &IrExpr) -> Option<String> {
    match &expr.kind {
        IrExprKind::Literal(IrLiteral::String(s)) => Some(s.clone()),
        IrExprKind::Literal(IrLiteral::Int(n)) => Some(n.to_string()),
        IrExprKind::Literal(IrLiteral::Bool(b)) => Some(if *b {
            "true".to_string()
        } else {
            "false".to_string()
        }),
        _ => None,
    }
}

fn render_prompt_constant(
    segments: &[TemplateSegment<'_>],
    args: &[IrExpr],
) -> Option<String> {
    let mut rendered = String::new();
    for seg in segments {
        match seg {
            TemplateSegment::Literal(text) => rendered.push_str(text),
            TemplateSegment::Param(idx) => {
                rendered.push_str(&prompt_constant_arg_text(&args[*idx])?);
            }
        }
    }
    Some(rendered)
}

/// Parse a prompt template into literal + `{param_name}` segments.
/// Param names that aren't in `params` produce a codegen error —
/// matches what the typechecker should already enforce, kept as
/// belt-and-braces.
fn parse_prompt_template<'a>(
    template: &'a str,
    params: &[corvid_ir::IrParam],
    span: Span,
) -> Result<Vec<TemplateSegment<'a>>, CodegenError> {
    let mut out: Vec<TemplateSegment<'a>> = Vec::new();
    let mut cursor = 0;
    let bytes = template.as_bytes();
    while cursor < bytes.len() {
        if let Some(open_rel) = template[cursor..].find('{') {
            let open = cursor + open_rel;
            if open > cursor {
                out.push(TemplateSegment::Literal(&template[cursor..open]));
            }
            let close_rel = template[open + 1..].find('}').ok_or_else(|| {
                CodegenError::cranelift(
                    format!(
                        "prompt template has unmatched `{{` near offset {open}: `{template}`"
                    ),
                    span,
                )
            })?;
            let close = open + 1 + close_rel;
            let name = template[open + 1..close].trim();
            let idx = params.iter().position(|p| p.name == name).ok_or_else(|| {
                CodegenError::cranelift(
                    format!(
                        "prompt template references `{{{name}}}` but no such parameter — typechecker should have caught this; available: {:?}",
                        params.iter().map(|p| &p.name).collect::<Vec<_>>()
                    ),
                    span,
                )
            })?;
            out.push(TemplateSegment::Param(idx));
            cursor = close + 1;
        } else {
            out.push(TemplateSegment::Literal(&template[cursor..]));
            break;
        }
    }
    Ok(out)
}

pub(super) fn emit_string_const(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    s: &str,
    span: Span,
) -> Result<ClValue, CodegenError> {
    lower_string_literal(builder, module, runtime, s, span)
}

fn emit_stringify_arg(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    arg_value: ClValue,
    arg_ty: &Type,
    span: Span,
) -> Result<ClValue, CodegenError> {
    match arg_ty {
        Type::String => Ok(arg_value),
        Type::Int => {
            let f = module.declare_func_in_func(runtime.string_from_int, builder.func);
            let call = builder.ins().call(f, &[arg_value]);
            let results: Vec<ClValue> =
                builder.inst_results(call).iter().copied().collect();
            Ok(results[0])
        }
        Type::Bool => {
            let f = module.declare_func_in_func(runtime.string_from_bool, builder.func);
            let call = builder.ins().call(f, &[arg_value]);
            let results: Vec<ClValue> =
                builder.inst_results(call).iter().copied().collect();
            Ok(results[0])
        }
        Type::Float => {
            let f = module.declare_func_in_func(runtime.string_from_float, builder.func);
            let call = builder.ins().call(f, &[arg_value]);
            let results: Vec<ClValue> =
                builder.inst_results(call).iter().copied().collect();
            Ok(results[0])
        }
        other => Err(CodegenError::not_supported(
            format!(
                "prompt argument type `{}` is not yet supported in template interpolation — the native prompt bridge currently supports only Int / Bool / Float / String; Struct / List interpolation is not implemented yet",
                other.display_name()
            ),
            span,
        )),
    }
}

fn emit_concat_chain(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    parts: Vec<(ClValue, bool)>,
    span: Span,
) -> Result<(ClValue, bool), CodegenError> {
    if parts.is_empty() {
        return emit_string_const(builder, module, runtime, "", span).map(|v| (v, false));
    }
    let mut parts = parts.into_iter();
    let (mut acc, mut acc_borrowed) = parts.next().expect("parts not empty");
    let concat_fid = module.declare_func_in_func(runtime.string_concat, builder.func);
    for (next, next_borrowed) in parts {
        let call = builder.ins().call(concat_fid, &[acc, next]);
        let results: Vec<ClValue> =
            builder.inst_results(call).iter().copied().collect();
        let new_acc = results[0];
        if !acc_borrowed {
            emit_release(builder, module, runtime, acc);
        }
        if !next_borrowed {
            emit_release(builder, module, runtime, next);
        }
        acc = new_acc;
        acc_borrowed = false;
    }
    Ok((acc, acc_borrowed))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn lower_prompt_call(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    def_id: DefId,
    callee_name: &str,
    args: &[IrExpr],
    current_return_ty: &Type,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    scope_stack: &Vec<Vec<(LocalId, Variable)>>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    return_ty: &Type,
    span: Span,
) -> Result<ClValue, CodegenError> {
    let prompt = runtime
        .ir_prompts
        .get(&def_id)
        .cloned()
        .ok_or_else(|| {
            CodegenError::cranelift(
                format!(
                    "prompt `{callee_name}` metadata missing from ir_prompts — declare-pass invariant violated"
                ),
                span,
            )
        })?;

    if prompt.params.len() != args.len() {
        return Err(CodegenError::cranelift(
            format!(
                "prompt `{callee_name}` declared with {} param(s) but called with {}",
                prompt.params.len(),
                args.len()
            ),
            span,
        ));
    }

    let pinned_locals = runtime.prompt_pins.get(&span);
    let mut arg_vals: Vec<ClValue> = Vec::with_capacity(args.len());
    let mut arg_pinned: Vec<bool> = Vec::with_capacity(args.len());
    for a in args {
        let v = lower_expr(
            builder,
            a,
            current_return_ty,
            env,
            scope_stack,
            func_ids_by_def,
            module,
            runtime,
        )?;
        let pinned = matches!(&a.ty, Type::String)
            && matches!(&a.kind, IrExprKind::Local { .. })
            && pinned_locals.is_some_and(|set| {
                matches!(
                    &a.kind,
                    IrExprKind::Local { local_id, .. } if set.contains(local_id)
                )
            });
        arg_vals.push(v);
        arg_pinned.push(pinned);
    }

    let segments = parse_prompt_template(&prompt.template, &prompt.params, span)?;
    let (rendered, rendered_borrowed) = if let Some(text) =
        render_prompt_constant(&segments, args)
    {
        (emit_string_const(builder, module, runtime, &text, span)?, false)
    } else {
        let mut parts: Vec<(ClValue, bool)> = Vec::with_capacity(segments.len());
        for seg in &segments {
            let part = match seg {
                TemplateSegment::Literal(text) => (
                    emit_string_const(builder, module, runtime, text, span)?,
                    false,
                ),
                TemplateSegment::Param(idx) => {
                    let av = arg_vals[*idx];
                    let aty = &args[*idx].ty;
                    (
                        emit_stringify_arg(builder, module, runtime, av, aty, span)?,
                        arg_pinned[*idx] && matches!(aty, Type::String),
                    )
                }
            };
            parts.push(part);
        }
        emit_concat_chain(builder, module, runtime, parts, span)?
    };

    let prompt_name_val = emit_string_const(builder, module, runtime, &prompt.name, span)?;
    let signature_val = emit_string_const(
        builder,
        module,
        runtime,
        &format_prompt_signature(&prompt),
        span,
    )?;
    let model_val = emit_string_const(builder, module, runtime, "", span)?;
    let arg_tys = args.iter().map(|arg| arg.ty.clone()).collect::<Vec<_>>();
    let trace_payload = emit_trace_payload(builder, module, runtime, &arg_vals, &arg_tys, span)?;

    let bridge_id = match return_ty {
        Type::Int => runtime.prompt_call_int,
        Type::Bool => runtime.prompt_call_bool,
        Type::Float => runtime.prompt_call_float,
        Type::String => runtime.prompt_call_string,
        other => {
            return Err(CodegenError::not_supported(
                format!(
                    "prompt `{callee_name}` returns `{}` — the native prompt bridge currently supports only Int / Bool / Float / String returns; structured prompt returns are not implemented yet",
                    other.display_name()
                ),
                span,
            ));
        }
    };
    let fref = module.declare_func_in_func(bridge_id, builder.func);
    let call = builder
        .ins()
        .call(
            fref,
            &[
                prompt_name_val,
                signature_val,
                rendered,
                model_val,
                trace_payload.type_tags,
                trace_payload.count,
                trace_payload.values_ptr,
            ],
        );
    let result_vals: Vec<ClValue> =
        builder.inst_results(call).iter().copied().collect();

    emit_release(builder, module, runtime, prompt_name_val);
    emit_release(builder, module, runtime, signature_val);
    if !rendered_borrowed {
        emit_release(builder, module, runtime, rendered);
    }
    emit_release(builder, module, runtime, model_val);
    emit_release(builder, module, runtime, trace_payload.type_tags);

    for (v, a) in arg_vals.iter().zip(args.iter()) {
        if is_refcounted_type(&a.ty) {
            let _ = v;
        } else {
            let _ = v;
        }
    }

    if result_vals.len() != 1 {
        return Err(CodegenError::cranelift(
            format!(
                "prompt bridge returned {} values; expected 1 for return type `{}`",
                result_vals.len(),
                return_ty.display_name()
            ),
            span,
        ));
    }
    Ok(result_vals[0])
}

fn format_prompt_signature(p: &corvid_ir::IrPrompt) -> String {
    let params = p
        .params
        .iter()
        .map(|param| format!("{}: {}", param.name, param.ty.display_name()))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{}({}) -> {}", p.name, params, p.return_ty.display_name())
}
