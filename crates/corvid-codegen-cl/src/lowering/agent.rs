use super::*;

/// Mangle a user agent's name into a link-safe symbol. Prevents
/// collisions with C runtime symbols (`main`, `printf`, `malloc`, ...).
///
/// Include the agent's `DefId` in the symbol because methods
/// declared inside `extend T:` blocks share their unmangled names
/// across types (`Order.total`, `Line.total` both get the AST name
/// `total`). Including the DefId disambiguates without changing the
/// emitted .obj's user-visible behavior ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â symbols are internal-only
/// (`Linkage::Local`) so the suffix never leaks into a public API.
pub(super) fn mangle_agent_symbol(user_name: &str, def_id: DefId) -> String {
    format!("corvid_agent_{user_name}_{}", def_id.0)
}


pub(super) fn reject_unsupported_types(agent: &IrAgent) -> Result<(), CodegenError> {
    for p in &agent.params {
        cl_type_for(&p.ty, p.span).map_err(|_| {
            CodegenError::not_supported(
                format!(
                    "parameter `{}: {}` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â this native lowering path supports `Int`, `Bool`, and `Float` here; `String`, `Struct`, and `List` use later lowering paths",
                    p.name,
                    p.ty.display_name()
                ),
                p.span,
            )
        })?;
    }
    cl_type_for(&agent.return_ty, agent.span).map_err(|_| {
        CodegenError::not_supported(
            format!(
                "agent `{}` returns `{}` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â this native lowering path supports `Int`, `Bool`, and `Float` returns here",
                agent.name,
                agent.return_ty.display_name()
            ),
            agent.span,
        )
    })?;
    Ok(())
}

/// Map a Corvid `Type` to the Cranelift IR type width we compile it to.
/// `Int` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢ `I64`, `Bool` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢ `I8`. Everything else raises
/// `CodegenError::NotSupported` with a descriptive feature boundary.
pub(super) fn cl_type_for(ty: &Type, span: Span) -> Result<clir::Type, CodegenError> {
    match ty {
        Type::Int => Ok(I64),
        Type::Bool => Ok(I8),
        Type::Float => Ok(F64),
        // String is a pointer to a 16-byte descriptor that lives behind
        // a 16-byte refcount header. Single I64 in registers/env keeps
        // the calling convention uniform with future Struct/List types.
        Type::String => Ok(I64),
        // Struct values are descriptor pointers (like String) ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â single
        // I64 in registers/env. The actual layout lives behind the
        // refcount header: `[header (16) | field0 (8) | ... | fieldN (8)]`.
        Type::Struct(_) => Ok(I64),
        // List values are descriptor pointers (like String, Struct) ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â
        // single I64 pointing at `[length (8) | elements...]` after the
        // 16-byte refcount header.
        Type::List(_) => Ok(I64),
        // Weak values are pointer-sized heap-managed slot boxes.
        Type::Weak(_, _) => Ok(I64),
        Type::Result(_, _) if is_native_result_type(ty) => Ok(I64),
        Type::Option(inner) if is_refcounted_type(inner) => Ok(I64),
        Type::Option(_) if is_native_wide_option_type(ty) => Ok(I64),
        Type::Grounded(inner) => cl_type_for(inner, span),
        Type::Stream(_) => Err(CodegenError::not_supported(
            "`Stream<T>` - Stream lowering is not yet implemented",
            span,
        )),
        Type::Nothing => Err(CodegenError::not_supported(
            "`Nothing` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â use a bare `return` instead",
            span,
        )),
        Type::Function { .. } => Err(CodegenError::not_supported(
            "function types as values ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â first-class callables are not implemented in native codegen yet",
            span,
        )),
        // Result<T,E> and Option<T> are accepted by the typechecker and
        // handled fully by the interpreter. Native tagged-union layout
        // and retry lowering are not implemented yet, so native
        // compilation reports a clean boundary here.
        Type::Result(_, _) => Err(CodegenError::not_supported(
            "`Result<T, E>` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â native tagged-union lowering is not implemented yet; use the interpreter tier (`corvid run --tier interp`) until then",
            span,
        )),
        Type::Option(_) => Err(CodegenError::not_supported(
            "`Option<T>` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â native codegen currently supports nullable-pointer `Option<T>` when `T` is refcounted plus wide scalar `Option<Int|Bool|Float>`; other payload shapes still need the interpreter tier",
            span,
        )),
        Type::TraceId => Err(CodegenError::not_supported(
            "`TraceId` - native codegen for replay expressions lands in Phase 21 slice 21-inv-E-4; use the interpreter tier (`corvid run --tier interp`) until then",
            span,
        )),
        Type::Unknown => Err(CodegenError::cranelift(
            "encountered `Unknown` type at codegen ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â typecheck should have caught this",
            span,
        )),
    }
}

