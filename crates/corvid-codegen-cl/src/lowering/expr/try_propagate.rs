//! `try` propagation lowering for `Option` and `Result`.
//!
//! `try expr` on a `Some(x)` / `Ok(x)` evaluates to `x`; on a
//! `None` / `Err(_)`, it returns from the enclosing function with
//! the same none/err payload re-wrapped to match the function's
//! declared return shape. Each variant lowering branches on the
//! tag, threads the success path back through the merge block,
//! and emits an early-return on the failure path.

use super::*;

pub fn lower_try_propagate_option(
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

pub fn lower_try_propagate_result(
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
