//! `try retry` lowering for `Option` and `Result`.
//!
//! `try expr retry { strategy }` re-evaluates `expr` up to N times
//! when it short-circuits with the failure variant, sleeping
//! between attempts according to a `Backoff` schedule. On final
//! failure the value is returned as-is to the caller. The shared
//! `emit_retry_delay_ms` helper computes per-attempt delays from
//! the `Backoff` parameters.

use super::*;

pub fn emit_retry_delay_ms(
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

pub fn lower_try_retry_result(
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

pub fn lower_try_retry_option(
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
