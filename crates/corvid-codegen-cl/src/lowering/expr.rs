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


/// Strict (eager) binary operator lowering: arithmetic and comparison
/// for both `Int` and `Float`. Mixed `Int + Float` operands are
/// promoted to `F64` first (matches the interpreter's widening rule).
/// `Int` arithmetic traps on overflow / div-zero; `Float` follows IEEE
/// 754 (no trap, NaN/Inf propagate naturally).
pub(super) fn lower_binop_strict(
    builder: &mut FunctionBuilder,
    op: BinaryOp,
    l: ClValue,
    r: ClValue,
    span: Span,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<ClValue, CodegenError> {
    // Promote mixed Int + Float operands to F64 ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â same widening the
    // interpreter applies in `eval_arithmetic`.
    let (l, r, dom) = promote_arith(builder, l, r, span)?;

    match (op, dom) {
        // ---- Int arithmetic, overflow-trapping ------------------------
        (BinaryOp::Add, ArithDomain::Int) => {
            with_overflow_trap(builder, l, r, module, runtime, |b| {
                b.ins().sadd_overflow(l, r)
            })
        }
        (BinaryOp::Sub, ArithDomain::Int) => {
            with_overflow_trap(builder, l, r, module, runtime, |b| {
                b.ins().ssub_overflow(l, r)
            })
        }
        (BinaryOp::Mul, ArithDomain::Int) => {
            with_overflow_trap(builder, l, r, module, runtime, |b| {
                b.ins().smul_overflow(l, r)
            })
        }
        (BinaryOp::Div, ArithDomain::Int) => {
            trap_on_zero(builder, r, module, runtime);
            Ok(builder.ins().sdiv(l, r))
        }
        (BinaryOp::Mod, ArithDomain::Int) => {
            trap_on_zero(builder, r, module, runtime);
            Ok(builder.ins().srem(l, r))
        }

        // ---- Float arithmetic, IEEE 754 (no trap) ---------------------
        (BinaryOp::Add, ArithDomain::Float) => Ok(builder.ins().fadd(l, r)),
        (BinaryOp::Sub, ArithDomain::Float) => Ok(builder.ins().fsub(l, r)),
        (BinaryOp::Mul, ArithDomain::Float) => Ok(builder.ins().fmul(l, r)),
        (BinaryOp::Div, ArithDomain::Float) => Ok(builder.ins().fdiv(l, r)),
        (BinaryOp::Mod, ArithDomain::Float) => {
            // Cranelift has no `frem`. Compute `a - trunc(a / b) * b`,
            // matching Rust's `f64::%` semantics.
            let div = builder.ins().fdiv(l, r);
            let trunc = builder.ins().trunc(div);
            let mul = builder.ins().fmul(trunc, r);
            Ok(builder.ins().fsub(l, mul))
        }

        // ---- Comparisons -----------------------------------------------
        (BinaryOp::Eq, ArithDomain::Int) => Ok(builder.ins().icmp(IntCC::Equal, l, r)),
        (BinaryOp::NotEq, ArithDomain::Int) => Ok(builder.ins().icmp(IntCC::NotEqual, l, r)),
        (BinaryOp::Lt, ArithDomain::Int) => Ok(builder.ins().icmp(IntCC::SignedLessThan, l, r)),
        (BinaryOp::LtEq, ArithDomain::Int) => {
            Ok(builder.ins().icmp(IntCC::SignedLessThanOrEqual, l, r))
        }
        (BinaryOp::Gt, ArithDomain::Int) => {
            Ok(builder.ins().icmp(IntCC::SignedGreaterThan, l, r))
        }
        (BinaryOp::GtEq, ArithDomain::Int) => {
            Ok(builder.ins().icmp(IntCC::SignedGreaterThanOrEqual, l, r))
        }
        // Float comparisons: IEEE-correct NaN handling. Rust's `==`
        // returns false when either side is NaN; `!=` returns true.
        // FloatCC::Equal matches `==`; UnorderedOrNotEqual matches `!=`.
        // The ordered LessThan / LessThanOrEqual / GreaterThan /
        // GreaterThanOrEqual variants all return false on NaN, matching
        // Rust's lt/le/gt/ge.
        (BinaryOp::Eq, ArithDomain::Float) => Ok(builder.ins().fcmp(FloatCC::Equal, l, r)),
        (BinaryOp::NotEq, ArithDomain::Float) => {
            Ok(builder.ins().fcmp(FloatCC::NotEqual, l, r))
        }
        (BinaryOp::Lt, ArithDomain::Float) => Ok(builder.ins().fcmp(FloatCC::LessThan, l, r)),
        (BinaryOp::LtEq, ArithDomain::Float) => {
            Ok(builder.ins().fcmp(FloatCC::LessThanOrEqual, l, r))
        }
        (BinaryOp::Gt, ArithDomain::Float) => {
            Ok(builder.ins().fcmp(FloatCC::GreaterThan, l, r))
        }
        (BinaryOp::GtEq, ArithDomain::Float) => {
            Ok(builder.ins().fcmp(FloatCC::GreaterThanOrEqual, l, r))
        }

        (BinaryOp::And | BinaryOp::Or, _) => {
            let _ = span;
            unreachable!("and/or is short-circuited upstream and never reaches lower_binop_strict")
        }
    }
}

/// Which arithmetic family this binop operates in after operand
/// promotion. `Bool == Bool` lands in `Int` because `I8` is integer
/// from Cranelift's perspective.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArithDomain {
    Int,
    Float,
}


