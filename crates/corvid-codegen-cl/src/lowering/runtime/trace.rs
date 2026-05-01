//! Per-type trace synthesis and JSON-payload emission.
//!
//! Trace functions (`corvid_trace_<TypeName>`) drive the cycle
//! collector's mark walk — for each refcounted slot they invoke
//! an indirect marker function pointer instead of releasing it.
//! Trace fns are emitted for every refcounted struct / result /
//! option (including ones with zero refcounted children) so the
//! collector can dispatch uniformly without a per-object NULL
//! check.
//!
//! The JSON-payload helpers (`emit_json_stringify_arg`,
//! `emit_trace_payload`, and friends) build a serialized String
//! representation of a runtime value for the diagnostic tracer.
//! They also recurse over the static `Type` to drive the C
//! `corvid_json_buffer_*` surface.

use super::*;

/// Emit `corvid_trace_<TypeName>(payload, marker, ctx)` for
/// a refcounted struct type. Mirrors `define_struct_destructor` but
/// dispatches through an indirect marker function pointer on each
/// refcounted field instead of releasing it.
///
/// Trace fns are emitted for every refcounted struct — including
/// structs with zero refcounted fields — so the future (17d) mark
/// collector can dispatch uniformly without a per-object NULL check.
/// The linker folds duplicate empty bodies, so the cost is ~zero.
///
/// Marker signature: `fn(obj: i64, ctx: i64) -> ()`. Context-passing
/// (rather than stateless) so 17d's collector can thread a worklist
/// pointer through the walk without TLS or globals.
pub fn define_struct_trace(
    module: &mut ObjectModule,
    ty: &corvid_ir::IrType,
    runtime: &RuntimeFuncs,
) -> Result<FuncId, CodegenError> {
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(I64)); // payload
    sig.params.push(AbiParam::new(I64)); // marker fn ptr
    sig.params.push(AbiParam::new(I64)); // ctx

    let symbol = format!("corvid_trace_{}", ty.name);
    let func_id = module
        .declare_function(&symbol, Linkage::Local, &sig)
        .map_err(|e| CodegenError::cranelift(format!("declare trace `{symbol}`: {e}"), ty.span))?;

    let mut ctx = Context::new();
    ctx.func = Function::with_name_signature(
        UserFuncName::user(0, func_id.as_u32()),
        module
            .declarations()
            .get_function_decl(func_id)
            .signature
            .clone(),
    );
    let mut bctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut bctx);

        // Marker signature: fn(obj: i64, ctx: i64) -> ()
        let mut marker_sig = Signature::new(module.isa().default_call_conv());
        marker_sig.params.push(AbiParam::new(I64));
        marker_sig.params.push(AbiParam::new(I64));
        let marker_sigref = builder.import_signature(marker_sig);

        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let payload = builder.block_params(entry)[0];
        let marker = builder.block_params(entry)[1];
        let marker_ctx = builder.block_params(entry)[2];

        for (i, field) in ty.fields.iter().enumerate() {
            if is_refcounted_type(&field.ty) {
                let offset = (i as i32) * STRUCT_FIELD_SLOT_BYTES;
                let v = builder.ins().load(
                    I64,
                    cranelift_codegen::ir::MemFlags::trusted(),
                    payload,
                    offset,
                );
                builder
                    .ins()
                    .call_indirect(marker_sigref, marker, &[v, marker_ctx]);
            }
        }

        builder.ins().return_(&[]);
        builder.finalize();
    }

    define_function_with_stack_maps(
        module,
        func_id,
        &mut ctx,
        runtime,
        ty.span,
        &format!("trace `{symbol}`"),
    )?;
    Ok(func_id)
}

