use super::*;

pub(super) fn lower_container_maybe_borrowed(
    builder: &mut FunctionBuilder,
    expr: &IrExpr,
    current_return_ty: &Type,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    scope_stack: &Vec<Vec<(LocalId, Variable)>>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<(ClValue, bool), CodegenError> {
    if let IrExprKind::Local { local_id, name } = &expr.kind {
        let (var, _ty) = env.get(local_id).ok_or_else(|| {
            CodegenError::cranelift(
                format!("no variable for local `{name}` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â compiler bug"),
                expr.span,
            )
        })?;
        let v = builder.use_var(*var);
        return Ok((v, true));
    }
    let v = lower_expr(
        builder,
        expr,
        current_return_ty,
        env,
        scope_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;
    Ok((v, false))
}

/// Lower a string-typed operand in a borrow position
/// (e.g. an operand of a consuming String BinOp: concat, equality, or
/// ordering compare). Returns `(value, borrowed)`:
///
///   * `borrowed = true` iff the operand was a bare `IrExprKind::Local`
///     ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â in that case we skip the ownership-conversion retain that
///     `lower_expr` would normally emit, and the caller must NOT
///     release the value afterward. The returned `ClValue` is a
///     borrow of the binding's current refcount (caller's scope
///     still governs the Drop).
///   * `borrowed = false` for every other expression shape ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â the
///     value is a fresh Owned +1 produced by `lower_expr`, and the
///     caller is responsible for the corresponding release.
///
/// Safe because the String runtime helpers (`corvid_string_concat`,
/// `corvid_string_eq`, `corvid_string_cmp`) only read their inputs ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â
/// they never mutate the operand refcount nor store the pointer. So
/// passing a borrow vs. an Owned +1 is indistinguishable from the
/// helper's perspective; the only observable difference is the net
/// zero retain/release pair we eliminated.
pub(super) fn lower_string_operand_maybe_borrowed(
    builder: &mut FunctionBuilder,
    expr: &IrExpr,
    current_return_ty: &Type,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    scope_stack: &Vec<Vec<(LocalId, Variable)>>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<(ClValue, bool), CodegenError> {
    // Under the .6d unified pass: take the non-peephole path for bare
    // Local operands. If we returned `borrowed=true`, BinOp would skip
    // the release and the caller's +1 would leak ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â the pass's
    // analysis treats BinOp operands as Owned-consumed and does NOT
    // schedule a Drop for an operand that's consumed (there's no
    // later use), so codegen MUST release to retire the +1.
    if !runtime.dup_drop_enabled {
        if let IrExprKind::Local { local_id, name } = &expr.kind {
            let (var, _ty) = env.get(local_id).ok_or_else(|| {
                CodegenError::cranelift(
                    format!("no variable for local `{name}` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â compiler bug"),
                    expr.span,
                )
            })?;
            let v = builder.use_var(*var);
            return Ok((v, true));
        }
    }
    let v = lower_expr(
        builder,
        expr,
        current_return_ty,
        env,
        scope_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;
    Ok((v, false))
}

/// Lower a string BinOp whose operands may be borrowed
/// rather than Owned. Mirrors `lower_string_binop` but conditionally
/// skips the post-op release for any operand that was passed as a
/// borrow (no +1 was produced; nothing to release).
#[allow(clippy::too_many_arguments)]
pub(super) fn lower_string_binop_with_ownership(
    builder: &mut FunctionBuilder,
    op: BinaryOp,
    l: ClValue,
    r: ClValue,
    l_borrowed: bool,
    r_borrowed: bool,
    span: Span,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<ClValue, CodegenError> {
    match op {
        BinaryOp::Add => {
            let callee = module.declare_func_in_func(runtime.string_concat, builder.func);
            let call = builder.ins().call(callee, &[l, r]);
            let result = builder.inst_results(call)[0];
            // BinOp consumes its refcounted operands by convention:
            // the pass's Dup-before-non-last-use supplies the extra
            // +1 for multi-read Locals; the single +1 flowing in as
            // each operand is retired here. Under both flag modes.
            if !l_borrowed {
                emit_release(builder, module, runtime, l);
            }
            if !r_borrowed {
                emit_release(builder, module, runtime, r);
            }
            Ok(result)
        }
        BinaryOp::Eq | BinaryOp::NotEq => {
            let callee = module.declare_func_in_func(runtime.string_eq, builder.func);
            let call = builder.ins().call(callee, &[l, r]);
            let eq_i64 = builder.inst_results(call)[0];
            let eq_i8 = builder.ins().ireduce(I8, eq_i64);
            let result = if matches!(op, BinaryOp::Eq) {
                eq_i8
            } else {
                let zero = builder.ins().iconst(I8, 0);
                builder.ins().icmp(IntCC::Equal, eq_i8, zero)
            };
            // BinOp consumes its refcounted operands by convention:
            // the pass's Dup-before-non-last-use supplies the extra
            // +1 for multi-read Locals; the single +1 flowing in as
            // each operand is retired here. Under both flag modes.
            if !l_borrowed {
                emit_release(builder, module, runtime, l);
            }
            if !r_borrowed {
                emit_release(builder, module, runtime, r);
            }
            Ok(result)
        }
        BinaryOp::Lt | BinaryOp::LtEq | BinaryOp::Gt | BinaryOp::GtEq => {
            let callee = module.declare_func_in_func(runtime.string_cmp, builder.func);
            let call = builder.ins().call(callee, &[l, r]);
            let cmp_i64 = builder.inst_results(call)[0];
            let zero = builder.ins().iconst(I64, 0);
            let cc = match op {
                BinaryOp::Lt => IntCC::SignedLessThan,
                BinaryOp::LtEq => IntCC::SignedLessThanOrEqual,
                BinaryOp::Gt => IntCC::SignedGreaterThan,
                BinaryOp::GtEq => IntCC::SignedGreaterThanOrEqual,
                _ => unreachable!(),
            };
            let result = builder.ins().icmp(cc, cmp_i64, zero);
            // BinOp consumes its refcounted operands by convention:
            // the pass's Dup-before-non-last-use supplies the extra
            // +1 for multi-read Locals; the single +1 flowing in as
            // each operand is retired here. Under both flag modes.
            if !l_borrowed {
                emit_release(builder, module, runtime, l);
            }
            if !r_borrowed {
                emit_release(builder, module, runtime, r);
            }
            Ok(result)
        }
        BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => Err(
            CodegenError::not_supported(
                format!("`{op:?}` is not defined for `String` operands"),
                span,
            ),
        ),
        BinaryOp::And | BinaryOp::Or => {
            unreachable!("and/or is short-circuited upstream and never reaches string BinOp")
        }
    }
}


/// Lower a struct constructor: allocate, store each field at its
/// offset, return the struct pointer (refcount = 1, Owned).
///
/// Each constructor argument is lowered as an Owned temp; the store
/// transfers ownership into the struct (no extra retain, no release).
/// If the struct has a per-type destructor (at least one refcounted
/// field), we use `corvid_alloc_with_destructor` so `corvid_release`
/// will release those fields when the struct is eventually freed.
pub(super) fn lower_struct_constructor(
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

pub(super) fn lower_result_constructor(
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
    emit_result_wrapper_value(builder, module, runtime, &expr.ty, payload, payload_ty, tag, expr.span)
}

pub(super) fn emit_result_wrapper_value(
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

pub(super) fn emit_option_wrapper_value(
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