/// Layout (single allocation):
/// ```text
///   offset 0:  refcount (8) = i64::MIN  (immortal sentinel)
///   offset 8:  reserved (8) = 0
///   offset 16: bytes_ptr (8) = self + 32 (relocated)
///   offset 24: length (8)
///   offset 32: bytes (length bytes)
/// ```
/// The compiled value is `symbol_value(self) + 16`, pointing at the
/// descriptor (matching what `corvid_alloc` returns for heap strings).
pub(super) fn lower_string_literal(
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
    module
        .define_data(data_id, &desc)
        .map_err(|e| CodegenError::cranelift(format!("define literal `{symbol_name}`: {e}"), span))?;

    // The String value is the address of the descriptor (symbol + 16),
    // matching what `corvid_alloc` returns for heap strings.
    let gv = module.declare_data_in_func(data_id, builder.func);
    let symbol_addr = builder.ins().symbol_value(I64, gv);
    Ok(builder.ins().iadd_imm(symbol_addr, 16))
}

/// Implicit-promote mixed `Int + Float` operands to `Float`. Same rule
/// as the interpreter's `eval_arithmetic`. Returns the (possibly
/// promoted) operands and the resulting arithmetic domain.
fn promote_arith(
    builder: &mut FunctionBuilder,
    l: ClValue,
    r: ClValue,
    span: Span,
) -> Result<(ClValue, ClValue, ArithDomain), CodegenError> {
    let lt = builder.func.dfg.value_type(l);
    let rt = builder.func.dfg.value_type(r);
    if lt == F64 && rt == F64 {
        return Ok((l, r, ArithDomain::Float));
    }
    if lt == I64 && rt == I64 {
        return Ok((l, r, ArithDomain::Int));
    }
    // Bool == Bool is Int domain ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â both sides are I8.
    if lt == I8 && rt == I8 {
        return Ok((l, r, ArithDomain::Int));
    }
    if lt == I64 && rt == F64 {
        let l_promoted = builder.ins().fcvt_from_sint(F64, l);
        return Ok((l_promoted, r, ArithDomain::Float));
    }
    if lt == F64 && rt == I64 {
        let r_promoted = builder.ins().fcvt_from_sint(F64, r);
        return Ok((l, r_promoted, ArithDomain::Float));
    }
    Err(CodegenError::cranelift(
        format!(
            "unsupported operand width combination for binop: {lt:?} and {rt:?} ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â typecheck should have caught this"
        ),
        span,
    ))
}