pub fn define_result_trace(
    module: &mut ObjectModule,
    result_ty: &Type,
    ok_ty: &Type,
    err_ty: &Type,
    runtime: &RuntimeFuncs,
) -> Result<FuncId, CodegenError> {
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(I64));
    sig.params.push(AbiParam::new(I64));
    sig.params.push(AbiParam::new(I64));

    let symbol = format!("corvid_trace_{}", mangle_type_name(result_ty));
    let func_id = module
        .declare_function(&symbol, Linkage::Local, &sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare trace `{symbol}`: {e}"), Span::new(0, 0))
        })?;

    let mut ctx = Context::new();
    ctx.func = Function::with_name_signature(
        UserFuncName::user(0, func_id.as_u32()),
        module
            .declarations()
            .get_function_decl(func_id)
            .signature
            .clone(),
    );
    let mut bctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut bctx);

        let mut marker_sig = Signature::new(module.isa().default_call_conv());
        marker_sig.params.push(AbiParam::new(I64));
        marker_sig.params.push(AbiParam::new(I64));
        let marker_sigref = builder.import_signature(marker_sig);

        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);

        let payload = builder.block_params(entry)[0];
        let marker = builder.block_params(entry)[1];
        let marker_ctx = builder.block_params(entry)[2];
        let tag = builder.ins().load(
            I64,
            cranelift_codegen::ir::MemFlags::trusted(),
            payload,
            RESULT_TAG_OFFSET,
        );

        if is_refcounted_type(ok_ty) || is_refcounted_type(err_ty) {
            let ok_block = builder.create_block();
            let err_block = builder.create_block();
            let done_block = builder.create_block();
            let is_ok = builder.ins().icmp_imm(IntCC::Equal, tag, RESULT_TAG_OK);
            builder.ins().brif(is_ok, ok_block, &[], err_block, &[]);

            builder.switch_to_block(ok_block);
            builder.seal_block(ok_block);
            if is_refcounted_type(ok_ty) {
                let v = builder.ins().load(
                    I64,
                    cranelift_codegen::ir::MemFlags::trusted(),
                    payload,
                    RESULT_PAYLOAD_OFFSET,
                );
                builder
                    .ins()
                    .call_indirect(marker_sigref, marker, &[v, marker_ctx]);
            }
            builder.ins().jump(done_block, &[]);

            builder.switch_to_block(err_block);
            builder.seal_block(err_block);
            if is_refcounted_type(err_ty) {
                let v = builder.ins().load(
                    I64,
                    cranelift_codegen::ir::MemFlags::trusted(),
                    payload,
                    RESULT_PAYLOAD_OFFSET,
                );
                builder
                    .ins()
                    .call_indirect(marker_sigref, marker, &[v, marker_ctx]);
            }
            builder.ins().jump(done_block, &[]);

            builder.switch_to_block(done_block);
            builder.seal_block(done_block);
        }

        builder.ins().return_(&[]);
        builder.finalize();
    }

    define_function_with_stack_maps(
        module,
        func_id,
        &mut ctx,
        runtime,
        Span::new(0, 0),
        &format!("trace `{symbol}`"),
    )?;
    Ok(func_id)
}

pub fn define_option_trace(
    module: &mut ObjectModule,
    option_ty: &Type,
    payload_ty: &Type,
    runtime: &RuntimeFuncs,
) -> Result<FuncId, CodegenError> {
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(I64)); // payload wrapper ptr
    sig.params.push(AbiParam::new(I64)); // marker fn ptr
    sig.params.push(AbiParam::new(I64)); // ctx

    let symbol = format!("corvid_trace_{}", mangle_type_name(option_ty));
    let func_id = module
        .declare_function(&symbol, Linkage::Local, &sig)
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare option trace `{symbol}`: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut ctx = Context::new();
    ctx.func = Function::with_name_signature(
        UserFuncName::user(0, func_id.as_u32()),
        module
            .declarations()
            .get_function_decl(func_id)
            .signature
            .clone(),
    );
    let mut bctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut bctx);

        let mut marker_sig = Signature::new(module.isa().default_call_conv());
        marker_sig.params.push(AbiParam::new(I64));
        marker_sig.params.push(AbiParam::new(I64));
        let marker_sigref = builder.import_signature(marker_sig);

        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let payload = builder.block_params(entry)[0];
        let marker = builder.block_params(entry)[1];
        let marker_ctx = builder.block_params(entry)[2];

        if is_refcounted_type(payload_ty) {
            let payload_val = builder.ins().load(
                I64,
                cranelift_codegen::ir::MemFlags::trusted(),
                payload,
                OPTION_PAYLOAD_OFFSET,
            );
            builder
                .ins()
                .call_indirect(marker_sigref, marker, &[payload_val, marker_ctx]);
        }

        builder.ins().return_(&[]);
        builder.finalize();
    }

    define_function_with_stack_maps(
        module,
        func_id,
        &mut ctx,
        runtime,
        Span::new(0, 0),
        &format!("option trace `{symbol}`"),
    )?;
    Ok(func_id)
}

