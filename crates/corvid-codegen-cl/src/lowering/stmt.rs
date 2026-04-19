use super::*;

#[derive(Clone, Copy)]
pub(super) enum BlockOutcome {
    Normal,
    Terminated,
}

pub(super) fn emit_function_return(
    builder: &mut FunctionBuilder,
    value: ClValue,
    scope_stack: &[Vec<(LocalId, Variable)>],
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) {
    if !runtime.dup_drop_enabled {
        for scope in scope_stack.iter().rev() {
            for (_, var) in scope.iter().rev() {
                let v_local = builder.use_var(*var);
                emit_release(builder, module, runtime, v_local);
            }
        }
    }
    builder.ins().return_(&[value]);
}

pub(super) fn lower_block(
    builder: &mut FunctionBuilder,
    block: &IrBlock,
    current_return_ty: &Type,
    env: &mut HashMap<LocalId, (Variable, clir::Type)>,
    var_idx: &mut usize,
    scope_stack: &mut Vec<Vec<(LocalId, Variable)>>,
    loop_stack: &mut Vec<LoopCtx>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<BlockOutcome, CodegenError> {
    for stmt in &block.stmts {
        match lower_stmt(
            builder,
            stmt,
            current_return_ty,
            env,
            var_idx,
            scope_stack,
            loop_stack,
            func_ids_by_def,
            module,
            runtime,
        )? {
            BlockOutcome::Terminated => return Ok(BlockOutcome::Terminated),
            BlockOutcome::Normal => {}
        }
    }
    Ok(BlockOutcome::Normal)
}

fn lower_stmt(
    builder: &mut FunctionBuilder,
    stmt: &IrStmt,
    current_return_ty: &Type,
    env: &mut HashMap<LocalId, (Variable, clir::Type)>,
    var_idx: &mut usize,
    scope_stack: &mut Vec<Vec<(LocalId, Variable)>>,
    loop_stack: &mut Vec<LoopCtx>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<BlockOutcome, CodegenError> {
    match stmt {
        IrStmt::Yield { span, .. } => {
            Err(CodegenError::not_supported(
                "Stream lowering not yet implemented",
                *span,
            ))
        }
        IrStmt::Return { value, span } => {
            let v = match value {
                Some(e) => lower_expr(
                    builder,
                    e,
                    current_return_ty,
                    env,
                    scope_stack,
                    func_ids_by_def,
                    module,
                    runtime,
                )?,
                None => {
                    return Err(CodegenError::not_supported(
                        "bare `return` (Nothing type not supported yet)",
                        *span,
                    ));
                }
            };
            // The return value is an Owned temp (per the three-state
            // ownership model Ã¢â‚¬â€ every `lower_expr` returns Owned for
            // refcounted types). The caller will receive the +1 we
            // hold; nothing more to do for the value itself.
            //
            // Release every refcounted local across all live scopes
            // before transferring control. Walk innermost-first to
            // mirror lexical scope exit order (matters only if the
            // `release` call has side effects we care about, which it
            // doesn't, but the ordering is conventional).
            emit_function_return(builder, v, scope_stack, module, runtime);
            Ok(BlockOutcome::Terminated)
        }
        IrStmt::Expr { expr, .. } => {
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
            // Discarded statement-expression:
            //   - If the expression is a BARE Local read, the value
            //     belongs to the Local binding; the .6d pass handles
            //     its lifetime via block-exit drops or last-use kills.
            //     Under flag-off the pre-.6d path retained at read
            //     time, so we release to match.
            //   - If the expression is anything else (call, composite,
            //     literal), it produced a fresh Owned temp with no
            //     owner Ã¢â‚¬â€ always release, regardless of flag, since
            //     the pass has no Local to drop.
            if is_refcounted_type(&expr.ty) {
                let is_bare_local = matches!(&expr.kind, IrExprKind::Local { .. });
                if !is_bare_local || !runtime.dup_drop_enabled {
                    emit_release(builder, module, runtime, v);
                }
            }
            Ok(BlockOutcome::Normal)
        }
        IrStmt::Let {
            local_id,
            ty,
            value,
            span,
            ..
        } => {
            let cl_ty = cl_type_for(ty, *span)?;
            let refcounted = is_refcounted_type(ty);
            // Declare-or-reuse: a fresh `LocalId` gets a new Cranelift
            // `Variable`; reassignment to a name already bound in this
            // function reuses the existing Variable. A type change on
            // reassignment is a typechecker bug Ã¢â‚¬â€ we surface it as a
            // clean `CodegenError` instead of letting Cranelift panic.
            let (var, is_reassignment) = match env.get(local_id) {
                Some(&(existing_var, existing_ty)) => {
                    if existing_ty != cl_ty {
                        return Err(CodegenError::cranelift(
                            format!(
                                "variable redeclared with different type: was {existing_ty}, now {cl_ty} Ã¢â‚¬â€ typechecker should have caught this"
                            ),
                            *span,
                        ));
                    }
                    (existing_var, true)
                }
                None => {
                    let new_var = Variable::from_u32(*var_idx as u32);
                    *var_idx += 1;
                    builder.declare_var(new_var, cl_ty);
                    env.insert(*local_id, (new_var, cl_ty));
                    (new_var, false)
                }
            };
            // For reassignment of a refcounted local: read the old
            // value first, release it, THEN bind the new value (which
            // came pre-Owned from `lower_expr`).
            // Reassignment: the old value of the Local is being
            // replaced; its +1 must drop. The .6d pass doesn't yet
            // model reassignment-kill (the analysis treats a rebind
            // as a def but doesn't schedule a Drop for the previous
            // value Ã¢â‚¬â€ this is a forward-compatibility gap tracked for
            // a future analysis extension). Always release the old
            // value at codegen time so the unified pass stays
            // refcount-correct on reassigned refcounted locals.
            if refcounted && is_reassignment {
                let old = builder.use_var(var);
                emit_release(builder, module, runtime, old);
            }
            let v = lower_expr(
                builder,
                value,
                current_return_ty,
                env,
                scope_stack,
                func_ids_by_def,
                module,
                runtime,
            )?;
            // The Value flowing into a refcounted
            // binding must be stack-map-declared so Cranelift spills
            // it across safepoints. Cranelift's safepoint pass
            // tracks liveness through SSA phis for Values that
            // travel between blocks via the Variable facade, but
            // only for Values originally declared here.
            if refcounted {
                builder.declare_value_needs_stack_map(v);
            }
            builder.def_var(var, v);
            // Track this binding in the current scope so it gets
            // released at scope exit. Only on first binding Ã¢â‚¬â€ a
            // reassignment is already tracked by the original Let.
            if refcounted && !is_reassignment {
                if let Some(top) = scope_stack.last_mut() {
                    top.push((*local_id, var));
                }
            }
            Ok(BlockOutcome::Normal)
        }
        IrStmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => lower_if(
            builder,
            cond,
            then_block,
            else_block.as_ref(),
            current_return_ty,
            env,
            var_idx,
            scope_stack,
            loop_stack,
            func_ids_by_def,
            module,
            runtime,
        ),
        IrStmt::For {
            var_local,
            iter,
            body,
            span,
            ..
        } => lower_for(
            builder,
            *var_local,
            iter,
            body,
            *span,
            current_return_ty,
            env,
            var_idx,
            scope_stack,
            loop_stack,
            func_ids_by_def,
            module,
            runtime,
        ),
        IrStmt::Approve { label, args, span } => {
            // Lower approve args once, keep their side effects, and
            // forward the typed values into the native runtime so the
            // trace matches interpreter ApprovalRequest args.
            let approve_arg_vals = args
                .iter()
                .map(|a| {
                    lower_expr(
                        builder,
                        a,
                        current_return_ty,
                        env,
                        scope_stack,
                        func_ids_by_def,
                        module,
                        runtime,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            let approve_arg_tys = args.iter().map(|a| a.ty.clone()).collect::<Vec<_>>();
            let trace_payload =
                emit_trace_payload(builder, module, runtime, &approve_arg_vals, &approve_arg_tys, *span)?;

            let label_val = emit_string_const(builder, module, runtime, label, *span)?;
            let approve_fref = module.declare_func_in_func(runtime.approve_sync, builder.func);
            let call = builder.ins().call(
                approve_fref,
                &[
                    label_val,
                    trace_payload.type_tags,
                    trace_payload.count,
                    trace_payload.values_ptr,
                ],
            );
            let results: Vec<ClValue> = builder.inst_results(call).iter().copied().collect();
            emit_release(builder, module, runtime, label_val);
            emit_release(builder, module, runtime, trace_payload.type_tags);
            if !runtime.dup_drop_enabled {
                for (v, arg) in approve_arg_vals.iter().zip(args.iter()) {
                    if is_refcounted_type(&arg.ty) {
                        emit_release(builder, module, runtime, *v);
                    }
                }
            }

            let denied = builder.ins().icmp_imm(IntCC::Equal, results[0], 0);
            let deny_block = builder.create_block();
            let continue_block = builder.create_block();
            builder
                .ins()
                .brif(denied, deny_block, &[], continue_block, &[]);

            builder.switch_to_block(deny_block);
            builder.seal_block(deny_block);
            builder
                .ins()
                .trap(cranelift_codegen::ir::TrapCode::unwrap_user(1));

            builder.switch_to_block(continue_block);
            builder.seal_block(continue_block);
            Ok(BlockOutcome::Normal)
        }
        IrStmt::Pass { .. } => Ok(BlockOutcome::Normal),
        IrStmt::Break { span } => lower_break_or_continue(
            builder,
            true,
            *span,
            scope_stack,
            loop_stack,
            module,
            runtime,
        ),
        IrStmt::Continue { span } => lower_break_or_continue(
            builder,
            false,
            *span,
            scope_stack,
            loop_stack,
            module,
            runtime,
        ),
        // Dup/Drop as first-class IR operations. In the fallback
        // path the ownership analysis pass hasn't been
        // written yet, so these variants never appear in the IR
        // codegen receives (only `None` borrow_sigs and no
        // Dup/Drop statements Ã¢â‚¬â€ all ownership is still handled by
        // the scattered `emit_retain`/`emit_release` calls in the
        // expression lowerings). 17b-1b replaces that with the
        // analysis pass that actually emits these. For forward
        // compatibility the handlers are present and correct today:
        // each lowers to a single runtime call on the variable's
        // current value, with a type-based no-op for non-refcounted
        // locals.
        IrStmt::Dup { local_id, span } => {
            let (var, cl_ty) = *env.get(local_id).ok_or_else(|| {
                CodegenError::cranelift(
                    format!("Dup references unknown local {:?}", local_id),
                    *span,
                )
            })?;
            // Non-refcounted locals use I64/F64/I8 for primitives;
            // only I64 values that point at refcounted payloads need
            // retain. The analysis pass is responsible for only
            // emitting Dup on refcounted locals Ã¢â‚¬â€ if a non-I64
            // slipped through, that's a bug in the analysis, not
            // a silent no-op here.
            if cl_ty != I64 {
                return Err(CodegenError::cranelift(
                    format!(
                        "Dup on non-I64 local (cl_ty={cl_ty:?}) Ã¢â‚¬â€ analysis \
                         should have filtered this out"
                    ),
                    *span,
                ));
            }
            let v = builder.use_var(var);
            emit_retain(builder, module, runtime, v);
            Ok(BlockOutcome::Normal)
        }
        IrStmt::Drop { local_id, span } => {
            let (var, cl_ty) = *env.get(local_id).ok_or_else(|| {
                CodegenError::cranelift(
                    format!("Drop references unknown local {:?}", local_id),
                    *span,
                )
            })?;
            if cl_ty != I64 {
                return Err(CodegenError::cranelift(
                    format!(
                        "Drop on non-I64 local (cl_ty={cl_ty:?}) Ã¢â‚¬â€ analysis \
                         should have filtered this out"
                    ),
                    *span,
                ));
            }
            let v = builder.use_var(var);
            emit_release(builder, module, runtime, v);
            Ok(BlockOutcome::Normal)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn lower_for(
    builder: &mut FunctionBuilder,
    var_local: LocalId,
    iter: &IrExpr,
    body: &IrBlock,
    span: Span,
    current_return_ty: &Type,
    env: &mut HashMap<LocalId, (Variable, clir::Type)>,
    var_idx: &mut usize,
    scope_stack: &mut Vec<Vec<(LocalId, Variable)>>,
    loop_stack: &mut Vec<LoopCtx>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<BlockOutcome, CodegenError> {
    // Element type comes from the iterator's list type.
    let elem_ty = match &iter.ty {
        Type::List(elem) => (**elem).clone(),
        Type::String => {
            return Err(CodegenError::not_supported(
                "`for c in string` iteration in native code Ã¢â‚¬â€ not implemented yet (needs iterator \
                 protocol or string-specific lowering)",
                span,
            ));
        }
        other => {
            return Err(CodegenError::cranelift(
                format!(
                    "`for` iterator has non-list type `{other:?}` Ã¢â‚¬â€ typecheck should have caught this"
                ),
                span,
            ));
        }
    };
    let elem_cl_ty = cl_type_for(&elem_ty, span)?;
    let elem_refcounted = is_refcounted_type(&elem_ty);

    // Iter-as-bare-Local borrow peephole. Same
    // correctness argument as 17b-1b.3's FieldAccess/Index Ã¢â‚¬â€ the
    // loop's use of `list_ptr` (length load + per-element load +
    // bounds arithmetic) only READS the list's memory; never
    // mutates the list's refcount or escapes the pointer. If iter
    // is a bare Local of the enclosing scope, we can borrow it:
    // skip the ownership-conversion retain at lower_expr time AND
    // skip the paired release at loop exit. The Local's binding
    // stays Live in its enclosing scope, governed by the usual
    // scope-exit release.
    let (list_ptr, list_borrowed) = lower_container_maybe_borrowed(
        builder, iter, current_return_ty, env, scope_stack, func_ids_by_def, module, runtime,
    )?;

    // Load length from offset 0 of the list payload.
    let length = builder.ins().load(
        I64,
        cranelift_codegen::ir::MemFlags::trusted(),
        list_ptr,
        0,
    );

    // Declare + init loop variable. Starts at 0 (null if refcounted)
    // so the release-on-rebind on the first iteration is a no-op.
    let loop_var = Variable::from_u32(*var_idx as u32);
    *var_idx += 1;
    builder.declare_var(loop_var, elem_cl_ty);
    let zero_elem = builder.ins().iconst(elem_cl_ty, 0);
    builder.def_var(loop_var, zero_elem);
    env.insert(var_local, (loop_var, elem_cl_ty));
    // Track the loop variable in the enclosing (not yet pushed) body
    // scope's sibling Ã¢â‚¬â€ the CURRENT scope, so it releases when the
    // enclosing block exits. That ensures the final iteration's value
    // gets released.
    if elem_refcounted {
        if let Some(top) = scope_stack.last_mut() {
            top.push((var_local, loop_var));
        }
    }

    // Declare + init index counter.
    let i_var = Variable::from_u32(*var_idx as u32);
    *var_idx += 1;
    builder.declare_var(i_var, I64);
    let zero_i = builder.ins().iconst(I64, 0);
    builder.def_var(i_var, zero_i);

    // Create the four blocks.
    let header_b = builder.create_block();
    let body_b = builder.create_block();
    let step_b = builder.create_block();
    let exit_b = builder.create_block();

    // Record the loop context so break/continue can find their targets.
    let scope_depth_at_entry = scope_stack.len();
    loop_stack.push(LoopCtx {
        step_block: step_b,
        exit_block: exit_b,
        scope_depth_at_entry,
    });

    builder.ins().jump(header_b, &[]);

    // --- header ---
    builder.switch_to_block(header_b);
    let i_now = builder.use_var(i_var);
    let keep_going = builder.ins().icmp(IntCC::SignedLessThan, i_now, length);
    builder.ins().brif(keep_going, body_b, &[], exit_b, &[]);

    // --- body ---
    builder.switch_to_block(body_b);
    builder.seal_block(body_b);
    // Compute element address: list_ptr + 8 + i * 8.
    let offset = builder.ins().imul_imm(i_now, 8);
    let base = builder.ins().iadd_imm(list_ptr, 8);
    let elem_addr = builder.ins().iadd(base, offset);
    let elem_val = builder.ins().load(
        elem_cl_ty,
        cranelift_codegen::ir::MemFlags::trusted(),
        elem_addr,
        0,
    );
    // The loaded element is a refcounted pointer that
    // flows into the loop variable. Declare it so Cranelift spills
    // before any safepoint in the loop body.
    if elem_refcounted {
        builder.declare_value_needs_stack_map(elem_val);
    }
    // Rebind loop-var: release old value (null-safe), retain new if
    // refcounted, def_var. Under the .6d pass, the pass inserts a
    // Drop on the loop variable at each iteration's natural last-use
    // point and does not emit a fresh Dup for the new iteration
    // element (the load above produces it with no +1 attached; the
    // consumer patterns inside the body supply Dups as needed).
    if !runtime.dup_drop_enabled && elem_refcounted {
        let old = builder.use_var(loop_var);
        emit_release(builder, module, runtime, old);
        emit_retain(builder, module, runtime, elem_val);
    }
    builder.def_var(loop_var, elem_val);

    // Push body scope so body-local Lets get released at end of each
    // iteration (or on break/continue/return from inside body).
    scope_stack.push(Vec::new());
    let body_outcome = lower_block(
        builder,
        body,
        current_return_ty,
        env,
        var_idx,
        scope_stack,
        loop_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;
    match body_outcome {
        BlockOutcome::Normal => {
            // Release body-scope locals before jumping to step.
            let body_scope = scope_stack.pop().unwrap_or_default();
            if !runtime.dup_drop_enabled {
                for (_, v) in body_scope.iter().rev() {
                    let x = builder.use_var(*v);
                    emit_release(builder, module, runtime, x);
                }
            }
            builder.ins().jump(step_b, &[]);
        }
        BlockOutcome::Terminated => {
            // Body returned Ã¢â‚¬â€ the return already emitted releases for
            // all live scopes. Just pop the body scope.
            scope_stack.pop();
        }
    }

    // --- step ---
    builder.switch_to_block(step_b);
    builder.seal_block(step_b);
    let i_next = builder.ins().iadd_imm(i_now, 1);
    builder.def_var(i_var, i_next);
    builder.ins().jump(header_b, &[]);

    // Now both predecessors of header_b (entry + step) have been
    // emitted, so we can seal it.
    builder.seal_block(header_b);

    // --- exit ---
    builder.switch_to_block(exit_b);
    builder.seal_block(exit_b);
    loop_stack.pop();

    // Release the list pointer we retained at the top Ã¢â‚¬â€ only if we
    // actually produced a +1 (non-borrowed path). When iter was a
    // bare Local we borrowed it and there's nothing to release.
    // `list_borrowed = false` means the iter was a fresh-owned temp
    // (list literal or call result), not a bare Local. The .6d pass
    // only schedules Drops for Locals in the IR; internal expression
    // temps are invisible to it, so the codegen must still drop the
    // list's +1 at loop end. `list_borrowed = true` means it was a
    // bare Local and the pass handles its lifetime.
    if is_refcounted_type(&iter.ty) && !list_borrowed {
        emit_release(builder, module, runtime, list_ptr);
    }
    Ok(BlockOutcome::Normal)
}

/// Release refcounted locals deeper than `floor_depth`, then jump to
/// the given block. Shared by `break` and `continue`.
fn lower_break_or_continue(
    builder: &mut FunctionBuilder,
    is_break: bool,
    span: Span,
    scope_stack: &mut Vec<Vec<(LocalId, Variable)>>,
    loop_stack: &mut Vec<LoopCtx>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<BlockOutcome, CodegenError> {
    let ctx = loop_stack.last().ok_or_else(|| {
        CodegenError::cranelift(
            format!(
                "`{}` outside of a loop Ã¢â‚¬â€ typecheck or parser should have caught this",
                if is_break { "break" } else { "continue" }
            ),
            span,
        )
    })?;
    // Walk scopes deeper than `scope_depth_at_entry`, releasing
    // refcounted locals. Don't pop Ã¢â‚¬â€ the lower_block that created
    // those scopes is still on the stack above us.
    if !runtime.dup_drop_enabled {
        for depth in (ctx.scope_depth_at_entry..scope_stack.len()).rev() {
            let scope = &scope_stack[depth];
            for (_, v) in scope.iter().rev() {
                let x = builder.use_var(*v);
                emit_release(builder, module, runtime, x);
            }
        }
    }
    let target = if is_break {
        ctx.exit_block
    } else {
        ctx.step_block
    };
    builder.ins().jump(target, &[]);
    Ok(BlockOutcome::Terminated)
}

/// bodies; both, if they fall through, `jump` to a merge block. If
/// neither falls through, merge is terminated with a trap (dead code)
/// and the enclosing `lower_block` is told the statement terminated.
fn lower_if(
    builder: &mut FunctionBuilder,
    cond: &IrExpr,
    then_block_ir: &IrBlock,
    else_block_ir: Option<&IrBlock>,
    current_return_ty: &Type,
    env: &mut HashMap<LocalId, (Variable, clir::Type)>,
    var_idx: &mut usize,
    scope_stack: &mut Vec<Vec<(LocalId, Variable)>>,
    loop_stack: &mut Vec<LoopCtx>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<BlockOutcome, CodegenError> {
    let cond_val = lower_expr(
        builder,
        cond,
        current_return_ty,
        env,
        scope_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;

    let then_b = builder.create_block();
    let else_b = if else_block_ir.is_some() {
        Some(builder.create_block())
    } else {
        None
    };
    let merge_b = builder.create_block();
    let false_target = else_b.unwrap_or(merge_b);
    builder.ins().brif(cond_val, then_b, &[], false_target, &[]);

    // `any_fell_through` starts true when there's no else because the
    // cond-false path flows straight to merge via the brif above.
    let mut any_fell_through = else_b.is_none();

    // Then branch Ã¢â‚¬â€ push a new scope for branch-local refcounted Lets;
    // pop after lowering, releasing each local's refcount if the
    // branch fell through normally.
    builder.switch_to_block(then_b);
    builder.seal_block(then_b);
    scope_stack.push(Vec::new());
    let then_outcome = lower_block(
        builder,
        then_block_ir,
        current_return_ty,
        env,
        var_idx,
        scope_stack,
        loop_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;
    if matches!(then_outcome, BlockOutcome::Normal) {
        // Release branch-scope refcounted locals before jumping to merge.
        // Under the .6d pass, the analysis handles branch-scope drops
        // via block-exit drops on locals with different last-use
        // points across branches Ã¢â‚¬â€ skip the scattered emission.
        let scope = scope_stack.pop().unwrap_or_default();
        if !runtime.dup_drop_enabled {
            for (_, var) in scope.iter().rev() {
                let v = builder.use_var(*var);
                emit_release(builder, module, runtime, v);
            }
        }
        builder.ins().jump(merge_b, &[]);
        any_fell_through = true;
    } else {
        // Branch terminated (return) Ã¢â‚¬â€ its return path already emitted
        // releases for all live locals across all scopes. Just pop.
        scope_stack.pop();
    }

    // Else branch (if present).
    if let (Some(else_b), Some(else_body)) = (else_b, else_block_ir) {
        builder.switch_to_block(else_b);
        builder.seal_block(else_b);
        scope_stack.push(Vec::new());
        let else_outcome = lower_block(
            builder,
            else_body,
            current_return_ty,
            env,
            var_idx,
            scope_stack,
            loop_stack,
            func_ids_by_def,
            module,
            runtime,
        )?;
        if matches!(else_outcome, BlockOutcome::Normal) {
            let scope = scope_stack.pop().unwrap_or_default();
            if !runtime.dup_drop_enabled {
                for (_, var) in scope.iter().rev() {
                    let v = builder.use_var(*var);
                    emit_release(builder, module, runtime, v);
                }
            }
            builder.ins().jump(merge_b, &[]);
            any_fell_through = true;
        } else {
            scope_stack.pop();
        }
    }

    builder.switch_to_block(merge_b);
    builder.seal_block(merge_b);
    if !any_fell_through {
        // Nothing flows here Ã¢â‚¬â€ both branches returned. Terminate the
        // unreachable merge with a trap so Cranelift's verifier is
        // satisfied, and tell the enclosing block the statement
        // terminated.
        builder
            .ins()
            .trap(cranelift_codegen::ir::TrapCode::INTEGER_OVERFLOW);
        return Ok(BlockOutcome::Terminated);
    }
    Ok(BlockOutcome::Normal)
}

