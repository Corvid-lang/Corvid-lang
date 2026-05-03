use super::*;

mod binop;
mod constructors;
mod operand;
mod overflow;
mod try_propagate;
mod try_retry;
mod wrappers;
pub(super) use binop::{
    lower_binop_strict, lower_binop_wrapping, lower_short_circuit, lower_unop, lower_unop_wrapping,
};
pub(super) use constructors::{
    emit_grounded_value_attestation, emit_option_wrapper_value, emit_result_wrapper_value,
    lower_result_constructor, lower_string_literal, lower_struct_constructor,
};
pub(super) use operand::{
    lower_container_maybe_borrowed, lower_string_binop_with_ownership,
    lower_string_operand_maybe_borrowed,
};
pub(super) use overflow::{trap_on_zero, with_overflow_trap};
pub(super) use try_propagate::{lower_try_propagate_option, lower_try_propagate_result};
pub(super) use try_retry::{lower_try_retry_option, lower_try_retry_result};
use wrappers::tool_wrapper_symbol;

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
