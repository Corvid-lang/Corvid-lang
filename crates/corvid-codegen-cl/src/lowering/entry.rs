use super::*;

pub(super) fn ir_uses_runtime(ir: &IrFile) -> bool {
    ir.agents.iter().any(|a| block_uses_runtime(&a.body))
}

fn block_uses_runtime(block: &IrBlock) -> bool {
    block.stmts.iter().any(stmt_uses_runtime)
}

fn stmt_uses_runtime(stmt: &IrStmt) -> bool {
    match stmt {
        IrStmt::Let { value, .. } => expr_uses_runtime(value),
        IrStmt::Yield { value, .. } => expr_uses_runtime(value),
        IrStmt::Return { value: Some(e), .. } => expr_uses_runtime(e),
        IrStmt::Return { value: None, .. } => false,
        IrStmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            expr_uses_runtime(cond)
                || block_uses_runtime(then_block)
                || else_block
                    .as_ref()
                    .map(block_uses_runtime)
                    .unwrap_or(false)
        }
        IrStmt::For { iter, body, .. } => expr_uses_runtime(iter) || block_uses_runtime(body),
        IrStmt::Approve { .. } => true,
        IrStmt::Expr { expr, .. } => expr_uses_runtime(expr),
        IrStmt::Break { .. } | IrStmt::Continue { .. } | IrStmt::Pass { .. } => false,
        // Dup/Drop emit direct runtime calls (corvid_retain/release)
        // but those don't need the async bridge ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â runtime is a C ABI
        // library linked in regardless.
        IrStmt::Dup { .. } | IrStmt::Drop { .. } => false,
    }
}

fn expr_uses_runtime(expr: &IrExpr) -> bool {
    match &expr.kind {
        IrExprKind::Literal(_) | IrExprKind::Local { .. } | IrExprKind::Decl { .. } => false,
        IrExprKind::Call { kind, args, .. } => {
            let self_needs = matches!(kind, IrCallKind::Tool { .. } | IrCallKind::Prompt { .. });
            self_needs || args.iter().any(expr_uses_runtime)
        }
        IrExprKind::FieldAccess { target, .. } => expr_uses_runtime(target),
        IrExprKind::UnwrapGrounded { value } => expr_uses_runtime(value),
        IrExprKind::Index { target, index } => {
            expr_uses_runtime(target) || expr_uses_runtime(index)
        }
        IrExprKind::BinOp { left, right, .. }
        | IrExprKind::WrappingBinOp { left, right, .. } => {
            expr_uses_runtime(left) || expr_uses_runtime(right)
        }
        IrExprKind::UnOp { operand, .. }
        | IrExprKind::WrappingUnOp { operand, .. } => expr_uses_runtime(operand),
        IrExprKind::List { items } => items.iter().any(expr_uses_runtime),
        // Result/Option IR variants recurse into sub-expressions. The
        // wrappers themselves don't use the async runtime, but a
        // `?`-propagated tool call inside `inner` still does.
        IrExprKind::WeakNew { strong: inner }
        | IrExprKind::WeakUpgrade { weak: inner }
        | IrExprKind::StreamSplitBy { stream: inner, .. }
        | IrExprKind::StreamMerge { groups: inner, .. }
        | IrExprKind::StreamOrderedBy { stream: inner, .. }
        | IrExprKind::StreamResumeToken { stream: inner }
        | IrExprKind::ResumeStream { token: inner, .. }
        | IrExprKind::ResultOk { inner }
        | IrExprKind::ResultErr { inner }
        | IrExprKind::OptionSome { inner }
        | IrExprKind::TryPropagate { inner } => expr_uses_runtime(inner),
        IrExprKind::OptionNone => false,
        IrExprKind::TryRetry { body, .. } => expr_uses_runtime(body),
        IrExprKind::Replay { trace, arms, else_body } => {
            // Replay dispatch itself reads a trace via the runtime,
            // so ANY replay expression needs the async bridge even
            // if every arm body is pure. Short-circuit to true.
            let _ = (trace, arms, else_body);
            true
        }
    }
}

