//! Per-type destructor synthesis for the cycle collector.
//!
//! For each refcounted type — struct, result, option — we emit a
//! `corvid_destroy_<TypeName>(payload)` function that releases the
//! refcounted slots inside the payload before `corvid_release`
//! frees the outer allocation. The destructor function pointer is
//! installed in the type's typeinfo blob (see `typeinfo.rs`).

use super::*;

/// Generate and define `corvid_destroy_<TypeName>(payload)` for a
/// struct type that has at least one refcounted field. The destructor
/// loads each refcounted field at its compile-time offset and calls
/// `corvid_release` on it. `corvid_release` then frees the struct's
/// own allocation after the destructor returns.
pub fn define_struct_destructor(
    module: &mut ObjectModule,
    ty: &corvid_ir::IrType,
    runtime: &RuntimeFuncs,
) -> Result<FuncId, CodegenError> {
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(I64));

    let symbol = format!("corvid_destroy_{}", ty.name);
    let func_id = module
        .declare_function(&symbol, Linkage::Local, &sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare destructor `{symbol}`: {e}"), ty.span)
        })?;

    let mut ctx = Context::new();
    ctx.func = Function::with_name_signature(
        UserFuncName::user(0, func_id.as_u32()),
        module
            .declarations()
            .get_function_decl(func_id)
            .signature
            .clone(),
    );
    let mut bctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut bctx);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let payload = builder.block_params(entry)[0];

        // For each refcounted field, load and release.
        let release_ref = module.declare_func_in_func(runtime.release, builder.func);
        for (i, field) in ty.fields.iter().enumerate() {
            if is_refcounted_type(&field.ty) {
                let offset = (i as i32) * STRUCT_FIELD_SLOT_BYTES;
                let v = builder.ins().load(
                    I64,
                    cranelift_codegen::ir::MemFlags::trusted(),
                    payload,
                    offset,
                );
                builder.ins().call(release_ref, &[v]);
            }
        }

        builder.ins().return_(&[]);
        builder.finalize();
    }

    define_function_with_stack_maps(
        module,
        func_id,
        &mut ctx,
        runtime,
        ty.span,
        &format!("destructor `{symbol}`"),
    )?;
    Ok(func_id)
}

pub fn define_result_destructor(
    module: &mut ObjectModule,
    result_ty: &Type,
    ok_ty: &Type,
    err_ty: &Type,
    runtime: &RuntimeFuncs,
) -> Result<FuncId, CodegenError> {
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(I64));

    let symbol = format!("corvid_destroy_{}", mangle_type_name(result_ty));
    let func_id = module
        .declare_function(&symbol, Linkage::Local, &sig)
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare destructor `{symbol}`: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut ctx = Context::new();
    ctx.func = Function::with_name_signature(
        UserFuncName::user(0, func_id.as_u32()),
        module
            .declarations()
            .get_function_decl(func_id)
            .signature
            .clone(),
    );
    let mut bctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut bctx);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);

        let payload = builder.block_params(entry)[0];
        let tag = builder.ins().load(
            I64,
            cranelift_codegen::ir::MemFlags::trusted(),
            payload,
            RESULT_TAG_OFFSET,
        );
        let release_ref = module.declare_func_in_func(runtime.release, builder.func);

        if is_refcounted_type(ok_ty) || is_refcounted_type(err_ty) {
            let ok_block = builder.create_block();
            let err_block = builder.create_block();
            let done_block = builder.create_block();
            let is_ok = builder.ins().icmp_imm(IntCC::Equal, tag, RESULT_TAG_OK);
            builder.ins().brif(is_ok, ok_block, &[], err_block, &[]);

            builder.switch_to_block(ok_block);
            builder.seal_block(ok_block);
            if is_refcounted_type(ok_ty) {
                let v = builder.ins().load(
                    I64,
                    cranelift_codegen::ir::MemFlags::trusted(),
                    payload,
                    RESULT_PAYLOAD_OFFSET,
                );
                builder.ins().call(release_ref, &[v]);
            }
            builder.ins().jump(done_block, &[]);

            builder.switch_to_block(err_block);
            builder.seal_block(err_block);
            if is_refcounted_type(err_ty) {
                let v = builder.ins().load(
                    I64,
                    cranelift_codegen::ir::MemFlags::trusted(),
                    payload,
                    RESULT_PAYLOAD_OFFSET,
                );
                builder.ins().call(release_ref, &[v]);
            }
            builder.ins().jump(done_block, &[]);

            builder.switch_to_block(done_block);
            builder.seal_block(done_block);
        }

        builder.ins().return_(&[]);
        builder.finalize();
    }

    define_function_with_stack_maps(
        module,
        func_id,
        &mut ctx,
        runtime,
        Span::new(0, 0),
        &format!("destructor `{symbol}`"),
    )?;
    Ok(func_id)
}

pub fn define_option_destructor(
    module: &mut ObjectModule,
    option_ty: &Type,
    payload_ty: &Type,
    runtime: &RuntimeFuncs,
) -> Result<FuncId, CodegenError> {
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(I64));

    let symbol = format!("corvid_destroy_{}", mangle_type_name(option_ty));
    let func_id = module
        .declare_function(&symbol, Linkage::Local, &sig)
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare option destructor `{symbol}`: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut ctx = Context::new();
    ctx.func = Function::with_name_signature(
        UserFuncName::user(0, func_id.as_u32()),
        module
            .declarations()
            .get_function_decl(func_id)
            .signature
            .clone(),
    );
    let mut bctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut bctx);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);

        let payload = builder.block_params(entry)[0];
        if is_refcounted_type(payload_ty) {
            let release_ref = module.declare_func_in_func(runtime.release, builder.func);
            let payload_val = builder.ins().load(
                I64,
                cranelift_codegen::ir::MemFlags::trusted(),
                payload,
                OPTION_PAYLOAD_OFFSET,
            );
            builder.ins().call(release_ref, &[payload_val]);
        }

        builder.ins().return_(&[]);
        builder.finalize();
    }

    define_function_with_stack_maps(
        module,
        func_id,
        &mut ctx,
        runtime,
        Span::new(0, 0),
        &format!("option destructor `{symbol}`"),
    )?;
    Ok(func_id)
}
