use super::*;

mod binop;
mod constructors;
pub(super) use binop::{
    lower_binop_strict, lower_binop_wrapping, lower_short_circuit, lower_unop, lower_unop_wrapping,
};
pub(super) use constructors::{
    emit_grounded_value_attestation, emit_option_wrapper_value, emit_result_wrapper_value,
    lower_result_constructor, lower_string_literal, lower_struct_constructor,
};

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
        BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => {
            Err(CodegenError::not_supported(
                format!("`{op:?}` is not defined for `String` operands"),
                span,
            ))
        }
        BinaryOp::And | BinaryOp::Or => {
            unreachable!("and/or is short-circuited upstream and never reaches string BinOp")
        }
    }
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
    builder
        .ins()
        .trap(cranelift_codegen::ir::TrapCode::INTEGER_OVERFLOW);

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
    builder
        .ins()
        .trap(cranelift_codegen::ir::TrapCode::INTEGER_OVERFLOW);
    builder.switch_to_block(cont_block);
    builder.seal_block(cont_block);
}

pub(super) fn lower_expr(
    builder: &mut FunctionBuilder,
    expr: &IrExpr,
    current_return_ty: &Type,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    scope_stack: &Vec<Vec<(LocalId, Variable)>>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<ClValue, CodegenError> {
    match &expr.kind {
        IrExprKind::Literal(IrLiteral::Int(n)) => Ok(builder.ins().iconst(I64, *n)),
        IrExprKind::Literal(IrLiteral::Bool(b)) => {
            Ok(builder.ins().iconst(I8, if *b { 1 } else { 0 }))
        }
        IrExprKind::Literal(IrLiteral::Float(n)) => Ok(builder.ins().f64const(*n)),
        IrExprKind::Literal(IrLiteral::String(s)) => {
            lower_string_literal(builder, module, runtime, s, expr.span)
        }
        IrExprKind::Literal(IrLiteral::Nothing) => Err(CodegenError::not_supported(
            "`nothing` literal is not supported yet",
            expr.span,
        )),
        IrExprKind::Local { local_id, name } => {
            let (var, _ty) = env.get(local_id).ok_or_else(|| {
                CodegenError::cranelift(
                    format!("no variable for local `{name}` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â compiler bug"),
                    expr.span,
                )
            })?;
            let v = builder.use_var(*var);
            // Three-state ownership: `use_var` produces a Borrowed
            // reference. Convert to Owned by retaining so the caller
            // (bind / return / call-arg / discard) can dispose of it
            // uniformly. For non-refcounted types this is a no-op.
            //
            // Under the .6d unified pass, we rely on the pass's Dup
            // insertion at non-last consuming uses instead of
            // retaining on every read ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â the last use of a local
            // consumes the existing +1 directly.
            if !runtime.dup_drop_enabled && is_refcounted_type(&expr.ty) {
                emit_retain(builder, module, runtime, v);
            }
            Ok(v)
        }
        IrExprKind::Decl { .. } => Err(CodegenError::not_supported(
            "declaration reference (imports/functions as values)",
            expr.span,
        )),
        IrExprKind::BinOp { op, left, right } => {
            if matches!(op, BinaryOp::And | BinaryOp::Or) {
                return lower_short_circuit(
                    builder,
                    *op,
                    left,
                    right,
                    current_return_ty,
                    env,
                    scope_stack,
                    func_ids_by_def,
                    module,
                    runtime,
                );
            }
            // String operands route through the String runtime helpers
            // and have their own ownership semantics (release inputs
            // after the helper call). Dispatch here so we have access
            // to the IR type information.
            //
            // Borrow-at-use-site peephole for
            // consuming string BinOps. If an operand is a bare
            // `IrExprKind::Local` of a refcounted String, we lower
            // the Local WITHOUT emitting the ownership-conversion
            // retain, and correspondingly skip the post-op release.
            // The comparison/concat runtime helpers only read their
            // inputs (they don't mutate the operand's refcount or
            // store the pointer), so reading the Variable directly
            // is semantically equivalent to retain-then-release with
            // zero observable refcount net effect. The Local's
            // binding remains Live in its scope, governed by the
            // scope-exit release that's already in place.
            //
            // Non-bare-Local operands (literals, nested expressions,
            // call results) still produce an Owned +1 and are
            // released by the helper as before ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â the original
            // ownership contract.
            if matches!(&left.ty, Type::String) && matches!(&right.ty, Type::String) {
                let (l, l_borrowed) =
                    lower_string_operand_maybe_borrowed(builder, left, current_return_ty, env, scope_stack, func_ids_by_def, module, runtime)?;
                let (r, r_borrowed) =
                    lower_string_operand_maybe_borrowed(builder, right, current_return_ty, env, scope_stack, func_ids_by_def, module, runtime)?;
                return lower_string_binop_with_ownership(
                    builder, *op, l, r, l_borrowed, r_borrowed, expr.span, module, runtime,
                );
            }
            let l = lower_expr(builder, left, current_return_ty, env, scope_stack, func_ids_by_def, module, runtime)?;
            let r = lower_expr(builder, right, current_return_ty, env, scope_stack, func_ids_by_def, module, runtime)?;
            let result = lower_binop_strict(builder, *op, l, r, expr.span, module, runtime)?;
            if is_refcounted_type(&left.ty) {
                emit_release(builder, module, runtime, l);
            }
            if is_refcounted_type(&right.ty) {
                emit_release(builder, module, runtime, r);
            }
            Ok(result)
        }
        IrExprKind::WrappingBinOp { op, left, right } => {
            let l = lower_expr(builder, left, current_return_ty, env, scope_stack, func_ids_by_def, module, runtime)?;
            let r = lower_expr(builder, right, current_return_ty, env, scope_stack, func_ids_by_def, module, runtime)?;
            lower_binop_wrapping(builder, *op, l, r, expr.span)
        }
        IrExprKind::UnOp { op, operand } => {
            let v = lower_expr(builder, operand, current_return_ty, env, scope_stack, func_ids_by_def, module, runtime)?;
            lower_unop(builder, *op, v, expr.span, module, runtime)
        }
        IrExprKind::WrappingUnOp { op, operand } => {
            let v = lower_expr(builder, operand, current_return_ty, env, scope_stack, func_ids_by_def, module, runtime)?;
            lower_unop_wrapping(builder, *op, v, expr.span)
        }
        IrExprKind::UnwrapGrounded { value } => {
            lower_expr(
                builder,
                value,
                current_return_ty,
                env,
                scope_stack,
                func_ids_by_def,
                module,
                runtime,
            )
        }
        IrExprKind::Call { kind, callee_name, args } => match kind {
            IrCallKind::Agent { def_id } => {
                let callee_id = func_ids_by_def.get(def_id).ok_or_else(|| {
                    CodegenError::cranelift(
                        format!("agent `{callee_name}` has no declared function id"),
                        expr.span,
                    )
                })?;
                let callee_ref = module.declare_func_in_func(*callee_id, builder.func);
                // Caller-side borrow peephole. For
                // each refcounted arg whose callee slot is Borrowed
                // (per the callee's `borrow_sig` populated by the
                // ownership pass) AND whose argument expression is a
                // bare `IrExprKind::Local`, we lower the Local
                // directly from its Variable without a retain, and
                // skip the paired post-call release. The callee
                // already skips its entry-retain + scope-exit release
                // for Borrowed params (17b-1b.1), so the whole
                // retain/release pair collapses on both sides.
                //
                // For Owned callee slots OR non-bare-Local args, the
                // original +0 ABI applies: lower_expr produces a +1
                // Owned, callee retains on entry (17b-1b.1 keeps this
                // for Owned), caller releases its +1 after the call.
                let callee_sig = runtime.agent_borrow_sigs.get(def_id);
                let mut arg_vals = Vec::with_capacity(args.len());
                // `needs_post_release[i]` = true iff the caller
                // produced a +1 that must be released post-call
                // (false when borrow peephole applies).
                let mut needs_post_release: Vec<bool> = Vec::with_capacity(args.len());
                for (i, a) in args.iter().enumerate() {
                    let is_ref = is_refcounted_type(&a.ty);
                    let callee_borrowed = callee_sig
                        .and_then(|s| s.get(i).copied())
                        .map(|b| matches!(b, corvid_ir::ParamBorrow::Borrowed))
                        .unwrap_or(false);
                    let arg_is_bare_local =
                        matches!(&a.kind, IrExprKind::Local { .. });
                    if is_ref && callee_borrowed && arg_is_bare_local {
                        // Borrow path: read the Variable directly, no retain.
                        if let IrExprKind::Local { local_id, name } = &a.kind {
                            let (var, _ty) = env.get(local_id).ok_or_else(|| {
                                CodegenError::cranelift(
                                    format!("no variable for local `{name}` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â compiler bug"),
                                    a.span,
                                )
                            })?;
                            let v = builder.use_var(*var);
                            arg_vals.push(v);
                            needs_post_release.push(false);
                            continue;
                        }
                    }
                    // Normal path: +1 Owned, caller releases after call.
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
                    arg_vals.push(v);
                    needs_post_release.push(is_ref);
                }
                let call = builder.ins().call(callee_ref, &arg_vals);
                let results = builder.inst_results(call);
                if matches!(expr.ty, Type::Nothing) {
                    if !results.is_empty() {
                        return Err(CodegenError::cranelift(
                            format!(
                                "agent `{callee_name}` returned {} values; native lowering expects 0 for `Nothing`",
                                results.len()
                            ),
                            expr.span,
                        ));
                    }
                } else if results.len() != 1 {
                    return Err(CodegenError::cranelift(
                        format!(
                            "agent `{callee_name}` returned {} values; native lowering expects exactly 1",
                            results.len()
                        ),
                        expr.span,
                    ));
                }
                let result = if matches!(expr.ty, Type::Nothing) {
                    builder.ins().iconst(I64, 0)
                } else {
                    results[0]
                };
                if !runtime.dup_drop_enabled {
                    for (v, needs) in arg_vals.iter().zip(needs_post_release.iter()) {
                        if *needs {
                            emit_release(builder, module, runtime, *v);
                        }
                    }
                }
                Ok(result)
            }
            IrCallKind::Tool { def_id, .. } => {
                // Emit a direct typed call to the tool's
                // `#[tool]`-generated wrapper symbol. No JSON, no
                // dynamic dispatch ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â just a `call` instruction against
                // a named import. Link-time symbol resolution catches
                // missing tool implementations; Cranelift-level
                // type-matching catches wrong-type mismatches at
                // parity-harness or codegen time.
                let tool = runtime.ir_tools.get(def_id).cloned().ok_or_else(|| {
                    CodegenError::cranelift(
                        format!(
                            "tool `{callee_name}` metadata missing from ir_tools ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â declare-pass invariant violated"
                        ),
                        expr.span,
                    )
                })?;

                // Arity cross-check ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â belt-and-braces vs. the
                // typechecker's check that already ran.
                if tool.params.len() != args.len() {
                    return Err(CodegenError::cranelift(
                        format!(
                            "tool `{callee_name}` declared with {} param(s) but called with {}",
                            tool.params.len(),
                            args.len()
                        ),
                        expr.span,
                    ));
                }

                // Declare or re-use the wrapper-symbol import.
                let wrapper_id = {
                    let mut cache = runtime.tool_wrapper_ids.borrow_mut();
                    if let Some(id) = cache.get(def_id) {
                        *id
                    } else {
                        let mut sig = module.make_signature();
                        for p in &tool.params {
                            sig.params.push(AbiParam::new(cl_type_for(&p.ty, p.span)?));
                        }
                        if !matches!(tool.return_ty, Type::Nothing) {
                            sig.returns
                                .push(AbiParam::new(cl_type_for(&tool.return_ty, tool.span)?));
                        }
                        let symbol = tool_wrapper_symbol(&tool.name);
                        let id = module
                            .declare_function(&symbol, Linkage::Import, &sig)
                            .map_err(|e| {
                                CodegenError::cranelift(
                                    format!(
                                        "declare tool wrapper `{symbol}`: {e}"
                                    ),
                                    expr.span,
                                )
                            })?;
                        cache.insert(*def_id, id);
                        id
                    }
                };

                // Tool-call ABI: refcount lifecycle matches
                // the agent-call convention ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â caller
                // produces an Owned (+1) refcounted arg via the
                // existing `lower_expr` path (use_var retains to
                // convert BorrowedÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢Owned), the `#[tool]` wrapper
                // reads bytes without touching refcount
                // (`abi::FromCorvidAbi for String` is borrow-only so
                // the wrapper neither retains nor releases), and
                // after the call returns the caller releases its +1.
                // Net effect: one retain + one release around the
                // call = zero net refcount change, which is what a
                // borrow-style FFI boundary should look like. Without
                // the release, the +1 leaks.
                let mut arg_vals = Vec::with_capacity(args.len());
                let mut arg_refcounted: Vec<bool> = Vec::with_capacity(args.len());
                for a in args {
                    arg_vals.push(lower_expr(
                        builder,
                        a,
                        current_return_ty,
                        env,
                        scope_stack,
                        func_ids_by_def,
                        module,
                        runtime,
                    )?);
                    arg_refcounted.push(is_refcounted_type(&a.ty));
                }
                let trace_arg_tys = args.iter().map(|arg| arg.ty.clone()).collect::<Vec<_>>();
                let trace_payload =
                    emit_trace_payload(builder, module, runtime, &arg_vals, &trace_arg_tys, expr.span)?;
                let tool_name_val =
                    emit_string_const(builder, module, runtime, &tool.name, expr.span)?;
                let runtime_is_replay_ref =
                    module.declare_func_in_func(runtime.runtime_is_replay, builder.func);
                let runtime_is_replay_call = builder.ins().call(runtime_is_replay_ref, &[]);
                let runtime_is_replay = builder.inst_results(runtime_is_replay_call)[0];
                let replay_b = builder.create_block();
                let live_b = builder.create_block();
                let join_b = builder.create_block();
                let result_ty = if matches!(expr.ty, Type::Nothing) {
                    None
                } else {
                    Some(cl_type_for(&expr.ty, expr.span)?)
                };
                if let Some(result_ty) = result_ty {
                    builder.append_block_param(join_b, result_ty);
                }
                let replay_cond = builder.ins().icmp_imm(IntCC::NotEqual, runtime_is_replay, 0);
                builder
                    .ins()
                    .brif(replay_cond, replay_b, &[], live_b, &[]);

                builder.switch_to_block(replay_b);
                builder.seal_block(replay_b);
                let replay_func = match &expr.ty {
                    Type::Nothing => runtime.replay_tool_call_nothing,
                    Type::Int => runtime.replay_tool_call_int,
                    Type::Bool => runtime.replay_tool_call_bool,
                    Type::Float => runtime.replay_tool_call_float,
                    Type::String => runtime.replay_tool_call_string,
                    Type::Grounded(inner) => match &**inner {
                        Type::Int => runtime.replay_tool_call_int,
                        Type::Bool => runtime.replay_tool_call_bool,
                        Type::Float => runtime.replay_tool_call_float,
                        Type::String => runtime.replay_tool_call_string,
                        _ => {
                            return Err(CodegenError::not_supported(
                                format!(
                                    "native replay for tool `{}` with return type `{}` is not implemented yet",
                                    tool.name,
                                    expr.ty.display_name()
                                ),
                                expr.span,
                            ))
                        }
                    },
                    _ => {
                        return Err(CodegenError::not_supported(
                            format!(
                                "native replay for tool `{}` with return type `{}` is not implemented yet",
                                tool.name,
                                expr.ty.display_name()
                            ),
                            expr.span,
                        ))
                    }
                };
                let replay_ref = module.declare_func_in_func(replay_func, builder.func);
                let replay_call = builder.ins().call(
                    replay_ref,
                    &[
                        tool_name_val,
                        trace_payload.type_tags,
                        trace_payload.count,
                        trace_payload.values_ptr,
                    ],
                );
                if matches!(expr.ty, Type::Nothing) {
                    builder.ins().jump(join_b, &[]);
                } else {
                    let replay_value = builder.inst_results(replay_call)[0];
                    let replay_value = if matches!(expr.ty, Type::Grounded(_)) {
                        emit_grounded_value_attestation(
                            builder,
                            module,
                            runtime,
                            replay_value,
                            &expr.ty,
                            &tool.name,
                            1.0,
                            expr.span,
                        )?
                    } else {
                        replay_value
                    };
                    builder.ins().jump(join_b, &[replay_value]);
                }

                builder.switch_to_block(live_b);
                builder.seal_block(live_b);
                let trace_tool_call_ref =
                    module.declare_func_in_func(runtime.trace_tool_call, builder.func);
                builder.ins().call(
                    trace_tool_call_ref,
                    &[
                        tool_name_val,
                        trace_payload.type_tags,
                        trace_payload.count,
                        trace_payload.values_ptr,
                    ],
                );

                let fref = module.declare_func_in_func(wrapper_id, builder.func);
                let call = builder.ins().call(fref, &arg_vals);
                let result_vals: Vec<ClValue> =
                    builder.inst_results(call).iter().copied().collect();
                let trace_result_ref = match &expr.ty {
                    Type::Nothing => Some(runtime.trace_tool_result_null),
                    Type::Int => Some(runtime.trace_tool_result_int),
                    Type::Bool => Some(runtime.trace_tool_result_bool),
                    Type::Float => Some(runtime.trace_tool_result_float),
                    Type::String => Some(runtime.trace_tool_result_string),
                    Type::Grounded(inner) => match &**inner {
                        Type::Int => Some(runtime.trace_tool_result_int),
                        Type::Bool => Some(runtime.trace_tool_result_bool),
                        Type::Float => Some(runtime.trace_tool_result_float),
                        Type::String => Some(runtime.trace_tool_result_string),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(trace_func) = trace_result_ref {
                    let trace_result_call = module.declare_func_in_func(trace_func, builder.func);
                    let trace_args = if matches!(expr.ty, Type::Nothing) {
                        vec![tool_name_val]
                    } else {
                        vec![tool_name_val, result_vals[0]]
                    };
                    builder.ins().call(trace_result_call, &trace_args);
                }
                let live_result = if matches!(expr.ty, Type::Grounded(_)) {
                    emit_grounded_value_attestation(
                        builder,
                        module,
                        runtime,
                        result_vals[0],
                        &expr.ty,
                        &tool.name,
                        1.0,
                        expr.span,
                    )?
                } else {
                    result_vals[0]
                };
                if matches!(expr.ty, Type::Nothing) {
                    builder.ins().jump(join_b, &[]);
                } else if result_vals.len() == 1 {
                    builder.ins().jump(join_b, &[live_result]);
                } else {
                    return Err(CodegenError::cranelift(
                        format!(
                            "tool `{callee_name}` wrapper returned {} values; expected 1 for type `{}`",
                            result_vals.len(),
                            expr.ty.display_name()
                        ),
                        expr.span,
                    ));
                }

                builder.switch_to_block(join_b);
                builder.seal_block(join_b);
                emit_release(builder, module, runtime, trace_payload.type_tags);

                // Release the +1 we put on each refcounted arg. For
                // literals (refcount = i64::MIN sentinel) this is a
                // no-op; for heap values it decrements the refcount
                // we bumped pre-call.
                if !runtime.dup_drop_enabled {
                    for (v, is_ref) in arg_vals.iter().zip(arg_refcounted.iter()) {
                        if *is_ref {
                            emit_release(builder, module, runtime, *v);
                        }
                    }
                }
                emit_release(builder, module, runtime, tool_name_val);

                // Return-value unpacking. For Nothing-returning tools
                // there's no result to hand back; synthesize a
                // zero-Int so the expr-result contract stays uniform.
                if matches!(expr.ty, Type::Nothing) {
                    Ok(builder.ins().iconst(I64, 0))
                } else {
                    Ok(builder.block_params(join_b)[0])
                }
            }
            IrCallKind::Prompt { def_id, .. } => {
                lower_prompt_call(
                    builder,
                    module,
                    runtime,
                    *def_id,
                    callee_name,
                    args,
                    current_return_ty,
                    env,
                    scope_stack,
                    func_ids_by_def,
                    &expr.ty,
                    expr.span,
                )
            }
            IrCallKind::StructConstructor { def_id } => {
                let ir_type = runtime.ir_types.get(def_id).cloned().ok_or_else(|| {
                    CodegenError::cranelift(
                        format!("struct type `{callee_name}` metadata missing from ir_types"),
                        expr.span,
                    )
                })?;
                lower_struct_constructor(
                    builder,
                    module,
                    runtime,
                    &ir_type,
                    args,
                    current_return_ty,
                    env,
                    scope_stack,
                    func_ids_by_def,
                    expr.span,
                )
            }
            IrCallKind::Fixture { .. } => Err(CodegenError::cranelift(
                format!("test fixture `{callee_name}` is interpreter-only and cannot be lowered natively"),
                expr.span,
            )),
            IrCallKind::Unknown => Err(CodegenError::cranelift(
                format!("call to `{callee_name}` did not resolve ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â typecheck should have caught this"),
                expr.span,
            )),
        },
        IrExprKind::FieldAccess { target, field } => {
            let def_id = match &target.ty {
                Type::Struct(id) => *id,
                other => {
                    return Err(CodegenError::cranelift(
                        format!(
                            "field access target has non-struct type `{other:?}` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â typecheck should have caught this"
                        ),
                        expr.span,
                    ));
                }
            };
            let ir_type = runtime.ir_types.get(&def_id).cloned().ok_or_else(|| {
                CodegenError::cranelift(
                    format!("struct metadata missing for field access to `{field}`"),
                    expr.span,
                )
            })?;
            let (i, field_meta) = ir_type
                .fields
                .iter()
                .enumerate()
                .find(|(_, f)| &f.name == field)
                .ok_or_else(|| {
                    CodegenError::cranelift(
                        format!("field `{field}` not found on struct `{}`", ir_type.name),
                        expr.span,
                    )
                })?;
            let offset = (i as i32) * STRUCT_FIELD_SLOT_BYTES;
            let field_cl_ty = cl_type_for(&field_meta.ty, field_meta.span)?;

            // Borrow-at-use-site peephole: if the target is a bare
            // `IrExprKind::Local`, read the Variable directly with
            // no ownership-conversion retain, and skip the symmetric
            // post-extract release of the struct pointer. The load
            // of the field only reads the struct's memory; we never
            // mutate the struct's refcount. The Local's binding
            // stays Live ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â its scope-exit release handles cleanup.
            let (struct_ptr, struct_borrowed) = lower_container_maybe_borrowed(
                builder, target, current_return_ty, env, scope_stack, func_ids_by_def, module, runtime,
            )?;
            let field_val = builder.ins().load(
                field_cl_ty,
                cranelift_codegen::ir::MemFlags::trusted(),
                struct_ptr,
                offset,
            );
            // Retain refcounted field so caller gets an Owned ref.
            // This retain is NOT redundant pass traffic ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â it's the
            // ownership conversion from "field slot inside a parent"
            // to "standalone result value the caller owns." The .6d
            // pass cannot replace this because the extracted value
            // never gets an IR Local; it's an intermediate temp
            // known only to codegen. Pairs with the struct_ptr
            // release below when struct_ptr was a fresh temp.
            if is_refcounted_type(&field_meta.ty) {
                emit_retain(builder, module, runtime, field_val);
            }
            // Release the temp +1 on the struct pointer only if we
            // created one. `struct_borrowed = false` means the target
            // was a fresh temp (call result, constructor, nested field
            // access) not a bare Local ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â the .6d pass doesn't see
            // those, so codegen must drop unconditionally. Borrowed
            // reads have no +1 to release.
            if !struct_borrowed {
                emit_release(builder, module, runtime, struct_ptr);
            }
            Ok(field_val)
        }
        IrExprKind::Index { target, index } => {
            // Element type from the Index expression's annotated type
            // (the type checker attaches the element type).
            let elem_ty = expr.ty.clone();
            let elem_cl_ty = cl_type_for(&elem_ty, expr.span)?;
            let elem_refcounted = is_refcounted_type(&elem_ty);

            // Same borrow-at-use-site trick
            // as FieldAccess ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â if the list target is a bare Local,
            // borrow it (no retain) and skip the post-extract release.
            // The bounds-check + load only read the list's memory;
            // never mutate its refcount or escape the pointer.
            let (list_ptr, list_borrowed) = lower_container_maybe_borrowed(
                builder, target, current_return_ty, env, scope_stack, func_ids_by_def, module, runtime,
            )?;
            let idx_val = lower_expr(
                builder,
                index,
                current_return_ty,
                env,
                scope_stack,
                func_ids_by_def,
                module,
                runtime,
            )?;

            // Bounds check: trap if `idx_val >= length` or `idx_val < 0`.
            let length = builder.ins().load(
                I64,
                cranelift_codegen::ir::MemFlags::trusted(),
                list_ptr,
                0,
            );
            let in_range_hi =
                builder.ins().icmp(IntCC::SignedLessThan, idx_val, length);
            let zero = builder.ins().iconst(I64, 0);
            let in_range_lo =
                builder.ins().icmp(IntCC::SignedGreaterThanOrEqual, idx_val, zero);
            let in_range = builder.ins().band(in_range_hi, in_range_lo);
            let trap_block = builder.create_block();
            let cont_block = builder.create_block();
            builder
                .ins()
                .brif(in_range, cont_block, &[], trap_block, &[]);
            builder.switch_to_block(trap_block);
            builder.seal_block(trap_block);
            let callee_ref =
                module.declare_func_in_func(runtime.overflow, builder.func);
            builder.ins().call(callee_ref, &[]);
            builder
                .ins()
                .trap(cranelift_codegen::ir::TrapCode::INTEGER_OVERFLOW);
            builder.switch_to_block(cont_block);
            builder.seal_block(cont_block);

            // Element address = list_ptr + 8 + idx * 8.
            let offset = builder.ins().imul_imm(idx_val, 8);
            let base = builder.ins().iadd_imm(list_ptr, 8);
            let elem_addr = builder.ins().iadd(base, offset);
            let elem_val = builder.ins().load(
                elem_cl_ty,
                cranelift_codegen::ir::MemFlags::trusted(),
                elem_addr,
                0,
            );
            // Retain extracted element as ownership conversion from
            // list slot ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢ standalone caller-owned value. The .6d
            // pass can't replace this because the extracted value
            // never gets an IR Local. Pairs with list_ptr release.
            if elem_refcounted {
                emit_retain(builder, module, runtime, elem_val);
            }
            // Release the temp +1 on the list pointer only if we
            // actually took one. `list_borrowed = false` means the
            // target was a fresh temp, not a bare Local ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â the .6d
            // pass doesn't see internal temps, so codegen drops
            // unconditionally. Borrowed reads have no +1 to drop.
            if !list_borrowed {
                emit_release(builder, module, runtime, list_ptr);
            }
            Ok(elem_val)
        }
        IrExprKind::List { items } => {
            // Element type taken from the List's annotated type.
            let elem_ty = match &expr.ty {
                Type::List(elem) => (**elem).clone(),
                other => {
                    return Err(CodegenError::cranelift(
                        format!(
                            "list literal has non-list type `{other:?}` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â typecheck should have caught this"
                        ),
                        expr.span,
                    ));
                }
            };
            let _elem_refcounted = is_refcounted_type(&elem_ty);
            // Allocation size: 8 (length) + 8 * N (elements).
            let total_bytes = 8 + 8 * items.len() as i64;
            let size_val = builder.ins().iconst(I64, total_bytes);
            // Single typed allocator. The typeinfo block
            // (pre-emitted in lower_file) carries destroy_fn
            // (corvid_destroy_list for refcounted elements, NULL
            // otherwise) and trace_fn (corvid_trace_list always).
            let list_ti_id = runtime
                .list_typeinfos
                .get(&elem_ty)
                .copied()
                .ok_or_else(|| {
                    CodegenError::cranelift(
                        format!(
                            "no typeinfo pre-emitted for List<{}> ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â collect_list_element_types missed this site",
                            mangle_type_name(&elem_ty)
                        ),
                        expr.span,
                    )
                })?;
            let list_ti_gv = module.declare_data_in_func(list_ti_id, builder.func);
            let ti_addr = builder.ins().symbol_value(I64, list_ti_gv);
            let alloc_ref =
                module.declare_func_in_func(runtime.alloc_typed, builder.func);
            let call = builder.ins().call(alloc_ref, &[size_val, ti_addr]);
            let list_ptr = builder.inst_results(call)[0];
            // Store length at offset 0.
            let length_val = builder.ins().iconst(I64, items.len() as i64);
            builder.ins().store(
                cranelift_codegen::ir::MemFlags::trusted(),
                length_val,
                list_ptr,
                0,
            );
            // Store each element at offset 8 + i * 8. The element's
            // Owned +1 transfers into the list.
            for (i, item) in items.iter().enumerate() {
                let item_val = lower_expr(
                    builder,
                    item,
                    current_return_ty,
                    env,
                    scope_stack,
                    func_ids_by_def,
                    module,
                    runtime,
                )?;
                let offset = 8 + (i as i32) * 8;
                builder.ins().store(
                    cranelift_codegen::ir::MemFlags::trusted(),
                    item_val,
                    list_ptr,
                    offset,
                );
            }
            Ok(list_ptr)
        }
        IrExprKind::WeakNew { strong } => {
            let strong_val = lower_expr(
                builder,
                strong,
                current_return_ty,
                env,
                scope_stack,
                func_ids_by_def,
                module,
                runtime,
            )?;
            let weak_new_ref = module.declare_func_in_func(runtime.weak_new, builder.func);
            let call = builder.ins().call(weak_new_ref, &[strong_val]);
            let weak_ptr = builder.inst_results(call)[0];
            if !runtime.dup_drop_enabled && is_refcounted_type(&strong.ty) {
                emit_release(builder, module, runtime, strong_val);
            }
            Ok(weak_ptr)
        }
        IrExprKind::WeakUpgrade { weak } => {
            let weak_val = lower_expr(
                builder,
                weak,
                current_return_ty,
                env,
                scope_stack,
                func_ids_by_def,
                module,
                runtime,
            )?;
            let weak_upgrade_ref =
                module.declare_func_in_func(runtime.weak_upgrade, builder.func);
            let call = builder.ins().call(weak_upgrade_ref, &[weak_val]);
            let upgraded = builder.inst_results(call)[0];
            if !runtime.dup_drop_enabled && is_refcounted_type(&weak.ty) {
                emit_release(builder, module, runtime, weak_val);
            }
            Ok(upgraded)
        }
        IrExprKind::ResultOk { inner } if is_native_result_type(&expr.ty) => lower_result_constructor(
            builder,
            expr,
            inner,
            RESULT_TAG_OK,
            current_return_ty,
            env,
            scope_stack,
            func_ids_by_def,
            module,
            runtime,
        ),
        IrExprKind::ResultErr { inner } if is_native_result_type(&expr.ty) => lower_result_constructor(
            builder,
            expr,
            inner,
            RESULT_TAG_ERR,
            current_return_ty,
            env,
            scope_stack,
            func_ids_by_def,
            module,
            runtime,
        ),
        // Result/Option construction and `?` / `try-retry` control
        // flow are native only for the current principled subset:
        //   * nullable-pointer `Option<T>` when `T` is refcounted
        //   * wide scalar `Option<Int|Bool|Float>`
        //   * one-word `Result<T, E>`
        //   * `try ... retry` over the native `Result<T, E>` subset
        //
        // Any shape outside that subset still gets a clean,
        // actionable boundary here. The `cl_type_for` path above
        // will typically fire first for bindings, but these arms are
        // the load-bearing fallback for intermediate expressions and
        // other positions the typecheck doesn't intercept.
        IrExprKind::ResultOk { .. } | IrExprKind::ResultErr { .. } => {
            Err(CodegenError::not_supported(
                "`Result<T, E>` construction (`Ok(...)` / `Err(...)`) ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â native tagged-union lowering is not implemented yet; use the interpreter tier (`corvid run --tier interp`) until then",
                expr.span,
            ))
        }
        IrExprKind::OptionSome { inner } => {
            if option_uses_wrapper(&expr.ty) {
                let payload_ty = match &expr.ty {
                    Type::Option(inner_ty) => &**inner_ty,
                    _ => unreachable!("option wrapper gate requires Option<T> type"),
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
                emit_option_wrapper_value(builder, module, runtime, &expr.ty, payload, payload_ty, expr.span)
            } else if is_refcounted_type(&expr.ty) {
                lower_expr(
                    builder,
                    inner,
                    current_return_ty,
                    env,
                    scope_stack,
                    func_ids_by_def,
                    module,
                    runtime,
                )
            } else {
                Err(CodegenError::not_supported(
                    "`Some(...)` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â native codegen currently supports nullable-pointer `Option<T>` when `T` is refcounted plus wide scalar `Option<Int|Bool|Float>`",
                    expr.span,
                ))
            }
        }
        IrExprKind::OptionNone => {
            Ok(builder.ins().iconst(I64, 0))
        }
        IrExprKind::TryPropagate { inner } => match &inner.ty {
            Type::Result(_, _) => lower_try_propagate_result(
                builder,
                expr,
                inner,
                current_return_ty,
                env,
                scope_stack,
                func_ids_by_def,
                module,
                runtime,
            ),
            _ => lower_try_propagate_option(
                builder,
                expr,
                inner,
                current_return_ty,
                env,
                scope_stack,
                func_ids_by_def,
                module,
                runtime,
            ),
        },
        IrExprKind::TryRetry {
            body,
            attempts,
            backoff,
        } => match &body.ty {
            Type::Result(_, _) => lower_try_retry_result(
                builder,
                expr,
                body,
                *attempts,
                *backoff,
                current_return_ty,
                env,
                scope_stack,
                func_ids_by_def,
                module,
                runtime,
            ),
            Type::Option(_) => lower_try_retry_option(
                builder,
                expr,
                body,
                *attempts,
                *backoff,
                current_return_ty,
                env,
                scope_stack,
                func_ids_by_def,
                module,
                runtime,
            ),
            _ => Err(CodegenError::not_supported(
                "`try ... on error retry ...` in native code currently supports only native `Result<T, E>` and `Option<T>` bodies",
                expr.span,
            )),
        },
        IrExprKind::Replay { .. } => Err(CodegenError::not_supported(
            "`replay` expressions require runtime pattern-dispatch over a recorded trace; native tier lowering lands in a follow-up to 21-inv-E-runtime. Use the interpreter tier (`corvid run --tier interp`) until then.",
            expr.span,
        )),
        IrExprKind::StreamSplitBy { .. }
        | IrExprKind::StreamMerge { .. }
        | IrExprKind::StreamOrderedBy { .. }
        | IrExprKind::StreamResumeToken { .. }
        | IrExprKind::ResumeStream { .. }
        | IrExprKind::Ask { .. }
        | IrExprKind::Choose { .. } => {
            Err(CodegenError::not_supported(
                "human-boundary and stream combinators are interpreter-backed in this release; use the interpreter tier (`corvid run --tier interp`)",
                expr.span,
            ))
        }
    }
}

fn lower_try_propagate_option(
    builder: &mut FunctionBuilder,
    expr: &IrExpr,
    inner: &IrExpr,
    current_return_ty: &Type,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    scope_stack: &Vec<Vec<(LocalId, Variable)>>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<ClValue, CodegenError> {
    let (option_payload_ty, wrapped_option) = match &inner.ty {
        Type::Option(payload) if option_uses_wrapper(&inner.ty) => (&**payload, true),
        Type::Option(payload) if is_refcounted_type(payload) => (&**payload, false),
        Type::Option(_) => {
            return Err(CodegenError::not_supported(
                "postfix `?` on `Option<T>` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â native lowering currently supports nullable-pointer `Option<T>` with refcounted payloads plus wide scalar `Option<Int|Bool|Float>`",
                expr.span,
            ))
        }
        Type::Result(_, _) => {
            return Err(CodegenError::not_supported(
                "postfix `?` on `Result<T, E>` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â native tagged-union lowering is not implemented yet; use the interpreter tier until then",
                expr.span,
            ))
        }
        _ => {
            return Err(CodegenError::cranelift(
                format!(
                    "postfix `?` saw non-Option inner type `{}` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â typecheck should have caught this",
                    inner.ty.display_name()
                ),
                expr.span,
            ))
        }
    };

    match current_return_ty {
        Type::Option(ret_payload) if is_refcounted_type(ret_payload) => {}
        Type::Option(_) if is_native_wide_option_type(current_return_ty) => {}
        _ => {
            return Err(CodegenError::not_supported(
                "postfix `?` on `Option<T>` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â native lowering currently supports only functions returning native `Option<T>` shapes",
                expr.span,
            ))
        }
    }

    let option_val = lower_expr(
        builder,
        inner,
        current_return_ty,
        env,
        scope_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;
    let result_cl_ty = cl_type_for(option_payload_ty, expr.span)?;
    let some_block = builder.create_block();
    let none_block = builder.create_block();
    let merge_block = builder.create_block();
    let result = builder.append_block_param(merge_block, result_cl_ty);

    builder
        .ins()
        .brif(option_val, some_block, &[], none_block, &[]);

    builder.switch_to_block(none_block);
    builder.seal_block(none_block);
    let none_val = builder.ins().iconst(I64, 0);
    emit_function_return(builder, none_val, scope_stack, module, runtime);

    builder.switch_to_block(some_block);
    builder.seal_block(some_block);
    if wrapped_option {
        let payload = builder.ins().load(
            result_cl_ty,
            cranelift_codegen::ir::MemFlags::trusted(),
            option_val,
            OPTION_PAYLOAD_OFFSET,
        );
        if is_refcounted_type(option_payload_ty) {
            emit_retain(builder, module, runtime, payload);
        }
        emit_release(builder, module, runtime, option_val);
        builder.ins().jump(merge_block, &[payload.into()]);
    } else {
        builder.ins().jump(merge_block, &[option_val.into()]);
    }

    builder.switch_to_block(merge_block);
    builder.seal_block(merge_block);
    Ok(result)
}

fn lower_try_propagate_result(
    builder: &mut FunctionBuilder,
    expr: &IrExpr,
    inner: &IrExpr,
    current_return_ty: &Type,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    scope_stack: &Vec<Vec<(LocalId, Variable)>>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<ClValue, CodegenError> {
    let (ok_ty, err_ty) = match &inner.ty {
        Type::Result(ok, err) if is_native_result_type(&inner.ty) => (&**ok, &**err),
        Type::Result(_, _) => {
            return Err(CodegenError::not_supported(
                "postfix `?` on `Result<T, E>` ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â this concrete shape is outside the current native subset",
                expr.span,
            ))
        }
        _ => {
            return Err(CodegenError::cranelift(
                format!(
                    "postfix `?` saw non-Result inner type `{}` ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â typecheck should have caught this",
                    inner.ty.display_name()
                ),
                expr.span,
            ))
        }
    };

    if current_return_ty != &inner.ty {
        if let Type::Result(_, outer_err) = current_return_ty {
            if is_native_result_type(current_return_ty) && &**outer_err == err_ty {
                let result_ptr = lower_expr(
                    builder,
                    inner,
                    current_return_ty,
                    env,
                    scope_stack,
                    func_ids_by_def,
                    module,
                    runtime,
                )?;
                let tag = builder.ins().load(
                    I64,
                    cranelift_codegen::ir::MemFlags::trusted(),
                    result_ptr,
                    RESULT_TAG_OFFSET,
                );
                let ok_cl_ty = cl_type_for(ok_ty, expr.span)?;
                let ok_block = builder.create_block();
                let err_block = builder.create_block();
                let merge_block = builder.create_block();
                let unwrapped = builder.append_block_param(merge_block, ok_cl_ty);
                let is_ok = builder.ins().icmp_imm(IntCC::Equal, tag, RESULT_TAG_OK);
                builder.ins().brif(is_ok, ok_block, &[], err_block, &[]);

                builder.switch_to_block(err_block);
                builder.seal_block(err_block);
                let err_cl_ty = cl_type_for(err_ty, expr.span)?;
                let err_payload = builder.ins().load(
                    err_cl_ty,
                    cranelift_codegen::ir::MemFlags::trusted(),
                    result_ptr,
                    RESULT_PAYLOAD_OFFSET,
                );
                if is_refcounted_type(err_ty) {
                    emit_retain(builder, module, runtime, err_payload);
                }
                let outer_err_result = emit_result_wrapper_value(
                    builder,
                    module,
                    runtime,
                    current_return_ty,
                    err_payload,
                    err_ty,
                    RESULT_TAG_ERR,
                    expr.span,
                )?;
                emit_release(builder, module, runtime, result_ptr);
                emit_function_return(builder, outer_err_result, scope_stack, module, runtime);

                builder.switch_to_block(ok_block);
                builder.seal_block(ok_block);
                let payload = builder.ins().load(
                    ok_cl_ty,
                    cranelift_codegen::ir::MemFlags::trusted(),
                    result_ptr,
                    RESULT_PAYLOAD_OFFSET,
                );
                if is_refcounted_type(ok_ty) {
                    emit_retain(builder, module, runtime, payload);
                }
                emit_release(builder, module, runtime, result_ptr);
                builder.ins().jump(merge_block, &[payload.into()]);

                builder.switch_to_block(merge_block);
                builder.seal_block(merge_block);
                return Ok(unwrapped);
            }
        }
        return Err(CodegenError::not_supported(
            "postfix `?` on `Result<T, E>` ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â the current native subset supports propagation only when the enclosing function returns the same concrete `Result<T, E>` shape",
            expr.span,
        ));
    }

    let result_ptr = lower_expr(
        builder,
        inner,
        current_return_ty,
        env,
        scope_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;
    let tag = builder.ins().load(
        I64,
        cranelift_codegen::ir::MemFlags::trusted(),
        result_ptr,
        RESULT_TAG_OFFSET,
    );
    let ok_cl_ty = cl_type_for(ok_ty, expr.span)?;
    let ok_block = builder.create_block();
    let err_block = builder.create_block();
    let merge_block = builder.create_block();
    let unwrapped = builder.append_block_param(merge_block, ok_cl_ty);
    let is_ok = builder.ins().icmp_imm(IntCC::Equal, tag, RESULT_TAG_OK);
    builder.ins().brif(is_ok, ok_block, &[], err_block, &[]);

    builder.switch_to_block(err_block);
    builder.seal_block(err_block);
    emit_function_return(builder, result_ptr, scope_stack, module, runtime);

    builder.switch_to_block(ok_block);
    builder.seal_block(ok_block);
    let payload = builder.ins().load(
        ok_cl_ty,
        cranelift_codegen::ir::MemFlags::trusted(),
        result_ptr,
        RESULT_PAYLOAD_OFFSET,
    );
    if is_refcounted_type(ok_ty) {
        emit_retain(builder, module, runtime, payload);
    }
    emit_release(builder, module, runtime, result_ptr);
    builder.ins().jump(merge_block, &[payload.into()]);

    builder.switch_to_block(merge_block);
    builder.seal_block(merge_block);
    Ok(unwrapped)
}

fn emit_retry_delay_ms(
    builder: &mut FunctionBuilder,
    retry_index: ClValue,
    backoff: Backoff,
) -> ClValue {
    let cap = builder.ins().iconst(I64, i64::MAX);
    match backoff {
        Backoff::Linear(base_ms) => {
            if base_ms == 0 {
                return builder.ins().iconst(I64, 0);
            }
            let base = (base_ms.min(i64::MAX as u64)) as i64;
            let ordinal = builder.ins().iadd_imm(retry_index, 1);
            let max_multiplier = i64::MAX / base;
            let overflow =
                builder
                    .ins()
                    .icmp_imm(IntCC::SignedGreaterThan, ordinal, max_multiplier);
            let raw = builder.ins().imul_imm(ordinal, base);
            builder.ins().select(overflow, cap, raw)
        }
        Backoff::Exponential(base_ms) => {
            if base_ms == 0 {
                return builder.ins().iconst(I64, 0);
            }
            let base = (base_ms.min(i64::MAX as u64)) as i64;
            let max_shift = ((i64::MAX / base) as u64).ilog2() as i64;
            let overflow = builder
                .ins()
                .icmp_imm(IntCC::SignedGreaterThan, retry_index, max_shift);
            let base_val = builder.ins().iconst(I64, base);
            let raw = builder.ins().ishl(base_val, retry_index);
            builder.ins().select(overflow, cap, raw)
        }
    }
}

fn lower_try_retry_result(
    builder: &mut FunctionBuilder,
    expr: &IrExpr,
    body: &IrExpr,
    attempts: u64,
    backoff: Backoff,
    current_return_ty: &Type,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    scope_stack: &Vec<Vec<(LocalId, Variable)>>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<ClValue, CodegenError> {
    if !is_native_result_type(&body.ty) {
        return Err(CodegenError::not_supported(
            "`try ... on error retry ...` in native code currently supports only the native one-word `Result<T, E>` subset",
            expr.span,
        ));
    }
    if expr.ty != body.ty {
        return Err(CodegenError::cranelift(
            format!(
                "retry expression type `{}` did not match body type `{}` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â typecheck should have caught this",
                expr.ty.display_name(),
                body.ty.display_name()
            ),
            expr.span,
        ));
    }

    let total_attempts = attempts.max(1).min(i64::MAX as u64) as i64;
    let total_val = builder.ins().iconst(I64, total_attempts);
    let zero = builder.ins().iconst(I64, 0);

    let attempt_block = builder.create_block();
    let done_block = builder.create_block();
    let result_cl_ty = cl_type_for(&expr.ty, expr.span)?;
    let done_result = builder.append_block_param(done_block, result_cl_ty);
    let attempt_retry_index = builder.append_block_param(attempt_block, I64);
    let attempt_remaining = builder.append_block_param(attempt_block, I64);

    builder
        .ins()
        .jump(attempt_block, &[zero.into(), total_val.into()]);

    builder.switch_to_block(attempt_block);
    let retry_index_val = attempt_retry_index;
    let remaining_val = attempt_remaining;
    let result_ptr = lower_expr(
        builder,
        body,
        current_return_ty,
        env,
        scope_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;
    let tag = builder.ins().load(
        I64,
        cranelift_codegen::ir::MemFlags::trusted(),
        result_ptr,
        RESULT_TAG_OFFSET,
    );
    let is_ok = builder.ins().icmp_imm(IntCC::Equal, tag, RESULT_TAG_OK);
    let ok_block = builder.create_block();
    let err_block = builder.create_block();
    builder.ins().brif(is_ok, ok_block, &[], err_block, &[]);

    builder.switch_to_block(ok_block);
    builder.seal_block(ok_block);
    builder.ins().jump(done_block, &[result_ptr.into()]);

    builder.switch_to_block(err_block);
    builder.seal_block(err_block);
    let should_retry = builder
        .ins()
        .icmp_imm(IntCC::SignedGreaterThan, remaining_val, 1);
    let retry_block = builder.create_block();
    let finish_err_block = builder.create_block();
    builder.append_block_param(retry_block, result_cl_ty);
    builder.append_block_param(retry_block, I64);
    builder.append_block_param(retry_block, I64);
    builder.append_block_param(finish_err_block, result_cl_ty);
    builder.ins().brif(
        should_retry,
        retry_block,
        &[
            result_ptr.into(),
            retry_index_val.into(),
            remaining_val.into(),
        ],
        finish_err_block,
        &[result_ptr.into()],
    );

    builder.switch_to_block(finish_err_block);
    builder.seal_block(finish_err_block);
    let final_result = builder.block_params(finish_err_block)[0];
    builder.ins().jump(done_block, &[final_result.into()]);

    builder.switch_to_block(retry_block);
    builder.seal_block(retry_block);
    let retry_result = builder.block_params(retry_block)[0];
    let retry_index = builder.block_params(retry_block)[1];
    let retry_remaining = builder.block_params(retry_block)[2];
    emit_release(builder, module, runtime, retry_result);

    let delay_val = emit_retry_delay_ms(builder, retry_index, backoff);
    let zero_delay = builder.ins().iconst(I64, 0);
    let has_delay = builder
        .ins()
        .icmp(IntCC::SignedGreaterThan, delay_val, zero_delay);
    let sleep_block = builder.create_block();
    let no_sleep_block = builder.create_block();
    let continue_block = builder.create_block();
    builder.append_block_param(sleep_block, I64);
    builder.ins().brif(
        has_delay,
        sleep_block,
        &[delay_val.into()],
        no_sleep_block,
        &[],
    );

    builder.switch_to_block(sleep_block);
    builder.seal_block(sleep_block);
    let sleep_delay = builder.block_params(sleep_block)[0];
    let sleep_ref = module.declare_func_in_func(runtime.sleep_ms, builder.func);
    builder.ins().call(sleep_ref, &[sleep_delay]);
    builder.ins().jump(continue_block, &[]);

    builder.switch_to_block(no_sleep_block);
    builder.seal_block(no_sleep_block);
    builder.ins().jump(continue_block, &[]);

    builder.switch_to_block(continue_block);
    builder.seal_block(continue_block);

    let next_retry_index = builder.ins().iadd_imm(retry_index, 1);
    let next_remaining = builder.ins().iadd_imm(retry_remaining, -1);
    builder.ins().jump(
        attempt_block,
        &[next_retry_index.into(), next_remaining.into()],
    );
    builder.seal_block(attempt_block);

    builder.switch_to_block(done_block);
    builder.seal_block(done_block);
    Ok(done_result)
}

fn lower_try_retry_option(
    builder: &mut FunctionBuilder,
    expr: &IrExpr,
    body: &IrExpr,
    attempts: u64,
    backoff: Backoff,
    current_return_ty: &Type,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    scope_stack: &Vec<Vec<(LocalId, Variable)>>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<ClValue, CodegenError> {
    if !is_native_option_expr_type(&body.ty) {
        return Err(CodegenError::not_supported(
            "`try ... on error retry ...` in native code currently supports only native `Result<T, E>` and `Option<T>` bodies",
            expr.span,
        ));
    }
    if expr.ty != body.ty {
        return Err(CodegenError::cranelift(
            format!(
                "retry expression type `{}` did not match body type `{}` - typecheck should have caught this",
                expr.ty.display_name(),
                body.ty.display_name()
            ),
            expr.span,
        ));
    }

    let total_attempts = attempts.max(1).min(i64::MAX as u64) as i64;
    let total_val = builder.ins().iconst(I64, total_attempts);
    let zero = builder.ins().iconst(I64, 0);

    let attempt_block = builder.create_block();
    let done_block = builder.create_block();
    let result_cl_ty = cl_type_for(&expr.ty, expr.span)?;
    let done_result = builder.append_block_param(done_block, result_cl_ty);
    let attempt_retry_index = builder.append_block_param(attempt_block, I64);
    let attempt_remaining = builder.append_block_param(attempt_block, I64);

    builder
        .ins()
        .jump(attempt_block, &[zero.into(), total_val.into()]);

    builder.switch_to_block(attempt_block);
    let retry_index_val = attempt_retry_index;
    let remaining_val = attempt_remaining;
    let option_val = lower_expr(
        builder,
        body,
        current_return_ty,
        env,
        scope_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;

    let is_some = builder.ins().icmp_imm(IntCC::NotEqual, option_val, 0);
    let some_block = builder.create_block();
    let none_block = builder.create_block();
    builder
        .ins()
        .brif(is_some, some_block, &[], none_block, &[]);

    builder.switch_to_block(some_block);
    builder.seal_block(some_block);
    builder.ins().jump(done_block, &[option_val.into()]);

    builder.switch_to_block(none_block);
    builder.seal_block(none_block);
    let should_retry = builder
        .ins()
        .icmp_imm(IntCC::SignedGreaterThan, remaining_val, 1);
    let retry_block = builder.create_block();
    let finish_none_block = builder.create_block();
    builder.append_block_param(retry_block, result_cl_ty);
    builder.append_block_param(retry_block, I64);
    builder.append_block_param(retry_block, I64);
    builder.append_block_param(finish_none_block, result_cl_ty);
    builder.ins().brif(
        should_retry,
        retry_block,
        &[
            option_val.into(),
            retry_index_val.into(),
            remaining_val.into(),
        ],
        finish_none_block,
        &[option_val.into()],
    );

    builder.switch_to_block(finish_none_block);
    builder.seal_block(finish_none_block);
    let final_result = builder.block_params(finish_none_block)[0];
    builder.ins().jump(done_block, &[final_result.into()]);

    builder.switch_to_block(retry_block);
    builder.seal_block(retry_block);
    let _retry_result = builder.block_params(retry_block)[0];
    let retry_index = builder.block_params(retry_block)[1];
    let retry_remaining = builder.block_params(retry_block)[2];

    let delay_val = emit_retry_delay_ms(builder, retry_index, backoff);
    let zero_delay = builder.ins().iconst(I64, 0);
    let has_delay = builder
        .ins()
        .icmp(IntCC::SignedGreaterThan, delay_val, zero_delay);
    let sleep_block = builder.create_block();
    let no_sleep_block = builder.create_block();
    let continue_block = builder.create_block();
    builder.append_block_param(sleep_block, I64);
    builder.ins().brif(
        has_delay,
        sleep_block,
        &[delay_val.into()],
        no_sleep_block,
        &[],
    );

    builder.switch_to_block(sleep_block);
    builder.seal_block(sleep_block);
    let sleep_delay = builder.block_params(sleep_block)[0];
    let sleep_ref = module.declare_func_in_func(runtime.sleep_ms, builder.func);
    builder.ins().call(sleep_ref, &[sleep_delay]);
    builder.ins().jump(continue_block, &[]);

    builder.switch_to_block(no_sleep_block);
    builder.seal_block(no_sleep_block);
    builder.ins().jump(continue_block, &[]);

    builder.switch_to_block(continue_block);
    builder.seal_block(continue_block);

    let next_retry_index = builder.ins().iadd_imm(retry_index, 1);
    let next_remaining = builder.ins().iadd_imm(retry_remaining, -1);
    builder.ins().jump(
        attempt_block,
        &[next_retry_index.into(), next_remaining.into()],
    );
    builder.seal_block(attempt_block);

    builder.switch_to_block(done_block);
    builder.seal_block(done_block);
    Ok(done_result)
}

fn tool_wrapper_symbol(tool_name: &str) -> String {
    let mangled: String = tool_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("__corvid_tool_{mangled}")
}