/// Emit a signature-aware `int main(int argc, char** argv)` that:
///
///   1. Calls `corvid_init()` to register the leak-counter atexit.
///   2. Verifies `argc - 1 == n_params`, calling `corvid_arity_mismatch`
///      and aborting if not.
///   3. For each entry parameter, decodes `argv[i+1]` via the
///      appropriate parse helper (or `corvid_string_from_cstr` for
///      String parameters).
///   4. Calls the entry agent.
///   5. Prints the result via the matching helper.
///   6. Returns 0.
///
/// Replaces the original `corvid_entry` trampoline. Now that the
/// codegen knows the entry signature at emit time, generating `main`
/// directly avoids the C-shim-with-introspection trap.
pub(super) fn emit_entry_main(
    module: &mut ObjectModule,
    entry_agent: &IrAgent,
    entry_func_id: FuncId,
    runtime: &RuntimeFuncs,
    // Emit `corvid_runtime_init()` + `atexit(corvid_runtime_shutdown)`
    // only if the program actually needs the async runtime. Passing `false`
    // keeps compiled binaries as small + fast-starting as they were in
    // startup benchmark baseline.
    uses_runtime: bool,
) -> Result<(), CodegenError> {
    // I32 is imported at file scope since runtime setup needs it in
    // `declare_runtime_funcs` too.

    // Validate that every entry parameter and the return type are
    // representable at the command-line / stdout boundary. Struct and
    // List are deliberately excluded ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â they need a serialization implementation.
    for p in &entry_agent.params {
        check_entry_boundary_type(&p.ty, p.span, "parameter")?;
    }
    check_entry_boundary_type(&entry_agent.return_ty, entry_agent.span, "return")?;

    // main signature: (i32 argc, i64 argv) -> i32
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(I32));
    sig.params.push(AbiParam::new(I64));
    sig.returns.push(AbiParam::new(I32));
    let main_id = module
        .declare_function("main", Linkage::Export, &sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare main: {e}"), entry_agent.span)
        })?;

    let mut ctx = Context::new();
    ctx.func = Function::with_name_signature(
        UserFuncName::user(0, main_id.as_u32()),
        module.declarations().get_function_decl(main_id).signature.clone(),
    );

    let mut bctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut bctx);
        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        builder.switch_to_block(entry_block);
        builder.seal_block(entry_block);

        let argc_i32 = builder.block_params(entry_block)[0];
        let argv = builder.block_params(entry_block)[1];

        // 1. corvid_init() ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â registers atexit handler for leak counters.
        let init_ref = module.declare_func_in_func(runtime.entry_init, builder.func);
        builder.ins().call(init_ref, &[]);

        // If the program uses the async runtime, build
        // the tokio + corvid runtime globals NOW, eagerly. Shutdown is
        // registered via `atexit` so worker threads join cleanly at
        // exit. Shutdown runs BEFORE the leak-counter atexit (atexit
        // is LIFO), so any refcount activity from the runtime settles
        // before the counter prints ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â that's the intended ordering.
        if uses_runtime {
            let rt_init_ref =
                module.declare_func_in_func(runtime.runtime_init, builder.func);
            builder.ins().call(rt_init_ref, &[]);

            // Register corvid_runtime_shutdown via libc atexit. atexit
            // itself isn't in RuntimeFuncs because nothing else needs
            // it; declare it inline. Signature: int atexit(void (*)(void)).
            let mut atexit_sig = module.make_signature();
            atexit_sig.params.push(AbiParam::new(I64));
            atexit_sig.returns.push(AbiParam::new(I32));
            let atexit_id = module
                .declare_function("atexit", Linkage::Import, &atexit_sig)
                .map_err(|e| {
                    CodegenError::cranelift(
                        format!("declare atexit: {e}"),
                        entry_agent.span,
                    )
                })?;
            let atexit_ref = module.declare_func_in_func(atexit_id, builder.func);
            let shutdown_ref =
                module.declare_func_in_func(runtime.runtime_shutdown, builder.func);
            let shutdown_addr = builder.ins().func_addr(I64, shutdown_ref);
            builder.ins().call(atexit_ref, &[shutdown_addr]);
        }

        // 2. Arity check.
        let n_params = entry_agent.params.len() as i64;
        let argc_i64 = builder.ins().sextend(I64, argc_i32);
        let expected = builder.ins().iconst(I64, n_params + 1);
        let matches = builder.ins().icmp(IntCC::Equal, argc_i64, expected);
        let proceed_b = builder.create_block();
        let mismatch_b = builder.create_block();
        builder.ins().brif(matches, proceed_b, &[], mismatch_b, &[]);

        // mismatch path: call the helper, trap (it never returns).
        builder.switch_to_block(mismatch_b);
        builder.seal_block(mismatch_b);
        let arity_ref =
            module.declare_func_in_func(runtime.entry_arity_mismatch, builder.func);
        let expected_const = builder.ins().iconst(I64, n_params);
        let got = builder.ins().iadd_imm(argc_i64, -1);
        builder.ins().call(arity_ref, &[expected_const, got]);
        builder
            .ins()
            .trap(cranelift_codegen::ir::TrapCode::INTEGER_OVERFLOW);

        // 3. Decode each parameter from argv[i+1].
        builder.switch_to_block(proceed_b);
        builder.seal_block(proceed_b);
        let mut decoded_args: Vec<ClValue> = Vec::with_capacity(entry_agent.params.len());
        let mut decoded_refcounted: Vec<bool> = Vec::with_capacity(entry_agent.params.len());
        for (i, p) in entry_agent.params.iter().enumerate() {
            // Load argv[i+1] (a C string pointer).
            let cstr = builder.ins().load(
                I64,
                cranelift_codegen::ir::MemFlags::trusted(),
                argv,
                ((i as i32) + 1) * 8,
            );
            let argv_index = builder.ins().iconst(I64, (i as i64) + 1);
            let value = match &p.ty {
                Type::Int => {
                    let r =
                        module.declare_func_in_func(runtime.parse_i64, builder.func);
                    let c = builder.ins().call(r, &[cstr, argv_index]);
                    builder.inst_results(c)[0]
                }
                Type::Float => {
                    let r =
                        module.declare_func_in_func(runtime.parse_f64, builder.func);
                    let c = builder.ins().call(r, &[cstr, argv_index]);
                    builder.inst_results(c)[0]
                }
                Type::Bool => {
                    let r =
                        module.declare_func_in_func(runtime.parse_bool, builder.func);
                    let c = builder.ins().call(r, &[cstr, argv_index]);
                    builder.inst_results(c)[0]
                }
                Type::String => {
                    let r = module
                        .declare_func_in_func(runtime.string_from_cstr, builder.func);
                    let c = builder.ins().call(r, &[cstr]);
                    builder.inst_results(c)[0]
                }
                _ => {
                    // Already rejected by check_entry_boundary_type above;
                    // unreachable in practice.
                    return Err(CodegenError::cranelift(
                        format!(
                            "entry parameter `{}` of type `{}` reached lowering despite the boundary check",
                            p.name,
                            p.ty.display_name()
                        ),
                        p.span,
                    ));
                }
            };
            decoded_args.push(value);
            decoded_refcounted.push(is_refcounted_type(&p.ty));
        }

        let entry_ref = module.declare_func_in_func(entry_func_id, builder.func);
        let bench_enabled_ref =
            module.declare_func_in_func(runtime.bench_server_enabled, builder.func);
        let bench_next_ref =
            module.declare_func_in_func(runtime.bench_next_trial, builder.func);
        let bench_finish_ref =
            module.declare_func_in_func(runtime.bench_finish_trial, builder.func);

        let bench_mode = builder.ins().call(bench_enabled_ref, &[]);
        let bench_enabled = builder.inst_results(bench_mode)[0];
        let bench_check_b = builder.create_block();
        let bench_body_b = builder.create_block();
        builder.append_block_param(bench_body_b, I64);
        let bench_done_b = builder.create_block();
        let normal_call_b = builder.create_block();
        let bench_cmp = builder.ins().icmp_imm(IntCC::NotEqual, bench_enabled, 0);
        builder
            .ins()
            .brif(bench_cmp, bench_check_b, &[], normal_call_b, &[]);

        builder.switch_to_block(bench_check_b);
        let next_trial_call = builder.ins().call(bench_next_ref, &[]);
        let trial_idx = builder.inst_results(next_trial_call)[0];
        let has_trial = builder.ins().icmp_imm(IntCC::NotEqual, trial_idx, 0);
        builder
            .ins()
            .brif(has_trial, bench_body_b, &[trial_idx], bench_done_b, &[]);

        builder.switch_to_block(bench_body_b);
        let trial_idx = builder.block_params(bench_body_b)[0];
        let entry_call = builder.ins().call(entry_ref, &decoded_args);
        builder.ins().call(bench_finish_ref, &[trial_idx]);
        if let Some(result_val) = builder.inst_results(entry_call).first().copied() {
            emit_entry_result_print(
                &mut builder,
                module,
                runtime,
                &entry_agent.return_ty,
                result_val,
            );
        }
        builder.ins().jump(bench_check_b, &[]);
        builder.seal_block(bench_body_b);
        builder.seal_block(bench_check_b);

        builder.switch_to_block(normal_call_b);
        builder.seal_block(normal_call_b);
        if uses_runtime {
            let entry_name_val =
                emit_string_const(&mut builder, module, runtime, &entry_agent.name, entry_agent.span)?;
            let entry_arg_tys = entry_agent
                .params
                .iter()
                .map(|param| param.ty.clone())
                .collect::<Vec<_>>();
            let trace_payload = emit_trace_payload(
                &mut builder,
                module,
                runtime,
                &decoded_args,
                &entry_arg_tys,
                entry_agent.span,
            )?;
            let trace_run_started_ref =
                module.declare_func_in_func(runtime.trace_run_started, builder.func);
            builder.ins().call(
                trace_run_started_ref,
                &[
                    entry_name_val,
                    trace_payload.type_tags,
                    trace_payload.count,
                    trace_payload.values_ptr,
                ],
            );
            emit_release(&mut builder, module, runtime, trace_payload.type_tags);
            emit_release(&mut builder, module, runtime, entry_name_val);
        }
        let entry_call = builder.ins().call(entry_ref, &decoded_args);
        let result = builder.inst_results(entry_call).first().copied();

        if !runtime.dup_drop_enabled {
            for (v, is_ref) in decoded_args.iter().zip(decoded_refcounted.iter()) {
                if *is_ref {
                    emit_release(&mut builder, module, runtime, *v);
                }
            }
        }
        if let Some(result_val) = result {
            if uses_runtime {
                let trace_result_ref = match &entry_agent.return_ty {
                    Type::Int => Some(runtime.trace_run_completed_int),
                    Type::Bool => Some(runtime.trace_run_completed_bool),
                    Type::Float => Some(runtime.trace_run_completed_float),
                    Type::String => Some(runtime.trace_run_completed_string),
                    Type::Grounded(inner) => match &**inner {
                        Type::Int => Some(runtime.trace_run_completed_int),
                        Type::Bool => Some(runtime.trace_run_completed_bool),
                        Type::Float => Some(runtime.trace_run_completed_float),
                        Type::String => Some(runtime.trace_run_completed_string),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(trace_result) = trace_result_ref {
                    let trace_run_completed_ref =
                        module.declare_func_in_func(trace_result, builder.func);
                    builder.ins().call(trace_run_completed_ref, &[result_val]);
                }
            }
            emit_entry_result_print(
                &mut builder,
                module,
                runtime,
                &entry_agent.return_ty,
                result_val,
            );
        }
        let zero = builder.ins().iconst(I32, 0);
        builder.ins().return_(&[zero]);

        builder.switch_to_block(bench_done_b);
        builder.seal_block(bench_done_b);
        let zero = builder.ins().iconst(I32, 0);
        builder.ins().return_(&[zero]);
        builder.finalize();
    }

    define_function_with_stack_maps(
        module,
        main_id,
        &mut ctx,
        runtime,
        entry_agent.span,
        "main",
    )?;
    Ok(())
}

