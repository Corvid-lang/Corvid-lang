//! Constructor lowering for refcounted heap values — struct,
//! result, option, and string-literal sites — plus the
//! grounded-value attestation emission that wraps the bare struct
//! payload with provenance metadata.
//!
//! Each constructor allocates the value as Owned (refcount = 1)
//! and either uses `corvid_alloc_with_destructor` (when the type
//! has refcounted children that need a per-instance destructor) or
//! the bare allocator. Argument values transfer ownership into the
//! container without extra retain/release.

use super::*;

pub fn emit_grounded_value_attestation(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    value: ClValue,
    grounded_ty: &Type,
    source_name: &str,
    confidence: f64,
    span: Span,
) -> Result<ClValue, CodegenError> {
    let source_name_val = emit_string_const(builder, module, runtime, source_name, span)?;
    let confidence_val = builder.ins().f64const(confidence);
    let attest_id = match grounded_ty {
        Type::Grounded(inner) => match inner.as_ref() {
            Type::Int => runtime.grounded_attest_int,
            Type::Bool => runtime.grounded_attest_bool,
            Type::Float => runtime.grounded_attest_float,
            Type::String => runtime.grounded_attest_string,
            other => {
                return Err(CodegenError::not_supported(
                    format!(
                        "grounded return `{}` is not yet supported at the native ABI boundary",
                        other.display_name()
                    ),
                    span,
                ))
            }
        },
        _ => unreachable!("grounded attestation requires Grounded<T>"),
    };
    let attest_ref = module.declare_func_in_func(attest_id, builder.func);
    let attest_call = builder
        .ins()
        .call(attest_ref, &[value, source_name_val, confidence_val]);
    let attested = builder.inst_results(attest_call)[0];
    emit_release(builder, module, runtime, source_name_val);
    Ok(attested)
}

/// Lower a struct constructor: allocate, store each field at its
/// offset, return the struct pointer (refcount = 1, Owned).
///
/// Each constructor argument is lowered as an Owned temp; the store
/// transfers ownership into the struct (no extra retain, no release).
/// If the struct has a per-type destructor (at least one refcounted
/// field), we use `corvid_alloc_with_destructor` so `corvid_release`
/// will release those fields when the struct is eventually freed.
pub fn lower_struct_constructor(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    ty: &corvid_ir::IrType,
    args: &[IrExpr],
    current_return_ty: &Type,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    scope_stack: &Vec<Vec<(LocalId, Variable)>>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    span: Span,
) -> Result<ClValue, CodegenError> {
    if args.len() != ty.fields.len() {
        return Err(CodegenError::cranelift(
            format!(
                "struct `{}` expects {} field(s), got {}",
                ty.name,
                ty.fields.len(),
                args.len()
            ),
            span,
        ));
    }
    let size = builder
        .ins()
        .iconst(I64, struct_payload_bytes(ty.fields.len()));
    // Structs with refcounted fields use the typeinfo-
    // driven allocator. Structs with no refcounted fields currently
    // skip typeinfo entirely (lower_file bypasses emission for them)
    // ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â they allocate with NULL destroy_fn via a runtime-owned
    // empty typeinfo? For 17a we keep the pre-typed behavior for
    // these: only emit typed allocation when the struct actually
    // has refcounted fields requiring dispatch. Non-refcounted
    // structs remain on the old path *only temporarily* ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â future
    // 17a's follow-up will uniformize this once 17d needs every
    // heap object traceable. Tracked as a TODO in ROADMAP.
    let struct_ptr = if let Some(&ti_id) = runtime.struct_typeinfos.get(&ty.id) {
        let ti_gv = module.declare_data_in_func(ti_id, builder.func);
        let ti_addr = builder.ins().symbol_value(I64, ti_gv);
        let alloc_ref = module.declare_func_in_func(runtime.alloc_typed, builder.func);
        let call = builder.ins().call(alloc_ref, &[size, ti_addr]);
        builder.inst_results(call)[0]
    } else {
        return Err(CodegenError::cranelift(
            format!(
                "struct `{}` has no typeinfo emitted ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â 17a should cover every refcounted struct; is this a non-refcounted struct that still hits this path?",
                ty.name
            ),
            span,
        ));
    };

    // Store each field at offset i * STRUCT_FIELD_SLOT_BYTES. Each
    // field arg is lowered as an Owned temp; the store transfers that
    // +1 ownership into the struct ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â no extra retain, no release.
    for (i, arg) in args.iter().enumerate() {
        let value = lower_expr(
            builder,
            arg,
            current_return_ty,
            env,
            scope_stack,
            func_ids_by_def,
            module,
            runtime,
        )?;
        let offset = (i as i32) * STRUCT_FIELD_SLOT_BYTES;
        builder.ins().store(
            cranelift_codegen::ir::MemFlags::trusted(),
            value,
            struct_ptr,
            offset,
        );
    }
    Ok(struct_ptr)
}