/// Build a JSON-encoded `Corvid String` for `value` and return a
/// descriptor pointer that the trace recorder can pin into a
/// `'j'`-tagged trace slot. The returned descriptor has refcount = 1;
/// the caller releases it after the trace event has been recorded.
///
/// Walks the static `Type` to drive a `corvid_json_buffer_*` C surface:
/// scalars are appended directly, structs/lists/options/results recurse
/// over the respective memory layout. The payload format mirrors
/// `serde_json::Value::to_string()` for the same logical value, so the
/// downstream `decode_slot_json('j')` path can decode through
/// `serde_json::from_str` without any custom parser.
pub fn emit_json_stringify_arg(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    value: ClValue,
    ty: &Type,
    span: Span,
) -> Result<ClValue, CodegenError> {
    let new_ref = module.declare_func_in_func(runtime.json_buffer_new, builder.func);
    let new_call = builder.ins().call(new_ref, &[]);
    let buf = builder.inst_results(new_call)[0];

    emit_json_append(builder, module, runtime, buf, value, ty, span)?;

    let finish_ref = module.declare_func_in_func(runtime.json_buffer_finish, builder.func);
    let finish_call = builder.ins().call(finish_ref, &[buf]);
    Ok(builder.inst_results(finish_call)[0])
}

fn emit_json_append(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    buf: ClValue,
    value: ClValue,
    ty: &Type,
    span: Span,
) -> Result<(), CodegenError> {
    match ty {
        Type::Int => {
            let f = module.declare_func_in_func(runtime.json_buffer_append_int, builder.func);
            builder.ins().call(f, &[buf, value]);
        }
        Type::Bool => {
            let f = module.declare_func_in_func(runtime.json_buffer_append_bool, builder.func);
            builder.ins().call(f, &[buf, value]);
        }
        Type::Float => {
            let f = module.declare_func_in_func(runtime.json_buffer_append_float, builder.func);
            builder.ins().call(f, &[buf, value]);
        }
        Type::String => {
            let f = module.declare_func_in_func(runtime.json_buffer_append_string, builder.func);
            builder.ins().call(f, &[buf, value]);
        }
        Type::Grounded(inner) => {
            emit_json_append(builder, module, runtime, buf, value, inner, span)?;
        }
        Type::Struct(def_id) => {
            emit_json_append_struct(builder, module, runtime, buf, value, *def_id, span)?;
        }
        Type::List(elem_ty) => {
            emit_json_append_list(builder, module, runtime, buf, value, elem_ty, span)?;
        }
        Type::Option(payload_ty) => {
            emit_json_append_option(builder, module, runtime, buf, value, ty, payload_ty, span)?;
        }
        Type::Result(ok_ty, err_ty) => {
            emit_json_append_result(builder, module, runtime, buf, value, ok_ty, err_ty, span)?;
        }
        other => {
            return Err(CodegenError::not_supported(
                format!(
                    "JSON-encoding `{}` for trace payload — non-scalar trace coverage is incremental; this concrete shape is outside the current native subset",
                    other.display_name()
                ),
                span,
            ));
        }
    }
    Ok(())
}

fn emit_json_append_raw(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    buf: ClValue,
    text: &str,
    span: Span,
) -> Result<(), CodegenError> {
    let lit = lower_string_literal(builder, module, runtime, text, span)?;
    let f = module.declare_func_in_func(runtime.json_buffer_append_raw, builder.func);
    builder.ins().call(f, &[buf, lit]);
    Ok(())
}

fn emit_json_append_struct(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    buf: ClValue,
    value: ClValue,
    def_id: DefId,
    span: Span,
) -> Result<(), CodegenError> {
    let ir_type = runtime
        .ir_types
        .get(&def_id)
        .ok_or_else(|| {
            CodegenError::cranelift(
                format!("JSON-encoding struct: missing IR type for def {def_id:?}"),
                span,
            )
        })?
        .clone();
    if ir_type.fields.is_empty() {
        emit_json_append_raw(builder, module, runtime, buf, "{}", span)?;
        return Ok(());
    }
    for (i, field) in ir_type.fields.iter().enumerate() {
        // Corvid identifiers are alphanumeric + underscore, so they
        // need no JSON escaping inside a key string.
        let prefix = if i == 0 {
            format!("{{\"{}\":", field.name)
        } else {
            format!(",\"{}\":", field.name)
        };
        emit_json_append_raw(builder, module, runtime, buf, &prefix, span)?;
        let field_cl_ty = cl_type_for(&field.ty, span)?;
        let offset = (i as i32) * STRUCT_FIELD_SLOT_BYTES;
        let field_val = builder
            .ins()
            .load(field_cl_ty, MemFlags::trusted(), value, offset);
        emit_json_append(builder, module, runtime, buf, field_val, &field.ty, span)?;
    }
    emit_json_append_raw(builder, module, runtime, buf, "}", span)?;
    Ok(())
}