/// Validate that a type is one of the four supported at the
/// command-line / stdout boundary. Struct and List need a
/// dedicated serialization layer; Nothing isn't a sensible
/// CLI value either.
fn check_entry_boundary_type(
    ty: &Type,
    span: Span,
    role: &str,
) -> Result<(), CodegenError> {
    match ty {
        Type::Int | Type::Bool | Type::Float | Type::String => Ok(()),
        Type::Struct(_) | Type::ImportedStruct(_) | Type::List(_) | Type::Nothing
        | Type::Result(_, _) | Type::Option(_) | Type::Weak(_, _) | Type::Stream(_)
        | Type::Partial(_) | Type::ResumeToken(_) => {
            Err(CodegenError::not_supported(
                format!(
                    "entry agent {role} of type `{}` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â the native command-line boundary currently supports only `Int` / `Bool` / `Float` / `String`; structured types (including Result, Option, and Weak) need a dedicated serialization layer (use a wrapper agent that converts internally)",
                    ty.display_name()
                ),
                span,
            ))
        }
        Type::Grounded(inner) => check_entry_boundary_type(inner, span, role),
        Type::TraceId => Err(CodegenError::not_supported(
            format!(
                "entry agent {role} of type `TraceId` - `replay` expressions at the native command-line boundary land in Phase 21 slice 21-inv-E-4; use the interpreter tier for now"
            ),
            span,
        )),
        Type::Function { .. } | Type::Unknown => Err(CodegenError::cranelift(
            format!("entry agent {role} has un-printable type `{}`", ty.display_name()),
            span,
        )),
    }
}

fn emit_entry_result_print(
    builder: &mut FunctionBuilder<'_>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    return_ty: &Type,
    result_val: ClValue,
) {
    match return_ty {
        Type::Int => {
            let r = module.declare_func_in_func(runtime.print_i64, builder.func);
            builder.ins().call(r, &[result_val]);
        }
        Type::Bool => {
            let widened = builder.ins().uextend(I64, result_val);
            let r = module.declare_func_in_func(runtime.print_bool, builder.func);
            builder.ins().call(r, &[widened]);
        }
        Type::Float => {
            let r = module.declare_func_in_func(runtime.print_f64, builder.func);
            builder.ins().call(r, &[result_val]);
        }
        Type::String => {
            builder.declare_value_needs_stack_map(result_val);
            let r = module.declare_func_in_func(runtime.print_string, builder.func);
            builder.ins().call(r, &[result_val]);
            emit_release(builder, module, runtime, result_val);
        }
        Type::Grounded(inner) => {
            emit_entry_result_print(builder, module, runtime, inner, result_val);
        }
        _ => unreachable!("boundary check rejected non-printable returns"),
    }
}