/// Lower unary operators.
///
/// - `Not` flips a Bool via `icmp_eq(v, 0)` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â 0ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢1, 1ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢0 ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â and produces `I8`.
/// - `Neg` on `Int` is `0 - x` with overflow trap, matching the
///   interpreter's `checked_neg` semantics for `i64::MIN`.
pub(super) fn lower_unop(
    builder: &mut FunctionBuilder,
    op: UnaryOp,
    v: ClValue,
    span: Span,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<ClValue, CodegenError> {
    let vt = builder.func.dfg.value_type(v);
    match op {
        UnaryOp::Not => {
            let zero = builder.ins().iconst(I8, 0);
            Ok(builder.ins().icmp(IntCC::Equal, v, zero))
        }
        UnaryOp::Neg if vt == F64 => {
            // Float negation is IEEE ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â flips the sign bit, no trap. NaN
            // negation produces NaN with the sign flipped, also fine.
            Ok(builder.ins().fneg(v))
        }
        UnaryOp::Neg if vt == I64 => {
            // Int `-x` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â°ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¡ `0 - x`, trap on overflow (only at i64::MIN).
            let zero = builder.ins().iconst(I64, 0);
            with_overflow_trap(builder, zero, v, module, runtime, |b| {
                b.ins().ssub_overflow(zero, v)
            })
        }
        UnaryOp::Neg => Err(CodegenError::cranelift(
            format!("unary `-` applied to value of width {vt:?} ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â typecheck should have caught this"),
            span,
        )),
    }
}

/// Short-circuit `and`/`or`.
///
/// Implementation: evaluate the left operand; branch on it. The "short
/// path" skips the right operand entirely and jumps to the merge block
/// with a constant (0 for `and`, 1 for `or`). The "evaluate path"
/// executes the right operand and forwards its value. Merge block
/// receives an `I8` block parameter carrying the chosen result.
pub(super) fn lower_short_circuit(
    builder: &mut FunctionBuilder,
    op: BinaryOp,
    left: &IrExpr,
    right: &IrExpr,
    current_return_ty: &Type,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    scope_stack: &Vec<Vec<(LocalId, Variable)>>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<ClValue, CodegenError> {
    let l = lower_expr(
        builder,
        left,
        current_return_ty,
        env,
        scope_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;

    let right_block = builder.create_block();
    let merge_block = builder.create_block();
    let result = builder.append_block_param(merge_block, I8);

    match op {
        BinaryOp::And => {
            // l != 0 ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢ eval right; l == 0 ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢ short-circuit to false.
            let short_val = builder.ins().iconst(I8, 0);
            builder
                .ins()
                .brif(l, right_block, &[], merge_block, &[short_val.into()]);
        }
        BinaryOp::Or => {
            // l != 0 ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢ short-circuit to true; l == 0 ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢ eval right.
            let short_val = builder.ins().iconst(I8, 1);
            builder
                .ins()
                .brif(l, merge_block, &[short_val.into()], right_block, &[]);
        }
        _ => unreachable!("lower_short_circuit only handles And/Or"),
    }

    builder.switch_to_block(right_block);
    builder.seal_block(right_block);
    let r = lower_expr(
        builder,
        right,
        current_return_ty,
        env,
        scope_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;
    builder.ins().jump(merge_block, &[r.into()]);

    builder.switch_to_block(merge_block);
    builder.seal_block(merge_block);
    Ok(result)
}


/// Run an overflow-producing Cranelift op, branch to an overflow handler
/// block on the flag, and return the sum/diff/product value.
fn with_overflow_trap<F>(
    builder: &mut FunctionBuilder,
    _l: ClValue,
    _r: ClValue,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    op: F,
) -> Result<ClValue, CodegenError>
where
    F: FnOnce(&mut FunctionBuilder) -> (ClValue, ClValue),
{
    let (result, overflow) = op(builder);
    let overflow_block = builder.create_block();
    let cont_block = builder.create_block();
    builder
        .ins()
        .brif(overflow, overflow_block, &[], cont_block, &[]);

    builder.switch_to_block(overflow_block);
    builder.seal_block(overflow_block);
    let callee_ref = module.declare_func_in_func(runtime.overflow, builder.func);
    builder.ins().call(callee_ref, &[]);
    builder.ins().trap(cranelift_codegen::ir::TrapCode::INTEGER_OVERFLOW);

    builder.switch_to_block(cont_block);
    builder.seal_block(cont_block);
    Ok(result)
}

fn trap_on_zero(
    builder: &mut FunctionBuilder,
    divisor: ClValue,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) {
    let zero = builder.ins().iconst(I64, 0);
    let is_zero = builder.ins().icmp(IntCC::Equal, divisor, zero);
    let trap_block = builder.create_block();
    let cont_block = builder.create_block();
    builder
        .ins()
        .brif(is_zero, trap_block, &[], cont_block, &[]);
    builder.switch_to_block(trap_block);
    builder.seal_block(trap_block);
    let callee_ref = module.declare_func_in_func(runtime.overflow, builder.func);
    builder.ins().call(callee_ref, &[]);
    builder.ins().trap(cranelift_codegen::ir::TrapCode::INTEGER_OVERFLOW);
    builder.switch_to_block(cont_block);
    builder.seal_block(cont_block);
}