fn emit_json_append_list(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    buf: ClValue,
    value: ClValue,
    elem_ty: &Type,
    span: Span,
) -> Result<(), CodegenError> {
    let elem_cl_ty = cl_type_for(elem_ty, span)?;
    emit_json_append_raw(builder, module, runtime, buf, "[", span)?;
    let length = builder.ins().load(I64, MemFlags::trusted(), value, 0);
    let zero = builder.ins().iconst(I64, 0);

    let header_block = builder.create_block();
    let body_block = builder.create_block();
    let comma_block = builder.create_block();
    let elem_block = builder.create_block();
    let end_block = builder.create_block();
    let counter = builder.append_block_param(header_block, I64);

    builder.ins().jump(header_block, &[zero.into()]);

    builder.switch_to_block(header_block);
    let cond = builder.ins().icmp(IntCC::SignedLessThan, counter, length);
    builder.ins().brif(cond, body_block, &[], end_block, &[]);

    builder.switch_to_block(body_block);
    builder.seal_block(body_block);
    let needs_comma = builder.ins().icmp_imm(IntCC::SignedGreaterThan, counter, 0);
    builder
        .ins()
        .brif(needs_comma, comma_block, &[], elem_block, &[]);

    builder.switch_to_block(comma_block);
    builder.seal_block(comma_block);
    emit_json_append_raw(builder, module, runtime, buf, ",", span)?;
    builder.ins().jump(elem_block, &[]);

    builder.switch_to_block(elem_block);
    builder.seal_block(elem_block);
    let offset = builder.ins().imul_imm(counter, 8);
    let base = builder.ins().iadd_imm(value, 8);
    let elem_addr = builder.ins().iadd(base, offset);
    let elem_val = builder
        .ins()
        .load(elem_cl_ty, MemFlags::trusted(), elem_addr, 0);
    emit_json_append(builder, module, runtime, buf, elem_val, elem_ty, span)?;
    let next = builder.ins().iadd_imm(counter, 1);
    builder.ins().jump(header_block, &[next.into()]);

    // Header has two predecessors: initial jump and loop-back jump.
    // Seal only after both edges have been emitted.
    builder.seal_block(header_block);

    builder.switch_to_block(end_block);
    builder.seal_block(end_block);
    emit_json_append_raw(builder, module, runtime, buf, "]", span)?;
    Ok(())
}

fn emit_json_append_option(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    buf: ClValue,
    value: ClValue,
    option_ty: &Type,
    payload_ty: &Type,
    span: Span,
) -> Result<(), CodegenError> {
    if !is_native_option_type(option_ty) {
        return Err(CodegenError::not_supported(
            format!(
                "JSON-encoding `{}` for trace payload — only nullable-pointer Option<T> for refcounted T plus wide scalar Option<Int|Bool|Float> are covered today",
                option_ty.display_name()
            ),
            span,
        ));
    }
    let some_block = builder.create_block();
    let none_block = builder.create_block();
    let merge_block = builder.create_block();

    builder.ins().brif(value, some_block, &[], none_block, &[]);

    builder.switch_to_block(none_block);
    builder.seal_block(none_block);
    let null_f = module.declare_func_in_func(runtime.json_buffer_append_null, builder.func);
    builder.ins().call(null_f, &[buf]);
    builder.ins().jump(merge_block, &[]);

    builder.switch_to_block(some_block);
    builder.seal_block(some_block);
    let payload_val = if option_uses_wrapper(option_ty) {
        let payload_cl_ty = cl_type_for(payload_ty, span)?;
        builder.ins().load(
            payload_cl_ty,
            MemFlags::trusted(),
            value,
            OPTION_PAYLOAD_OFFSET,
        )
    } else {
        // Refcounted-payload Option uses bare nullable-pointer
        // encoding — the value IS the payload pointer when non-null.
        value
    };
    emit_json_append(builder, module, runtime, buf, payload_val, payload_ty, span)?;
    builder.ins().jump(merge_block, &[]);

    builder.switch_to_block(merge_block);
    builder.seal_block(merge_block);
    Ok(())
}