pub fn lower_result_constructor(
    builder: &mut FunctionBuilder,
    expr: &IrExpr,
    inner: &IrExpr,
    tag: i64,
    current_return_ty: &Type,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    scope_stack: &Vec<Vec<(LocalId, Variable)>>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<ClValue, CodegenError> {
    if !is_native_result_type(&expr.ty) {
        return Err(CodegenError::not_supported(
            "`Result<T, E>` construction outside the supported native subset ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â native lowering currently supports one-word payload shapes only",
            expr.span,
        ));
    }

    let payload_ty = match (&expr.ty, tag) {
        (Type::Result(ok, _), RESULT_TAG_OK) => &**ok,
        (Type::Result(_, err), RESULT_TAG_ERR) => &**err,
        (Type::Result(_, _), _) => {
            return Err(CodegenError::cranelift(
                format!("invalid native Result tag {tag}"),
                expr.span,
            ))
        }
        _ => {
            return Err(CodegenError::cranelift(
                format!(
                    "Result constructor expression has non-Result type `{}` ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â typecheck should have caught this",
                    expr.ty.display_name()
                ),
                expr.span,
            ))
        }
    };

    let payload = lower_expr(
        builder,
        inner,
        current_return_ty,
        env,
        scope_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;
    emit_result_wrapper_value(
        builder, module, runtime, &expr.ty, payload, payload_ty, tag, expr.span,
    )
}

pub fn emit_result_wrapper_value(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    result_ty: &Type,
    payload: ClValue,
    payload_ty: &Type,
    tag: i64,
    span: Span,
) -> Result<ClValue, CodegenError> {
    let result_ti_id = runtime
        .result_typeinfos
        .get(result_ty)
        .copied()
        .ok_or_else(|| {
            CodegenError::cranelift(
                format!(
                    "no typeinfo pre-emitted for Result type `{}`",
                    result_ty.display_name()
                ),
                span,
            )
        })?;
    let size = builder.ins().iconst(I64, RESULT_PAYLOAD_BYTES);
    let ti_gv = module.declare_data_in_func(result_ti_id, builder.func);
    let ti_addr = builder.ins().symbol_value(I64, ti_gv);
    let alloc_ref = module.declare_func_in_func(runtime.alloc_typed, builder.func);
    let call = builder.ins().call(alloc_ref, &[size, ti_addr]);
    let result_ptr = builder.inst_results(call)[0];
    let tag_val = builder.ins().iconst(I64, tag);
    builder.ins().store(
        cranelift_codegen::ir::MemFlags::trusted(),
        tag_val,
        result_ptr,
        RESULT_TAG_OFFSET,
    );
    let payload_cl_ty = cl_type_for(payload_ty, span)?;
    builder.ins().store(
        cranelift_codegen::ir::MemFlags::trusted(),
        payload,
        result_ptr,
        RESULT_PAYLOAD_OFFSET,
    );
    // Keep `payload_cl_ty` used explicitly for the correctness check
    // above: mismatched payload widths should fail before store.
    let _ = payload_cl_ty;
    Ok(result_ptr)
}

pub fn emit_option_wrapper_value(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    option_ty: &Type,
    payload: ClValue,
    payload_ty: &Type,
    span: Span,
) -> Result<ClValue, CodegenError> {
    let option_ti_id = runtime
        .option_typeinfos
        .get(option_ty)
        .copied()
        .ok_or_else(|| {
            CodegenError::cranelift(
                format!(
                    "no typeinfo pre-emitted for wide Option type `{}`",
                    option_ty.display_name()
                ),
                span,
            )
        })?;
    let size = builder.ins().iconst(I64, OPTION_PAYLOAD_BYTES);
    let ti_gv = module.declare_data_in_func(option_ti_id, builder.func);
    let ti_addr = builder.ins().symbol_value(I64, ti_gv);
    let alloc_ref = module.declare_func_in_func(runtime.alloc_typed, builder.func);
    let call = builder.ins().call(alloc_ref, &[size, ti_addr]);
    let option_ptr = builder.inst_results(call)[0];
    let payload_cl_ty = cl_type_for(payload_ty, span)?;
    builder.ins().store(
        cranelift_codegen::ir::MemFlags::trusted(),
        payload,
        option_ptr,
        OPTION_PAYLOAD_OFFSET,
    );
    let _ = payload_cl_ty;
    Ok(option_ptr)
}

pub fn lower_string_literal(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    s: &str,
    span: Span,
) -> Result<ClValue, CodegenError> {
    let id = runtime.next_literal_id();
    let symbol_name = format!("corvid_lit_{id}");
    let bytes = s.as_bytes();
    let len = bytes.len() as i64;
    let total = 32 + bytes.len();
    let mut data = vec![0u8; total];
    // refcount = i64::MIN (immortal)
    data[0..8].copy_from_slice(&i64::MIN.to_le_bytes());
    // typeinfo_ptr (offset 8) ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â relocation below points it at
    // `corvid_typeinfo_String` so runtime tracers can dispatch
    // uniformly through the same typeinfo path as heap-allocated
    // strings.
    // bytes_ptr placeholder at offset 16 ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â written by the relocation
    // length at offset 24
    data[24..32].copy_from_slice(&len.to_le_bytes());
    // bytes at offset 32
    if !bytes.is_empty() {
        data[32..].copy_from_slice(bytes);
    }

    let data_id = module
        .declare_data(&symbol_name, Linkage::Local, false, false)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare literal `{symbol_name}`: {e}"), span)
        })?;
    let mut desc = DataDescription::new();
    desc.set_align(8);
    desc.define(data.into_boxed_slice());
    // typeinfo_ptr at offset 8 ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢ &corvid_typeinfo_String
    let ti_gv = module.declare_data_in_data(runtime.string_typeinfo, &mut desc);
    desc.write_data_addr(8, ti_gv, 0);
    // Self-relative relocation: at offset 16, write the address of
    // (this same symbol + 32) so `bytes_ptr` points at the inline bytes.
    let self_gv = module.declare_data_in_data(data_id, &mut desc);
    desc.write_data_addr(16, self_gv, 32);
    module.define_data(data_id, &desc).map_err(|e| {
        CodegenError::cranelift(format!("define literal `{symbol_name}`: {e}"), span)
    })?;

    // The String value is the address of the descriptor (symbol + 16),
    // matching what `corvid_alloc` returns for heap strings.
    let gv = module.declare_data_in_func(data_id, builder.func);
    let symbol_addr = builder.ins().symbol_value(I64, gv);
    Ok(builder.ins().iadd_imm(symbol_addr, 16))
}