pub(super) fn define_agent(
    module: &mut ObjectModule,
    agent: &IrAgent,
    func_id: FuncId,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    runtime: &RuntimeFuncs,
) -> Result<(), CodegenError> {
    let mut ctx = Context::new();
    ctx.func = Function::with_name_signature(
        UserFuncName::user(0, func_id.as_u32()),
        module.declarations().get_function_decl(func_id).signature.clone(),
    );

    let mut builder_ctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);

        let mut env: HashMap<LocalId, (Variable, clir::Type)> = HashMap::new();
        let mut var_idx: usize = 0;
        // The function-root scope. `lower_block` for the agent body
        // does NOT push its own scope (it would double-push); this is
        // it. Branch blocks inside `if`/`else` push/pop their own.
        let mut scope_stack: Vec<Vec<(LocalId, Variable)>> = vec![Vec::new()];
        // Loop context stack ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â empty at function entry. `for` pushes
        // on entry, `break`/`continue` consult the top entry, `for`
        // pops on exit.
        let mut loop_stack: Vec<LoopCtx> = Vec::new();

        for (i, p) in agent.params.iter().enumerate() {
            let block_arg = builder.block_params(entry)[i];
            let var = Variable::from_u32(var_idx as u32);
            var_idx += 1;
            let ty = cl_type_for(&p.ty, p.span)?;
            builder.declare_var(var, ty);
            // Mark the refcounted parameter Value so
            // Cranelift spills it before safepoints and records its
            // SP-relative offset in the function's stack map. The
            // `declare_value_needs_stack_map` API is per-Value (not
            // per-Variable); the safepoint liveness pass handles
            // SSA-phi flow for values that travel through this
            // Variable across blocks.
            //
            // The cycle collector mark walk uses these recorded
            // offsets at each safepoint PC to find on-stack GC roots.
            if is_refcounted_type(&p.ty) {
                builder.declare_value_needs_stack_map(block_arg);
            }
            builder.def_var(var, block_arg);
            env.insert(p.local_id, (var, ty));
            // Borrow ABI driven by `agent.borrow_sig`:
            //   * `ParamBorrow::Owned` (pre-17b default, and what a
            //      consuming body needs): callee retains on entry,
            //      tracks in function-root scope for symmetric
            //      scope-exit release.
            //   * `ParamBorrow::Borrowed`: caller keeps its +1 and
            //      the callee does NOT retain nor release ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â ownership
            //      doesn't cross the ABI. Saves one retain + one
            //      release per call site for read-only parameters.
            //
            // `borrow_sig = None` means the ownership pass didn't
            // run on this agent ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â fall back to Owned (pre-17b
            // semantics). Keeps parity for code paths that bypass
            // `ownership::analyze`.
            if is_refcounted_type(&p.ty) {
                let is_borrowed = agent
                    .borrow_sig
                    .as_ref()
                    .and_then(|v| v.get(i).copied())
                    .map(|b| matches!(b, corvid_ir::ParamBorrow::Borrowed))
                    .unwrap_or(false);
                if !is_borrowed {
                    if !runtime.dup_drop_enabled {
                        emit_retain(&mut builder, module, runtime, block_arg);
                    }
                    // scope_stack tracking is still needed even under
                    // dup_drop_enabled for the fallback path to work
                    // (CORVID_DUP_DROP_PASS=0). The scope walk at
                    // return/break/continue guards its emit_release
                    // calls on the flag, so carrying the entry is
                    // free when the flag is on.
                    scope_stack[0].push((p.local_id, var));
                }
                // Borrowed: no retain, no scope tracking. Caller's
                // +1 stays with the caller; the callee just reads.
            }
        }

        let lowered = lower_block(
            &mut builder,
            &agent.body,
            &agent.return_ty,
            &mut env,
            &mut var_idx,
            &mut scope_stack,
            &mut loop_stack,
            func_ids_by_def,
            module,
            runtime,
        )?;

        // Defensive fallthrough terminator. `lower_block` returns
        // `BlockOutcome::Terminated` after emitting a `return`, so we
        // only reach here for agents whose type checker somehow allowed
        // a missing return (shouldn't happen for `Int` agents). Terminate
        // with a trap so the verifier doesn't reject the function.
        match lowered {
            BlockOutcome::Normal => {
                if builder.current_block().is_some() {
                    let cur = builder.current_block().unwrap();
                    if !builder.func.layout.is_block_inserted(cur) {
                        // Should not happen ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â a block was switched to but
                        // not inserted. Terminator still needed.
                    }
                    // If Cranelift considers the block unterminated,
                    // trap ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â an untyped fallthrough on an `Int` return
                    // is already a frontend bug.
                    let last_inst = builder.func.layout.last_inst(cur);
                    let terminated = last_inst
                        .map(|i| builder.func.dfg.insts[i].opcode().is_terminator())
                        .unwrap_or(false);
                    if !terminated {
                        builder
                            .ins()
                            .trap(cranelift_codegen::ir::TrapCode::INTEGER_OVERFLOW);
                    }
                }
            }
            BlockOutcome::Terminated => {}
        }

        builder.finalize();
    }

    define_function_with_stack_maps(
        module,
        func_id,
        &mut ctx,
        runtime,
        agent.span,
        &format!("agent `{}`", agent.name),
    )?;
    Ok(())
}