fn emit_json_append_result(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    buf: ClValue,
    value: ClValue,
    ok_ty: &Type,
    err_ty: &Type,
    span: Span,
) -> Result<(), CodegenError> {
    let tag = builder
        .ins()
        .load(I64, MemFlags::trusted(), value, RESULT_TAG_OFFSET);
    let is_ok = builder.ins().icmp_imm(IntCC::Equal, tag, RESULT_TAG_OK);

    let ok_block = builder.create_block();
    let err_block = builder.create_block();
    let merge_block = builder.create_block();

    builder.ins().brif(is_ok, ok_block, &[], err_block, &[]);

    builder.switch_to_block(ok_block);
    builder.seal_block(ok_block);
    emit_json_append_raw(builder, module, runtime, buf, "{\"Ok\":", span)?;
    let ok_cl_ty = cl_type_for(ok_ty, span)?;
    let ok_payload =
        builder
            .ins()
            .load(ok_cl_ty, MemFlags::trusted(), value, RESULT_PAYLOAD_OFFSET);
    emit_json_append(builder, module, runtime, buf, ok_payload, ok_ty, span)?;
    emit_json_append_raw(builder, module, runtime, buf, "}", span)?;
    builder.ins().jump(merge_block, &[]);

    builder.switch_to_block(err_block);
    builder.seal_block(err_block);
    emit_json_append_raw(builder, module, runtime, buf, "{\"Err\":", span)?;
    let err_cl_ty = cl_type_for(err_ty, span)?;
    let err_payload =
        builder
            .ins()
            .load(err_cl_ty, MemFlags::trusted(), value, RESULT_PAYLOAD_OFFSET);
    emit_json_append(builder, module, runtime, buf, err_payload, err_ty, span)?;
    emit_json_append_raw(builder, module, runtime, buf, "}", span)?;
    builder.ins().jump(merge_block, &[]);

    builder.switch_to_block(merge_block);
    builder.seal_block(merge_block);
    Ok(())
}

pub fn emit_trace_payload(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    values: &[ClValue],
    tys: &[Type],
    span: Span,
) -> Result<TracePayload, CodegenError> {
    debug_assert_eq!(values.len(), tys.len());
    let tags = tys
        .iter()
        .map(trace_tag_for_type)
        .collect::<Result<String, _>>()?;
    let type_tags = lower_string_literal(builder, module, runtime, &tags, span)?;
    let count = builder.ins().iconst(I64, values.len() as i64);
    let mut owned_values = Vec::new();
    let values_ptr = if values.is_empty() {
        builder.ins().iconst(I64, 0)
    } else {
        let stack_slot = builder.create_sized_stack_slot(clir::StackSlotData::new(
            clir::StackSlotKind::ExplicitSlot,
            (values.len() as u32) * 8,
            3,
        ));
        for (idx, (value, ty)) in values.iter().zip(tys.iter()).enumerate() {
            let offset = (idx as i32) * 8;
            match ty {
                Type::Grounded(inner) => match inner.as_ref() {
                    Type::Int | Type::String => {
                        builder.ins().stack_store(*value, stack_slot, offset);
                    }
                    Type::Bool => {
                        let widened = builder.ins().uextend(I64, *value);
                        builder.ins().stack_store(widened, stack_slot, offset);
                    }
                    Type::Float => {
                        builder.ins().stack_store(*value, stack_slot, offset);
                    }
                    other => {
                        let json =
                            emit_json_stringify_arg(builder, module, runtime, *value, other, span)?;
                        builder.ins().stack_store(json, stack_slot, offset);
                        owned_values.push(json);
                    }
                },
                Type::Int | Type::String => {
                    builder.ins().stack_store(*value, stack_slot, offset);
                }
                Type::Bool => {
                    let widened = builder.ins().uextend(I64, *value);
                    builder.ins().stack_store(widened, stack_slot, offset);
                }
                Type::Float => {
                    builder.ins().stack_store(*value, stack_slot, offset);
                }
                other => {
                    let json =
                        emit_json_stringify_arg(builder, module, runtime, *value, other, span)?;
                    builder.ins().stack_store(json, stack_slot, offset);
                    owned_values.push(json);
                }
            }
        }
        builder.ins().stack_addr(I64, stack_slot, 0)
    };
    Ok(TracePayload {
        type_tags,
        count,
        values_ptr,
        owned_values,
    })
}

fn trace_tag_for_type(ty: &Type) -> Result<char, CodegenError> {
    match ty {
        Type::Grounded(inner) => trace_tag_for_type(inner),
        Type::Int => Ok('i'),
        Type::Bool => Ok('b'),
        Type::Float => Ok('f'),
        Type::String => Ok('s'),
        _ => Ok('j'),
    }
}
