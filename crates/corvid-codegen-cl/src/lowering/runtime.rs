use super::*;

pub(super) fn declare_runtime_funcs(
    module: &mut ObjectModule,
    overflow_func_id: FuncId,
) -> Result<RuntimeFuncs, CodegenError> {
    let mut retain_sig = module.make_signature();
    retain_sig.params.push(AbiParam::new(I64));
    let retain_id = module
        .declare_function(RETAIN_SYMBOL, Linkage::Import, &retain_sig)
        .map_err(|e| CodegenError::cranelift(format!("declare retain: {e}"), Span::new(0, 0)))?;

    let mut release_sig = module.make_signature();
    release_sig.params.push(AbiParam::new(I64));
    let release_id = module
        .declare_function(RELEASE_SYMBOL, Linkage::Import, &release_sig)
        .map_err(|e| CodegenError::cranelift(format!("declare release: {e}"), Span::new(0, 0)))?;

    let mut concat_sig = module.make_signature();
    concat_sig.params.push(AbiParam::new(I64));
    concat_sig.params.push(AbiParam::new(I64));
    concat_sig.returns.push(AbiParam::new(I64));
    let concat_id = module
        .declare_function(STRING_CONCAT_SYMBOL, Linkage::Import, &concat_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare string_concat: {e}"), Span::new(0, 0))
        })?;

    let mut eq_sig = module.make_signature();
    eq_sig.params.push(AbiParam::new(I64));
    eq_sig.params.push(AbiParam::new(I64));
    eq_sig.returns.push(AbiParam::new(I64));
    let eq_id = module
        .declare_function(STRING_EQ_SYMBOL, Linkage::Import, &eq_sig)
        .map_err(|e| CodegenError::cranelift(format!("declare string_eq: {e}"), Span::new(0, 0)))?;

    let mut cmp_sig = module.make_signature();
    cmp_sig.params.push(AbiParam::new(I64));
    cmp_sig.params.push(AbiParam::new(I64));
    cmp_sig.returns.push(AbiParam::new(I64));
    let cmp_id = module
        .declare_function(STRING_CMP_SYMBOL, Linkage::Import, &cmp_sig)
        .map_err(|e| CodegenError::cranelift(format!("declare string_cmp: {e}"), Span::new(0, 0)))?;

    // corvid_alloc_typed(size: i64, typeinfo: i64) -> i64
    let mut alloc_typed_sig = module.make_signature();
    alloc_typed_sig.params.push(AbiParam::new(I64));
    alloc_typed_sig.params.push(AbiParam::new(I64));
    alloc_typed_sig.returns.push(AbiParam::new(I64));
    let alloc_typed_id = module
        .declare_function(ALLOC_TYPED_SYMBOL, Linkage::Import, &alloc_typed_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare alloc_typed: {e}"), Span::new(0, 0))
        })?;

    // corvid_destroy_list(payload) -> void — installed in every
    // refcounted-element list's typeinfo, referenced from the data
    // emission via write_function_addr.
    let mut list_destroy_sig = module.make_signature();
    list_destroy_sig.params.push(AbiParam::new(I64));
    let list_destroy_id = module
        .declare_function(LIST_DESTROY_SYMBOL, Linkage::Import, &list_destroy_sig)
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare list destroy: {e}"),
                Span::new(0, 0),
            )
        })?;

    // corvid_trace_list(payload, marker, ctx) -> void — installed in
    // every list's typeinfo (primitive-element included; the fn
    // no-ops when elem_typeinfo is NULL).
    let mut list_trace_sig = module.make_signature();
    list_trace_sig.params.push(AbiParam::new(I64));
    list_trace_sig.params.push(AbiParam::new(I64));
    list_trace_sig.params.push(AbiParam::new(I64));
    let list_trace_id = module
        .declare_function(LIST_TRACE_SYMBOL, Linkage::Import, &list_trace_sig)
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare list trace: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut weak_unary_sig = module.make_signature();
    weak_unary_sig.params.push(AbiParam::new(I64));
    weak_unary_sig.returns.push(AbiParam::new(I64));
    let weak_new_id = module
        .declare_function(WEAK_NEW_SYMBOL, Linkage::Import, &weak_unary_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare weak_new: {e}"), Span::new(0, 0))
        })?;
    let weak_upgrade_id = module
        .declare_function(WEAK_UPGRADE_SYMBOL, Linkage::Import, &weak_unary_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare weak_upgrade: {e}"), Span::new(0, 0))
        })?;

    let mut weak_clear_sig = module.make_signature();
    weak_clear_sig.params.push(AbiParam::new(I64));
    let weak_clear_self_id = module
        .declare_function(WEAK_CLEAR_SELF_SYMBOL, Linkage::Import, &weak_clear_sig)
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare weak_clear_self: {e}"),
                Span::new(0, 0),
            )
        })?;

    // corvid_typeinfo_String — runtime-provided data symbol. Declared
    // here so codegen can reference it from static string literal
    // descriptors and from List<String>'s elem_typeinfo slot.
    let string_typeinfo_id = module
        .declare_data(STRING_TYPEINFO_SYMBOL, Linkage::Import, false, false)
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare String typeinfo: {e}"),
                Span::new(0, 0),
            )
        })?;
    let weak_box_typeinfo_id = module
        .declare_data(WEAK_BOX_TYPEINFO_SYMBOL, Linkage::Import, false, false)
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare WeakBox typeinfo: {e}"),
                Span::new(0, 0),
            )
        })?;

    // ---- native entry helpers ----
    let void_void_sig = module.make_signature();
    let entry_init_id = module
        .declare_function(ENTRY_INIT_SYMBOL, Linkage::Import, &void_void_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare corvid_init: {e}"), Span::new(0, 0))
        })?;

    let mut arity_sig = module.make_signature();
    arity_sig.params.push(AbiParam::new(I64));
    arity_sig.params.push(AbiParam::new(I64));
    let arity_id = module
        .declare_function(ENTRY_ARITY_MISMATCH_SYMBOL, Linkage::Import, &arity_sig)
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare arity_mismatch: {e}"),
                Span::new(0, 0),
            )
        })?;

    // parse helpers: (cstr_ptr, argv_index) -> typed value
    let mut parse_i64_sig = module.make_signature();
    parse_i64_sig.params.push(AbiParam::new(I64));
    parse_i64_sig.params.push(AbiParam::new(I64));
    parse_i64_sig.returns.push(AbiParam::new(I64));
    let parse_i64_id = module
        .declare_function(PARSE_I64_SYMBOL, Linkage::Import, &parse_i64_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare parse_i64: {e}"), Span::new(0, 0))
        })?;

    let mut parse_f64_sig = module.make_signature();
    parse_f64_sig.params.push(AbiParam::new(I64));
    parse_f64_sig.params.push(AbiParam::new(I64));
    parse_f64_sig.returns.push(AbiParam::new(F64));
    let parse_f64_id = module
        .declare_function(PARSE_F64_SYMBOL, Linkage::Import, &parse_f64_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare parse_f64: {e}"), Span::new(0, 0))
        })?;

    let mut parse_bool_sig = module.make_signature();
    parse_bool_sig.params.push(AbiParam::new(I64));
    parse_bool_sig.params.push(AbiParam::new(I64));
    parse_bool_sig.returns.push(AbiParam::new(I8));
    let parse_bool_id = module
        .declare_function(PARSE_BOOL_SYMBOL, Linkage::Import, &parse_bool_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare parse_bool: {e}"), Span::new(0, 0))
        })?;

    // corvid_string_from_cstr(cstr_ptr) -> descriptor
    let mut from_cstr_sig = module.make_signature();
    from_cstr_sig.params.push(AbiParam::new(I64));
    from_cstr_sig.returns.push(AbiParam::new(I64));
    let from_cstr_id = module
        .declare_function(STRING_FROM_CSTR_SYMBOL, Linkage::Import, &from_cstr_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare string_from_cstr: {e}"), Span::new(0, 0))
        })?;

    // print helpers
    let mut print_i64_sig = module.make_signature();
    print_i64_sig.params.push(AbiParam::new(I64));
    let print_i64_id = module
        .declare_function(PRINT_I64_SYMBOL, Linkage::Import, &print_i64_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare print_i64: {e}"), Span::new(0, 0))
        })?;

    let mut print_bool_sig = module.make_signature();
    print_bool_sig.params.push(AbiParam::new(I64));
    let print_bool_id = module
        .declare_function(PRINT_BOOL_SYMBOL, Linkage::Import, &print_bool_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare print_bool: {e}"), Span::new(0, 0))
        })?;

    let mut print_f64_sig = module.make_signature();
    print_f64_sig.params.push(AbiParam::new(F64));
    let print_f64_id = module
        .declare_function(PRINT_F64_SYMBOL, Linkage::Import, &print_f64_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare print_f64: {e}"), Span::new(0, 0))
        })?;

    let mut print_string_sig = module.make_signature();
    print_string_sig.params.push(AbiParam::new(I64));
    let print_string_id = module
        .declare_function(PRINT_STRING_SYMBOL, Linkage::Import, &print_string_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare print_string: {e}"), Span::new(0, 0))
        })?;
    let mut bench_enabled_sig = module.make_signature();
    bench_enabled_sig.returns.push(AbiParam::new(I64));
    let bench_server_enabled_id = module
        .declare_function(BENCH_SERVER_ENABLED_SYMBOL, Linkage::Import, &bench_enabled_sig)
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare bench_server_enabled: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut bench_next_sig = module.make_signature();
    bench_next_sig.returns.push(AbiParam::new(I64));
    let bench_next_trial_id = module
        .declare_function(BENCH_NEXT_TRIAL_SYMBOL, Linkage::Import, &bench_next_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare bench_next_trial: {e}"), Span::new(0, 0))
        })?;

    let mut bench_finish_sig = module.make_signature();
    bench_finish_sig.params.push(AbiParam::new(I64));
    let bench_finish_trial_id = module
        .declare_function(BENCH_FINISH_TRIAL_SYMBOL, Linkage::Import, &bench_finish_sig)
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare bench_finish_trial: {e}"),
                Span::new(0, 0),
            )
        })?;

    // Runtime bridge imports.
    let mut tool_call_sync_int_sig = module.make_signature();
    tool_call_sync_int_sig.params.push(AbiParam::new(I64)); // name_ptr
    tool_call_sync_int_sig.params.push(AbiParam::new(I64)); // name_len
    tool_call_sync_int_sig.returns.push(AbiParam::new(I64)); // i64 result
    let tool_call_sync_int_id = module
        .declare_function(
            TOOL_CALL_SYNC_INT_SYMBOL,
            Linkage::Import,
            &tool_call_sync_int_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare tool_call_sync_int: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut runtime_is_replay_sig = module.make_signature();
    runtime_is_replay_sig.returns.push(AbiParam::new(I8));
    let runtime_is_replay_id = module
        .declare_function(
            RUNTIME_IS_REPLAY_SYMBOL,
            Linkage::Import,
            &runtime_is_replay_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare runtime_is_replay: {e}"),
                Span::new(0, 0),
            )
        })?;

    let make_replay_tool_sig =
        |module: &mut ObjectModule, ret_ty: Option<cranelift_codegen::ir::Type>| {
            let mut sig = module.make_signature();
            sig.params.push(AbiParam::new(I64));
            sig.params.push(AbiParam::new(I64));
            sig.params.push(AbiParam::new(I64));
            sig.params.push(AbiParam::new(I64));
            if let Some(ret_ty) = ret_ty {
                sig.returns.push(AbiParam::new(ret_ty));
            }
            sig
        };
    let replay_tool_nothing_sig = make_replay_tool_sig(module, None);
    let replay_tool_call_nothing_id = module
        .declare_function(
            REPLAY_TOOL_CALL_NOTHING_SYMBOL,
            Linkage::Import,
            &replay_tool_nothing_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare replay_tool_call_nothing: {e}"),
                Span::new(0, 0),
            )
        })?;
    let replay_tool_int_sig = make_replay_tool_sig(module, Some(I64));
    let replay_tool_call_int_id = module
        .declare_function(
            REPLAY_TOOL_CALL_INT_SYMBOL,
            Linkage::Import,
            &replay_tool_int_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare replay_tool_call_int: {e}"),
                Span::new(0, 0),
            )
        })?;
    let replay_tool_bool_sig = make_replay_tool_sig(module, Some(I8));
    let replay_tool_call_bool_id = module
        .declare_function(
            REPLAY_TOOL_CALL_BOOL_SYMBOL,
            Linkage::Import,
            &replay_tool_bool_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare replay_tool_call_bool: {e}"),
                Span::new(0, 0),
            )
        })?;
    let replay_tool_float_sig = make_replay_tool_sig(module, Some(F64));
    let replay_tool_call_float_id = module
        .declare_function(
            REPLAY_TOOL_CALL_FLOAT_SYMBOL,
            Linkage::Import,
            &replay_tool_float_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare replay_tool_call_float: {e}"),
                Span::new(0, 0),
            )
        })?;
    let replay_tool_string_sig = make_replay_tool_sig(module, Some(I64));
    let replay_tool_call_string_id = module
        .declare_function(
            REPLAY_TOOL_CALL_STRING_SYMBOL,
            Linkage::Import,
            &replay_tool_string_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare replay_tool_call_string: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut runtime_init_sig = module.make_signature();
    runtime_init_sig.returns.push(AbiParam::new(I32));
    let runtime_init_id = module
        .declare_function(RUNTIME_INIT_SYMBOL, Linkage::Import, &runtime_init_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare runtime_init: {e}"), Span::new(0, 0))
        })?;

    let runtime_shutdown_sig = module.make_signature();
    let runtime_shutdown_id = module
        .declare_function(
            RUNTIME_SHUTDOWN_SYMBOL,
            Linkage::Import,
            &runtime_shutdown_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare runtime_shutdown: {e}"),
                Span::new(0, 0),
            )
        })?;

    let embed_init_sig = module.make_signature();
    let embed_init_id = module
        .declare_function(
            RUNTIME_EMBED_INIT_SYMBOL,
            Linkage::Import,
            &embed_init_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare runtime_embed_init: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut string_into_cstr_sig = module.make_signature();
    string_into_cstr_sig.params.push(AbiParam::new(I64));
    string_into_cstr_sig.returns.push(AbiParam::new(I64));
    let string_into_cstr_id = module
        .declare_function(
            STRING_INTO_CSTR_SYMBOL,
            Linkage::Import,
            &string_into_cstr_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare string_into_cstr: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut sleep_ms_sig = module.make_signature();
    sleep_ms_sig.params.push(AbiParam::new(I64));
    let sleep_ms_id = module
        .declare_function(SLEEP_MS_SYMBOL, Linkage::Import, &sleep_ms_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare sleep_ms: {e}"), Span::new(0, 0))
        })?;

    // Stringification helpers. Each takes a typed scalar
    // and returns a Corvid String descriptor pointer (i64).
    let mut sfi_sig = module.make_signature();
    sfi_sig.params.push(AbiParam::new(I64));
    sfi_sig.returns.push(AbiParam::new(I64));
    let string_from_int_id = module
        .declare_function(STRING_FROM_INT_SYMBOL, Linkage::Import, &sfi_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare string_from_int: {e}"), Span::new(0, 0))
        })?;

    let mut sfb_sig = module.make_signature();
    sfb_sig.params.push(AbiParam::new(I8));
    sfb_sig.returns.push(AbiParam::new(I64));
    let string_from_bool_id = module
        .declare_function(STRING_FROM_BOOL_SYMBOL, Linkage::Import, &sfb_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare string_from_bool: {e}"), Span::new(0, 0))
        })?;

    let mut sff_sig = module.make_signature();
    sff_sig.params.push(AbiParam::new(F64));
    sff_sig.returns.push(AbiParam::new(I64));
    let string_from_float_id = module
        .declare_function(STRING_FROM_FLOAT_SYMBOL, Linkage::Import, &sff_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare string_from_float: {e}"), Span::new(0, 0))
        })?;

    let mut approve_sig = module.make_signature();
    approve_sig.params.push(AbiParam::new(I64));
    approve_sig.params.push(AbiParam::new(I64));
    approve_sig.params.push(AbiParam::new(I64));
    approve_sig.params.push(AbiParam::new(I64));
    approve_sig.returns.push(AbiParam::new(I8));
    let approve_sync_id = module
        .declare_function(APPROVE_SYNC_SYMBOL, Linkage::Import, &approve_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare approve_sync: {e}"), Span::new(0, 0))
        })?;

    // Typed prompt bridges. Each takes 7 args:
    //   prompt name, signature, rendered template, model,
    //   arg type-tag string, argc, arg value slots pointer.
    let make_prompt_sig =
        |module: &mut ObjectModule, ret_ty: cranelift_codegen::ir::Type| {
            let mut s = module.make_signature();
            s.params.push(AbiParam::new(I64));
            s.params.push(AbiParam::new(I64));
            s.params.push(AbiParam::new(I64));
            s.params.push(AbiParam::new(I64));
            s.params.push(AbiParam::new(I64));
            s.params.push(AbiParam::new(I64));
            s.params.push(AbiParam::new(I64));
            s.returns.push(AbiParam::new(ret_ty));
            s
        };
    let prompt_int_sig = make_prompt_sig(module, I64);
    let prompt_call_int_id = module
        .declare_function(PROMPT_CALL_INT_SYMBOL, Linkage::Import, &prompt_int_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare prompt_call_int: {e}"), Span::new(0, 0))
        })?;
    let prompt_bool_sig = make_prompt_sig(module, I8);
    let prompt_call_bool_id = module
        .declare_function(PROMPT_CALL_BOOL_SYMBOL, Linkage::Import, &prompt_bool_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare prompt_call_bool: {e}"), Span::new(0, 0))
        })?;
    let prompt_float_sig = make_prompt_sig(module, F64);
    let prompt_call_float_id = module
        .declare_function(PROMPT_CALL_FLOAT_SYMBOL, Linkage::Import, &prompt_float_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare prompt_call_float: {e}"), Span::new(0, 0))
        })?;
    let prompt_string_sig = make_prompt_sig(module, I64); // returns CorvidString descriptor ptr
    let prompt_call_string_id = module
        .declare_function(PROMPT_CALL_STRING_SYMBOL, Linkage::Import, &prompt_string_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare prompt_call_string: {e}"), Span::new(0, 0))
        })?;

    let mut trace_run_started_sig = module.make_signature();
    trace_run_started_sig.params.push(AbiParam::new(I64));
    trace_run_started_sig.params.push(AbiParam::new(I64));
    trace_run_started_sig.params.push(AbiParam::new(I64));
    trace_run_started_sig.params.push(AbiParam::new(I64));
    let trace_run_started_id = module
        .declare_function(TRACE_RUN_STARTED_SYMBOL, Linkage::Import, &trace_run_started_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare trace_run_started: {e}"), Span::new(0, 0))
        })?;

    let mut trace_run_completed_int_sig = module.make_signature();
    trace_run_completed_int_sig.params.push(AbiParam::new(I64));
    let trace_run_completed_int_id = module
        .declare_function(
            TRACE_RUN_COMPLETED_INT_SYMBOL,
            Linkage::Import,
            &trace_run_completed_int_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare trace_run_completed_int: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut trace_run_completed_bool_sig = module.make_signature();
    trace_run_completed_bool_sig.params.push(AbiParam::new(I8));
    let trace_run_completed_bool_id = module
        .declare_function(
            TRACE_RUN_COMPLETED_BOOL_SYMBOL,
            Linkage::Import,
            &trace_run_completed_bool_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare trace_run_completed_bool: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut trace_run_completed_float_sig = module.make_signature();
    trace_run_completed_float_sig.params.push(AbiParam::new(F64));
    let trace_run_completed_float_id = module
        .declare_function(
            TRACE_RUN_COMPLETED_FLOAT_SYMBOL,
            Linkage::Import,
            &trace_run_completed_float_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare trace_run_completed_float: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut trace_run_completed_string_sig = module.make_signature();
    trace_run_completed_string_sig.params.push(AbiParam::new(I64));
    let trace_run_completed_string_id = module
        .declare_function(
            TRACE_RUN_COMPLETED_STRING_SYMBOL,
            Linkage::Import,
            &trace_run_completed_string_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare trace_run_completed_string: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut trace_tool_call_sig = module.make_signature();
    trace_tool_call_sig.params.push(AbiParam::new(I64));
    trace_tool_call_sig.params.push(AbiParam::new(I64));
    trace_tool_call_sig.params.push(AbiParam::new(I64));
    trace_tool_call_sig.params.push(AbiParam::new(I64));
    let trace_tool_call_id = module
        .declare_function(TRACE_TOOL_CALL_SYMBOL, Linkage::Import, &trace_tool_call_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare trace_tool_call: {e}"), Span::new(0, 0))
        })?;

    let mut trace_tool_result_null_sig = module.make_signature();
    trace_tool_result_null_sig.params.push(AbiParam::new(I64));
    let trace_tool_result_null_id = module
        .declare_function(
            TRACE_TOOL_RESULT_NULL_SYMBOL,
            Linkage::Import,
            &trace_tool_result_null_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare trace_tool_result_null: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut trace_tool_result_int_sig = module.make_signature();
    trace_tool_result_int_sig.params.push(AbiParam::new(I64));
    trace_tool_result_int_sig.params.push(AbiParam::new(I64));
    let trace_tool_result_int_id = module
        .declare_function(
            TRACE_TOOL_RESULT_INT_SYMBOL,
            Linkage::Import,
            &trace_tool_result_int_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare trace_tool_result_int: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut trace_tool_result_bool_sig = module.make_signature();
    trace_tool_result_bool_sig.params.push(AbiParam::new(I64));
    trace_tool_result_bool_sig.params.push(AbiParam::new(I8));
    let trace_tool_result_bool_id = module
        .declare_function(
            TRACE_TOOL_RESULT_BOOL_SYMBOL,
            Linkage::Import,
            &trace_tool_result_bool_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare trace_tool_result_bool: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut trace_tool_result_float_sig = module.make_signature();
    trace_tool_result_float_sig.params.push(AbiParam::new(I64));
    trace_tool_result_float_sig.params.push(AbiParam::new(F64));
    let trace_tool_result_float_id = module
        .declare_function(
            TRACE_TOOL_RESULT_FLOAT_SYMBOL,
            Linkage::Import,
            &trace_tool_result_float_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare trace_tool_result_float: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut trace_tool_result_string_sig = module.make_signature();
    trace_tool_result_string_sig.params.push(AbiParam::new(I64));
    trace_tool_result_string_sig.params.push(AbiParam::new(I64));
    let trace_tool_result_string_id = module
        .declare_function(
            TRACE_TOOL_RESULT_STRING_SYMBOL,
            Linkage::Import,
            &trace_tool_result_string_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare trace_tool_result_string: {e}"),
                Span::new(0, 0),
            )
        })?;

    Ok(RuntimeFuncs {
        overflow: overflow_func_id,
        retain: retain_id,
        release: release_id,
        string_concat: concat_id,
        string_eq: eq_id,
        string_cmp: cmp_id,
        alloc_typed: alloc_typed_id,
        list_destroy: list_destroy_id,
        list_trace: list_trace_id,
        weak_new: weak_new_id,
        weak_upgrade: weak_upgrade_id,
        weak_clear_self: weak_clear_self_id,
        string_typeinfo: string_typeinfo_id,
        weak_box_typeinfo: weak_box_typeinfo_id,
        entry_init: entry_init_id,
        entry_arity_mismatch: arity_id,
        parse_i64: parse_i64_id,
        parse_f64: parse_f64_id,
        parse_bool: parse_bool_id,
        string_from_cstr: from_cstr_id,
        print_i64: print_i64_id,
        print_bool: print_bool_id,
        print_f64: print_f64_id,
        print_string: print_string_id,
        bench_server_enabled: bench_server_enabled_id,
        bench_next_trial: bench_next_trial_id,
        bench_finish_trial: bench_finish_trial_id,
        tool_call_sync_int: tool_call_sync_int_id,
        runtime_is_replay: runtime_is_replay_id,
        replay_tool_call_nothing: replay_tool_call_nothing_id,
        replay_tool_call_int: replay_tool_call_int_id,
        replay_tool_call_bool: replay_tool_call_bool_id,
        replay_tool_call_float: replay_tool_call_float_id,
        replay_tool_call_string: replay_tool_call_string_id,
        runtime_init: runtime_init_id,
        runtime_shutdown: runtime_shutdown_id,
        runtime_embed_init: embed_init_id,
        sleep_ms: sleep_ms_id,
        string_into_cstr: string_into_cstr_id,
        string_from_int: string_from_int_id,
        string_from_bool: string_from_bool_id,
        string_from_float: string_from_float_id,
        approve_sync: approve_sync_id,
        prompt_call_int: prompt_call_int_id,
        prompt_call_bool: prompt_call_bool_id,
        prompt_call_float: prompt_call_float_id,
        prompt_call_string: prompt_call_string_id,
        trace_run_started: trace_run_started_id,
        trace_run_completed_int: trace_run_completed_int_id,
        trace_run_completed_bool: trace_run_completed_bool_id,
        trace_run_completed_float: trace_run_completed_float_id,
        trace_run_completed_string: trace_run_completed_string_id,
        trace_tool_call: trace_tool_call_id,
        trace_tool_result_null: trace_tool_result_null_id,
        trace_tool_result_int: trace_tool_result_int_id,
        trace_tool_result_bool: trace_tool_result_bool_id,
        trace_tool_result_float: trace_tool_result_float_id,
        trace_tool_result_string: trace_tool_result_string_id,
        literal_counter: std::cell::Cell::new(0),
        // The unified ownership pass is the default. It produces
        // refcount-correct code
        // across all 106 parity fixtures + 9 verifier-audit classes,
        // with systematically lower RC op counts than the peephole-
        // optimized predecessor.
        //
        // Set `CORVID_DUP_DROP_PASS=0` to fall back to the legacy
        // scattered-emit codegen for A/B comparison. That fallback
        // path will be removed once the unified pass has fully
        // stabilized in production.
        dup_drop_enabled: std::env::var("CORVID_DUP_DROP_PASS")
            .map(|v| v != "0" && v != "false")
            .unwrap_or(true),
        struct_destructors: HashMap::new(),
        struct_traces: HashMap::new(),
        struct_typeinfos: HashMap::new(),
        list_typeinfos: HashMap::new(),
        result_destructors: HashMap::new(),
        result_traces: HashMap::new(),
        result_typeinfos: HashMap::new(),
        option_typeinfos: HashMap::new(),
        ir_types: HashMap::new(),
        ir_tools: HashMap::new(),
        tool_wrapper_ids: std::cell::RefCell::new(HashMap::new()),
        ir_prompts: HashMap::new(),
        prompt_pins: HashMap::new(),
        agent_borrow_sigs: HashMap::new(),
        stack_maps: std::cell::RefCell::new(HashMap::new()),
    })
}

/// Define a Cranelift function and preserve its user
/// stack maps out of `CompiledCode.buffer` before they're discarded
/// by `ObjectModule::define_function`.
///
/// Why this helper exists: cranelift-object 0.116's `define_function`
/// internally does `compile(isa, ctrl)` → `define_function_bytes(...)`
/// but only passes `code_buffer()` + `relocs()` through to the
/// backend, dropping `user_stack_maps()` on the floor. The stack
/// maps are produced by compilation — they're correct — but the
/// caller has no hook to observe them. This helper replicates the
/// internal two-step path, intercepting the stack maps in between.
///
/// The intercepted maps are stashed in `runtime.stack_maps` keyed
/// by `func_id`. After `module.finish()` we emit a `corvid_stack_maps`
/// `.rodata` symbol with function-pointer relocations so the cycle
/// collector's mark walk can look up a stack map given a return PC.
///
/// Signature mirrors `ObjectModule::define_function` so call-site
/// rewrites are a straight substitution.
pub(super) fn define_function_with_stack_maps(
    module: &mut ObjectModule,
    func_id: FuncId,
    ctx: &mut Context,
    runtime: &RuntimeFuncs,
    error_span: Span,
    error_context: &str,
) -> Result<(), CodegenError> {
    let mut ctrl_plane = ControlPlane::default();
    ctx.compile(module.isa(), &mut ctrl_plane).map_err(|e| {
        CodegenError::cranelift(
            format!("compile `{error_context}`: {e:?}"),
            error_span,
        )
    })?;

    // Rescue the stack maps before the compile result borrow is
    // dropped. `to_vec()` clones the SmallVec into a Vec; the
    // UserStackMap entries themselves are Clone.
    let code = ctx
        .compiled_code()
        .expect("compile just succeeded; compiled_code must be Some");
    let extracted: Vec<(CodeOffset, u32, UserStackMap)> = code
        .buffer
        .user_stack_maps()
        .iter()
        .map(|(offset, span, sm)| (*offset, *span, sm.clone()))
        .collect();
    if !extracted.is_empty() {
        runtime
            .stack_maps
            .borrow_mut()
            .insert(func_id, extracted);
    }

    // Replicate cranelift-object's inner path: feed the already-
    // compiled bytes + relocs + alignment straight into
    // define_function_bytes. This is precisely what
    // ObjectModule::define_function does internally, minus the
    // dropped-on-the-floor stack maps.
    let alignment = code.buffer.alignment as u64;
    module
        .define_function_bytes(
            func_id,
            &ctx.func,
            alignment,
            ctx.compiled_code().unwrap().code_buffer(),
            ctx.compiled_code().unwrap().buffer.relocs(),
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("define `{error_context}`: {e}"),
                error_span,
            )
        })?;
    Ok(())
}

/// Emit the `corvid_stack_maps` data symbol from the
/// accumulated per-function stack maps in `runtime.stack_maps`.
///
/// Binary layout of `corvid_stack_maps` (must match the reader in
/// `corvid-runtime/runtime/stack_maps.c`):
///
/// ```text
///   offset  0 :  u64 entry_count
///   offset  8 :  u64 reserved (= 0; available for future metadata)
///   offset 16 :  entries[entry_count] — each 32 bytes:
///                   +0  u64 fn_start     (relocated to function symbol)
///                   +8  u32 pc_offset    (inline; return-PC = fn_start + pc_offset)
///                  +12  u32 frame_bytes  (size of the safepoint's callsite)
///                  +16  u32 ref_count    (number of refcounted slots)
///                  +20  u32 _pad
///                  +24  u64 ref_offsets  (relocated into the refs pool below)
///              then refs pool: flat u32 array of SP-relative slot offsets
/// ```
///
/// Runtime lookup (the collector's mark walk, via `corvid_stack_maps_find`):
/// given a return PC from an on-stack frame, scan entries computing
/// `fn_start + pc_offset` and match. For each match, walk the
/// `ref_count`-length `ref_offsets` array; each u32 is a byte offset
/// from SP where a live refcounted pointer lives.
///
/// Called once at the end of `lower_file` after every function has
/// been compiled and its stack maps captured. Emits even when empty
/// so downstream consumers don't fail with unresolved-symbol errors
/// on programs that have no refcounted values.
pub(super) fn emit_stack_map_table(
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<(), CodegenError> {
    const HEADER_BYTES: usize = 16;
    const ENTRY_BYTES: usize = 32;
    const ENTRY_OFF_FN_START: u32 = 0;
    const ENTRY_OFF_PC_OFFSET: u32 = 8;
    const ENTRY_OFF_FRAME_BYTES: u32 = 12;
    const ENTRY_OFF_REF_COUNT: u32 = 16;
    const ENTRY_OFF_REFS_PTR: u32 = 24;

    // Collect + filter entries. Each Cranelift `UserStackMap`
    // `entries()` yields `(Type, offset)`. We keep only `I64`
    // entries (refcounted pointers in Corvid are always I64;
    // primitives like I32/F64 would not be declare'd for stack-map
    // inclusion and shouldn't appear, but filtering belt-and-braces).
    //
    // Sort (func_id, pc_offset) for deterministic output — matters
    // for reproducible builds and for future binary-search lookup.
    let stack_maps = runtime.stack_maps.borrow();
    let mut entries: Vec<(FuncId, CodeOffset, u32, Vec<u32>)> = Vec::new();
    for (func_id, fn_maps) in stack_maps.iter() {
        for (pc_offset, span, sm) in fn_maps {
            let mut refs: Vec<u32> = sm
                .entries()
                .filter(|(ty, _)| *ty == I64)
                .map(|(_, off)| off)
                .collect();
            refs.sort_unstable();
            entries.push((*func_id, *pc_offset, *span, refs));
        }
    }
    entries.sort_by_key(|(f, o, _, _)| (f.as_u32(), *o));

    let n = entries.len();
    let refs_pool_offset = HEADER_BYTES + n * ENTRY_BYTES;
    let total_refs: usize = entries.iter().map(|(_, _, _, r)| r.len()).sum();
    let total_bytes = refs_pool_offset + total_refs * 4;

    let mut bytes = vec![0u8; total_bytes];
    bytes[0..8].copy_from_slice(&(n as u64).to_le_bytes());
    // bytes[8..16] — reserved, zero

    // Record (entry_byte_offset, func_id, refs_pool_byte_offset) for
    // the relocation pass below. Also write inline fields now.
    let mut reloc_recs: Vec<(u32, FuncId, u32)> = Vec::with_capacity(n);
    let mut refs_cursor = refs_pool_offset;
    for (i, (func_id, pc_offset, frame_bytes, refs)) in entries.iter().enumerate() {
        let entry_byte = HEADER_BYTES + i * ENTRY_BYTES;
        // fn_start (offset +0): zeroed, reloc'd below.
        bytes[entry_byte + ENTRY_OFF_PC_OFFSET as usize
            ..entry_byte + ENTRY_OFF_PC_OFFSET as usize + 4]
            .copy_from_slice(&pc_offset.to_le_bytes());
        bytes[entry_byte + ENTRY_OFF_FRAME_BYTES as usize
            ..entry_byte + ENTRY_OFF_FRAME_BYTES as usize + 4]
            .copy_from_slice(&frame_bytes.to_le_bytes());
        bytes[entry_byte + ENTRY_OFF_REF_COUNT as usize
            ..entry_byte + ENTRY_OFF_REF_COUNT as usize + 4]
            .copy_from_slice(&(refs.len() as u32).to_le_bytes());
        // ref_offsets ptr (offset +24): zeroed, reloc'd below.

        reloc_recs.push((entry_byte as u32, *func_id, refs_cursor as u32));

        for r in refs {
            bytes[refs_cursor..refs_cursor + 4].copy_from_slice(&r.to_le_bytes());
            refs_cursor += 4;
        }
    }

    let data_id = module
        .declare_data("corvid_stack_maps", Linkage::Export, false, false)
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare corvid_stack_maps: {e}"),
                Span::new(0, 0),
            )
        })?;
    let mut desc = DataDescription::new();
    desc.set_align(8);
    desc.define(bytes.into_boxed_slice());

    // Apply relocations: per-entry fn_start (to the function symbol)
    // and ref_offsets pointer (self-data-relocation to an offset
    // within the same `corvid_stack_maps` symbol).
    let self_gv = module.declare_data_in_data(data_id, &mut desc);
    for (entry_byte, func_id, refs_pool_off) in reloc_recs {
        let func_ref = module.declare_func_in_data(func_id, &mut desc);
        desc.write_function_addr(entry_byte + ENTRY_OFF_FN_START, func_ref);
        desc.write_data_addr(
            entry_byte + ENTRY_OFF_REFS_PTR,
            self_gv,
            refs_pool_off as i64,
        );
    }

    module.define_data(data_id, &desc).map_err(|e| {
        CodegenError::cranelift(
            format!("define corvid_stack_maps: {e}"),
            Span::new(0, 0),
        )
    })?;
    Ok(())
}

/// Generate and define `corvid_destroy_<TypeName>(payload)` for a
/// struct type that has at least one refcounted field. The destructor
/// loads each refcounted field at its compile-time offset and calls
/// `corvid_release` on it. `corvid_release` then frees the struct's
/// own allocation after the destructor returns.
pub(super) fn define_struct_destructor(
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
        module.declarations().get_function_decl(func_id).signature.clone(),
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
        let release_ref =
            module.declare_func_in_func(runtime.release, builder.func);
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

/// Emit `corvid_trace_<TypeName>(payload, marker, ctx)` for
/// a refcounted struct type. Mirrors `define_struct_destructor` but
/// dispatches through an indirect marker function pointer on each
/// refcounted field instead of releasing it.
///
/// Trace fns are emitted for every refcounted struct — including
/// structs with zero refcounted fields — so the future (17d) mark
/// collector can dispatch uniformly without a per-object NULL check.
/// The linker folds duplicate empty bodies, so the cost is ~zero.
///
/// Marker signature: `fn(obj: i64, ctx: i64) -> ()`. Context-passing
/// (rather than stateless) so 17d's collector can thread a worklist
/// pointer through the walk without TLS or globals.
pub(super) fn define_struct_trace(
    module: &mut ObjectModule,
    ty: &corvid_ir::IrType,
    runtime: &RuntimeFuncs,
) -> Result<FuncId, CodegenError> {
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(I64)); // payload
    sig.params.push(AbiParam::new(I64)); // marker fn ptr
    sig.params.push(AbiParam::new(I64)); // ctx

    let symbol = format!("corvid_trace_{}", ty.name);
    let func_id = module
        .declare_function(&symbol, Linkage::Local, &sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare trace `{symbol}`: {e}"), ty.span)
        })?;

    let mut ctx = Context::new();
    ctx.func = Function::with_name_signature(
        UserFuncName::user(0, func_id.as_u32()),
        module.declarations().get_function_decl(func_id).signature.clone(),
    );
    let mut bctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut bctx);

        // Marker signature: fn(obj: i64, ctx: i64) -> ()
        let mut marker_sig = Signature::new(module.isa().default_call_conv());
        marker_sig.params.push(AbiParam::new(I64));
        marker_sig.params.push(AbiParam::new(I64));
        let marker_sigref = builder.import_signature(marker_sig);

        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let payload = builder.block_params(entry)[0];
        let marker = builder.block_params(entry)[1];
        let marker_ctx = builder.block_params(entry)[2];

        for (i, field) in ty.fields.iter().enumerate() {
            if is_refcounted_type(&field.ty) {
                let offset = (i as i32) * STRUCT_FIELD_SLOT_BYTES;
                let v = builder.ins().load(
                    I64,
                    cranelift_codegen::ir::MemFlags::trusted(),
                    payload,
                    offset,
                );
                builder.ins().call_indirect(marker_sigref, marker, &[v, marker_ctx]);
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
        &format!("trace `{symbol}`"),
    )?;
    Ok(func_id)
}

/// On-disk typeinfo block layout. Must match exactly the
/// `corvid_typeinfo` struct in `crates/corvid-runtime/runtime/alloc.c`.
///
/// ```text
/// offset  0: u32 size               (payload size hint; 0 for variable)
/// offset  4: u32 flags              (CORVID_TI_* bits)
/// offset  8: fn_ptr destroy_fn      (8B — NULL if no refcounted children)
/// offset 16: fn_ptr trace_fn        (8B)
/// offset 24: fn_ptr weak_fn         (8B — NULL in 17a, reserved for 17g)
/// offset 32: data_ptr elem_typeinfo (8B — NULL for non-lists)
/// offset 40: data_ptr name          (8B — NULL in 17a; 17d will fill for dump_graph)
/// total:     48 bytes, 8-byte aligned
/// ```
const TYPEINFO_BYTES: usize = 48;
const TYPEINFO_OFF_SIZE: u32 = 0;
const TYPEINFO_OFF_FLAGS: u32 = 4;
const TYPEINFO_OFF_DESTROY_FN: u32 = 8;
const TYPEINFO_OFF_TRACE_FN: u32 = 16;
#[allow(dead_code)]
const TYPEINFO_OFF_WEAK_FN: u32 = 24;
const TYPEINFO_OFF_ELEM_TYPEINFO: u32 = 32;
#[allow(dead_code)]
const TYPEINFO_OFF_NAME: u32 = 40;

/// Typeinfo flags — must match `CORVID_TI_*` in alloc.c. Bits beyond
/// IS_LIST are reserved for the 17b-prime effect-typed memory model
/// (region inference + Perceus linearity + in-place reuse); defining
/// them now locks the type-info layout so future slices don't force
/// another migration.
#[allow(dead_code)]
const TYPEINFO_FLAG_CYCLIC_CAPABLE: u32 = 0x01;
#[allow(dead_code)]
const TYPEINFO_FLAG_HAS_WEAK_REFS: u32 = 0x02;
const TYPEINFO_FLAG_IS_LIST: u32 = 0x04;
#[allow(dead_code)]
const TYPEINFO_FLAG_LINEAR_CAPABLE: u32 = 0x08;
#[allow(dead_code)]
const TYPEINFO_FLAG_REGION_ALLOCATABLE: u32 = 0x10;
#[allow(dead_code)]
const TYPEINFO_FLAG_REUSE_SHAPE_HINT: u32 = 0x20;

pub(super) const RESULT_PAYLOAD_BYTES: i64 = 16;
pub(super) const RESULT_TAG_OFFSET: i32 = 0;
pub(super) const RESULT_PAYLOAD_OFFSET: i32 = 8;
pub(super) const RESULT_TAG_OK: i64 = 0;
pub(super) const RESULT_TAG_ERR: i64 = 1;
pub(super) const OPTION_PAYLOAD_BYTES: i64 = 8;
pub(super) const OPTION_PAYLOAD_OFFSET: i32 = 0;

/// Emit `corvid_typeinfo_<TypeName>` as a .rodata data symbol with
/// function-pointer relocations to the type's destroy_fn (if any)
/// and trace_fn. Returns the DataId so allocations can reference it.
pub(super) fn emit_struct_typeinfo(
    module: &mut ObjectModule,
    ty: &corvid_ir::IrType,
    destroy_fn: Option<FuncId>,
    trace_fn: FuncId,
    runtime: &RuntimeFuncs,
) -> Result<cranelift_module::DataId, CodegenError> {
    let symbol = format!("corvid_typeinfo_{}", ty.name);
    let data_id = module
        .declare_data(&symbol, Linkage::Local, false, false)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare typeinfo `{symbol}`: {e}"), ty.span)
        })?;

    let mut desc = DataDescription::new();
    desc.set_align(8);
    let mut bytes = vec![0u8; TYPEINFO_BYTES];

    // size: 8 bytes per refcounted field slot (payload is N*8).
    // Matches struct_payload_bytes().
    let payload_size = (ty.fields.len() as u32) * (STRUCT_FIELD_SLOT_BYTES as u32);
    bytes[TYPEINFO_OFF_SIZE as usize..(TYPEINFO_OFF_SIZE + 4) as usize]
        .copy_from_slice(&payload_size.to_le_bytes());

    // Struct payloads can be weak targets on the native heap, so we
    // install HAS_WEAK_REFS from 17g even if the struct itself does
    // not contain weak fields.
    let flags: u32 = TYPEINFO_FLAG_HAS_WEAK_REFS;
    bytes[TYPEINFO_OFF_FLAGS as usize..(TYPEINFO_OFF_FLAGS + 4) as usize]
        .copy_from_slice(&flags.to_le_bytes());

    desc.define(bytes.into_boxed_slice());

    // Function-pointer relocations. destroy_fn stays NULL (all-zero
    // bytes already written) if the struct has no refcounted fields.
    if let Some(dtor) = destroy_fn {
        let dtor_ref = module.declare_func_in_data(dtor, &mut desc);
        desc.write_function_addr(TYPEINFO_OFF_DESTROY_FN, dtor_ref);
    }
    let trace_ref = module.declare_func_in_data(trace_fn, &mut desc);
    desc.write_function_addr(TYPEINFO_OFF_TRACE_FN, trace_ref);
    let weak_ref = module.declare_func_in_data(runtime.weak_clear_self, &mut desc);
    desc.write_function_addr(TYPEINFO_OFF_WEAK_FN, weak_ref);

    module
        .define_data(data_id, &desc)
        .map_err(|e| {
            CodegenError::cranelift(format!("define typeinfo `{symbol}`: {e}"), ty.span)
        })?;
    Ok(data_id)
}

pub(super) fn define_result_destructor(
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
            CodegenError::cranelift(format!("declare destructor `{symbol}`: {e}"), Span::new(0, 0))
        })?;

    let mut ctx = Context::new();
    ctx.func = Function::with_name_signature(
        UserFuncName::user(0, func_id.as_u32()),
        module.declarations().get_function_decl(func_id).signature.clone(),
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

pub(super) fn option_uses_wrapper(option_ty: &Type) -> bool {
    match option_ty {
        Type::Option(inner) => is_native_wide_option_type(option_ty) || matches!(&**inner, Type::Option(_)),
        _ => false,
    }
}

pub(super) fn define_result_trace(
    module: &mut ObjectModule,
    result_ty: &Type,
    ok_ty: &Type,
    err_ty: &Type,
    runtime: &RuntimeFuncs,
) -> Result<FuncId, CodegenError> {
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(I64));
    sig.params.push(AbiParam::new(I64));
    sig.params.push(AbiParam::new(I64));

    let symbol = format!("corvid_trace_{}", mangle_type_name(result_ty));
    let func_id = module
        .declare_function(&symbol, Linkage::Local, &sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare trace `{symbol}`: {e}"), Span::new(0, 0))
        })?;

    let mut ctx = Context::new();
    ctx.func = Function::with_name_signature(
        UserFuncName::user(0, func_id.as_u32()),
        module.declarations().get_function_decl(func_id).signature.clone(),
    );
    let mut bctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut bctx);

        let mut marker_sig = Signature::new(module.isa().default_call_conv());
        marker_sig.params.push(AbiParam::new(I64));
        marker_sig.params.push(AbiParam::new(I64));
        let marker_sigref = builder.import_signature(marker_sig);

        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);

        let payload = builder.block_params(entry)[0];
        let marker = builder.block_params(entry)[1];
        let marker_ctx = builder.block_params(entry)[2];
        let tag = builder.ins().load(
            I64,
            cranelift_codegen::ir::MemFlags::trusted(),
            payload,
            RESULT_TAG_OFFSET,
        );

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
                builder.ins().call_indirect(marker_sigref, marker, &[v, marker_ctx]);
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
                builder.ins().call_indirect(marker_sigref, marker, &[v, marker_ctx]);
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
        &format!("trace `{symbol}`"),
    )?;
    Ok(func_id)
}

pub(super) fn emit_result_typeinfo(
    module: &mut ObjectModule,
    result_ty: &Type,
    destroy_fn: Option<FuncId>,
    trace_fn: FuncId,
    runtime: &RuntimeFuncs,
) -> Result<cranelift_module::DataId, CodegenError> {
    let symbol = format!("corvid_typeinfo_{}", mangle_type_name(result_ty));
    let data_id = module
        .declare_data(&symbol, Linkage::Local, false, false)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare typeinfo `{symbol}`: {e}"), Span::new(0, 0))
        })?;

    let mut desc = DataDescription::new();
    desc.set_align(8);
    let mut bytes = vec![0u8; TYPEINFO_BYTES];
    bytes[TYPEINFO_OFF_SIZE as usize..(TYPEINFO_OFF_SIZE + 4) as usize]
        .copy_from_slice(&(RESULT_PAYLOAD_BYTES as u32).to_le_bytes());

    let flags: u32 = TYPEINFO_FLAG_HAS_WEAK_REFS;
    bytes[TYPEINFO_OFF_FLAGS as usize..(TYPEINFO_OFF_FLAGS + 4) as usize]
        .copy_from_slice(&flags.to_le_bytes());
    desc.define(bytes.into_boxed_slice());

    if let Some(dtor) = destroy_fn {
        let dtor_ref = module.declare_func_in_data(dtor, &mut desc);
        desc.write_function_addr(TYPEINFO_OFF_DESTROY_FN, dtor_ref);
    }
    let trace_ref = module.declare_func_in_data(trace_fn, &mut desc);
    desc.write_function_addr(TYPEINFO_OFF_TRACE_FN, trace_ref);
    let weak_ref = module.declare_func_in_data(runtime.weak_clear_self, &mut desc);
    desc.write_function_addr(TYPEINFO_OFF_WEAK_FN, weak_ref);

    module
        .define_data(data_id, &desc)
        .map_err(|e| {
            CodegenError::cranelift(format!("define typeinfo `{symbol}`: {e}"), Span::new(0, 0))
        })?;
    Ok(data_id)
}

pub(super) fn define_option_trace(
    module: &mut ObjectModule,
    option_ty: &Type,
    payload_ty: &Type,
    runtime: &RuntimeFuncs,
) -> Result<FuncId, CodegenError> {
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(I64)); // payload wrapper ptr
    sig.params.push(AbiParam::new(I64)); // marker fn ptr
    sig.params.push(AbiParam::new(I64)); // ctx

    let symbol = format!("corvid_trace_{}", mangle_type_name(option_ty));
    let func_id = module
        .declare_function(&symbol, Linkage::Local, &sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare option trace `{symbol}`: {e}"), Span::new(0, 0))
        })?;

    let mut ctx = Context::new();
    ctx.func = Function::with_name_signature(
        UserFuncName::user(0, func_id.as_u32()),
        module.declarations().get_function_decl(func_id).signature.clone(),
    );
    let mut bctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut bctx);

        let mut marker_sig = Signature::new(module.isa().default_call_conv());
        marker_sig.params.push(AbiParam::new(I64));
        marker_sig.params.push(AbiParam::new(I64));
        let marker_sigref = builder.import_signature(marker_sig);

        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let payload = builder.block_params(entry)[0];
        let marker = builder.block_params(entry)[1];
        let marker_ctx = builder.block_params(entry)[2];

        if is_refcounted_type(payload_ty) {
            let payload_val = builder.ins().load(
                I64,
                cranelift_codegen::ir::MemFlags::trusted(),
                payload,
                OPTION_PAYLOAD_OFFSET,
            );
            builder
                .ins()
                .call_indirect(marker_sigref, marker, &[payload_val, marker_ctx]);
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
        &format!("option trace `{symbol}`"),
    )?;
    Ok(func_id)
}

pub(super) fn define_option_destructor(
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
            CodegenError::cranelift(format!("declare option destructor `{symbol}`: {e}"), Span::new(0, 0))
        })?;

    let mut ctx = Context::new();
    ctx.func = Function::with_name_signature(
        UserFuncName::user(0, func_id.as_u32()),
        module.declarations().get_function_decl(func_id).signature.clone(),
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

pub(super) fn emit_option_typeinfo(
    module: &mut ObjectModule,
    option_ty: &Type,
    destroy_fn: Option<FuncId>,
    trace_fn: FuncId,
    runtime: &RuntimeFuncs,
) -> Result<cranelift_module::DataId, CodegenError> {
    let symbol = format!("corvid_typeinfo_{}", mangle_type_name(option_ty));
    let data_id = module
        .declare_data(&symbol, Linkage::Local, false, false)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare option typeinfo `{symbol}`: {e}"), Span::new(0, 0))
        })?;

    let mut desc = DataDescription::new();
    desc.set_align(8);
    let mut bytes = vec![0u8; TYPEINFO_BYTES];
    bytes[TYPEINFO_OFF_SIZE as usize..(TYPEINFO_OFF_SIZE + 4) as usize]
        .copy_from_slice(&(OPTION_PAYLOAD_BYTES as u32).to_le_bytes());

    let flags: u32 = TYPEINFO_FLAG_HAS_WEAK_REFS;
    bytes[TYPEINFO_OFF_FLAGS as usize..(TYPEINFO_OFF_FLAGS + 4) as usize]
        .copy_from_slice(&flags.to_le_bytes());
    desc.define(bytes.into_boxed_slice());

    if let Some(dtor) = destroy_fn {
        let dtor_ref = module.declare_func_in_data(dtor, &mut desc);
        desc.write_function_addr(TYPEINFO_OFF_DESTROY_FN, dtor_ref);
    }
    let trace_ref = module.declare_func_in_data(trace_fn, &mut desc);
    desc.write_function_addr(TYPEINFO_OFF_TRACE_FN, trace_ref);
    let weak_ref = module.declare_func_in_data(runtime.weak_clear_self, &mut desc);
    desc.write_function_addr(TYPEINFO_OFF_WEAK_FN, weak_ref);

    module
        .define_data(data_id, &desc)
        .map_err(|e| {
            CodegenError::cranelift(format!("define option typeinfo `{symbol}`: {e}"), Span::new(0, 0))
        })?;
    Ok(data_id)
}

/// Emit `corvid_typeinfo_List_<elem>` for a concrete list
/// element type. Uses the runtime's shared `corvid_destroy_list` and
/// `corvid_trace_list` rather than per-type functions; the element
/// layout info lives entirely in `elem_typeinfo`.
///
/// `elem_typeinfo_data_id` is None for primitive-element lists
/// (List<Int>, List<Bool>, List<Float>); the runtime tracer checks
/// NULL and no-ops. Also sets destroy_fn=NULL for such lists —
/// `corvid_release` skips dispatch.
pub(super) fn emit_list_typeinfo(
    module: &mut ObjectModule,
    elem_ty: &Type,
    elem_typeinfo_data_id: Option<cranelift_module::DataId>,
    runtime: &RuntimeFuncs,
) -> Result<cranelift_module::DataId, CodegenError> {
    let symbol = format!("corvid_typeinfo_List_{}", mangle_type_name(elem_ty));
    let data_id = module
        .declare_data(&symbol, Linkage::Local, false, false)
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare list typeinfo `{symbol}`: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut desc = DataDescription::new();
    desc.set_align(8);
    let mut bytes = vec![0u8; TYPEINFO_BYTES];

    // size: 0 (variable-length)
    // flags: IS_LIST
    let flags: u32 = TYPEINFO_FLAG_IS_LIST | TYPEINFO_FLAG_HAS_WEAK_REFS;
    bytes[TYPEINFO_OFF_FLAGS as usize..(TYPEINFO_OFF_FLAGS + 4) as usize]
        .copy_from_slice(&flags.to_le_bytes());

    desc.define(bytes.into_boxed_slice());

    // destroy_fn: corvid_destroy_list, but only for refcounted-element
    // lists. Primitive-element lists leave it NULL so corvid_release
    // skips dispatch (matches the pre-17a plain-alloc behavior).
    if elem_typeinfo_data_id.is_some() {
        let dtor_ref = module.declare_func_in_data(runtime.list_destroy, &mut desc);
        desc.write_function_addr(TYPEINFO_OFF_DESTROY_FN, dtor_ref);
    }

    // trace_fn: corvid_trace_list — always set. No-ops on primitive-
    // element lists because the fn checks elem_typeinfo at runtime.
    let trace_ref = module.declare_func_in_data(runtime.list_trace, &mut desc);
    desc.write_function_addr(TYPEINFO_OFF_TRACE_FN, trace_ref);
    let weak_ref = module.declare_func_in_data(runtime.weak_clear_self, &mut desc);
    desc.write_function_addr(TYPEINFO_OFF_WEAK_FN, weak_ref);

    // elem_typeinfo: for refcounted-element lists, point at the
    // element's typeinfo (String built-in, struct typeinfo, or nested
    // list typeinfo). For primitive elements, stays NULL.
    if let Some(elem_ti_id) = elem_typeinfo_data_id {
        let elem_gv = module.declare_data_in_data(elem_ti_id, &mut desc);
        desc.write_data_addr(TYPEINFO_OFF_ELEM_TYPEINFO, elem_gv, 0);
    }

    module
        .define_data(data_id, &desc)
        .map_err(|e| {
            CodegenError::cranelift(
                format!("define list typeinfo `{symbol}`: {e}"),
                Span::new(0, 0),
            )
        })?;
    Ok(data_id)
}

pub(super) fn typeinfo_data_for_refcounted_payload(
    ty: &Type,
    runtime: &RuntimeFuncs,
) -> Option<cranelift_module::DataId> {
    match ty {
        Type::String => Some(runtime.string_typeinfo),
        Type::Struct(def_id) => runtime.struct_typeinfos.get(def_id).copied(),
        Type::List(inner_inner) => runtime.list_typeinfos.get(&(**inner_inner)).copied(),
        Type::Result(_, _) => runtime.result_typeinfos.get(ty).copied(),
        Type::Weak(_, _) => Some(runtime.weak_box_typeinfo),
        Type::Option(inner) => {
            if is_native_wide_option_type(ty) {
                runtime.option_typeinfos.get(ty).copied()
            } else {
                typeinfo_data_for_refcounted_payload(inner, runtime)
            }
        }
        _ => None,
    }
}

/// Stable, link-safe string from a Corvid `Type` for use in typeinfo
/// symbol names. `List<List<String>>` → `List_List_String`, etc.
pub(super) fn mangle_type_name(ty: &Type) -> String {
    match ty {
        Type::Int => "Int".into(),
        Type::Float => "Float".into(),
        Type::Bool => "Bool".into(),
        Type::String => "String".into(),
        Type::Nothing => "Nothing".into(),
        Type::List(inner) => format!("List_{}", mangle_type_name(inner)),
        Type::Stream(inner) => format!("Stream_{}", mangle_type_name(inner)),
        Type::Struct(def_id) => format!("Struct_{}", def_id.0),
        Type::Function { .. } => "Function".into(),
        // Result<T,E> and Option<T> are compiler-known
        // tagged unions. Their typeinfo emission (and full native
        // codegen) lands in 18d. For 17c we just need the mangler
        // to terminate; the resulting names won't be used at runtime
        // because programs touching these types fail at the
        // `lower_expr` codegen step below before reaching emission.
        Type::Result(ok, err) => format!(
            "Result_{}_{}",
            mangle_type_name(ok),
            mangle_type_name(err)
        ),
        Type::Option(inner) => format!("Option_{}", mangle_type_name(inner)),
        Type::Grounded(inner) => format!("Grounded_{}", mangle_type_name(inner)),
        Type::Weak(inner, effects) => {
            if effects.is_any() {
                format!("Weak_{}", mangle_type_name(inner))
            } else {
                let suffix: Vec<&'static str> = effects
                    .effects()
                    .into_iter()
                    .map(|effect| match effect {
                        corvid_ast::WeakEffect::ToolCall => "tool_call",
                        corvid_ast::WeakEffect::Llm => "llm",
                        corvid_ast::WeakEffect::Approve => "approve",
                    })
                    .collect();
                format!("Weak_{}_{}", mangle_type_name(inner), suffix.join("_"))
            }
        }
        Type::TraceId => "TraceId".into(),
        Type::Unknown => "Unknown".into(),
    }
}

/// Walk every `Type::List(_)` the IR mentions (agent sigs,
/// struct fields, tool/prompt sigs, expression types) and produce the
/// set of unique list element types in a dependency-friendly order:
/// element types come before lists that contain them.
///
/// The returned `Vec<Type>` holds the *element* type of each list
/// (not the `List<T>` type itself). Emission iterates this vec
/// creating one `corvid_typeinfo_List_<elem>` per entry.
pub(super) fn collect_list_element_types(ir: &IrFile) -> Vec<Type> {
    use std::collections::BTreeSet;
    let mut seen: BTreeSet<Type> = BTreeSet::new();
    let mut order: Vec<Type> = Vec::new();

    fn visit(ty: &Type, seen: &mut BTreeSet<Type>, order: &mut Vec<Type>) {
        match ty {
            Type::List(inner) => {
                // Recurse first so inner list types get their
                // typeinfo emitted BEFORE the outer list references
                // them via elem_typeinfo relocation.
                visit(inner, seen, order);
                let elem = (**inner).clone();
                if seen.insert(elem.clone()) {
                    order.push(elem);
                }
            }
            Type::Result(ok, err) => {
                visit(ok, seen, order);
                visit(err, seen, order);
            }
            Type::Option(inner) | Type::Weak(inner, _) => visit(inner, seen, order),
            Type::Function { params, ret, .. } => {
                for param in params {
                    visit(param, seen, order);
                }
                visit(ret, seen, order);
            }
            _ => {}
        }
    }

    for agent in &ir.agents {
        for param in &agent.params {
            visit(&param.ty, &mut seen, &mut order);
        }
        visit(&agent.return_ty, &mut seen, &mut order);
        visit_block_types(&agent.body, &mut seen, &mut order, &visit);
    }
    for ty in &ir.types {
        for field in &ty.fields {
            visit(&field.ty, &mut seen, &mut order);
        }
    }
    for tool in &ir.tools {
        for param in &tool.params {
            visit(&param.ty, &mut seen, &mut order);
        }
        visit(&tool.return_ty, &mut seen, &mut order);
    }
    for prompt in &ir.prompts {
        for param in &prompt.params {
            visit(&param.ty, &mut seen, &mut order);
        }
        visit(&prompt.return_ty, &mut seen, &mut order);
    }

    order
}

pub(super) fn collect_result_types(ir: &IrFile) -> Vec<Type> {
    use std::collections::BTreeSet;
    let mut seen: BTreeSet<Type> = BTreeSet::new();
    let mut order: Vec<Type> = Vec::new();

    fn visit(ty: &Type, seen: &mut BTreeSet<Type>, order: &mut Vec<Type>) {
        match ty {
            Type::Result(ok, err) => {
                visit(ok, seen, order);
                visit(err, seen, order);
                if seen.insert(ty.clone()) {
                    order.push(ty.clone());
                }
            }
            Type::List(inner) | Type::Option(inner) | Type::Weak(inner, _) => {
                visit(inner, seen, order);
            }
            Type::Function { params, ret, .. } => {
                for param in params {
                    visit(param, seen, order);
                }
                visit(ret, seen, order);
            }
            _ => {}
        }
    }

    for agent in &ir.agents {
        for param in &agent.params {
            visit(&param.ty, &mut seen, &mut order);
        }
        visit(&agent.return_ty, &mut seen, &mut order);
        visit_block_types(&agent.body, &mut seen, &mut order, &visit);
    }
    for ty in &ir.types {
        for field in &ty.fields {
            visit(&field.ty, &mut seen, &mut order);
        }
    }
    for tool in &ir.tools {
        for param in &tool.params {
            visit(&param.ty, &mut seen, &mut order);
        }
        visit(&tool.return_ty, &mut seen, &mut order);
    }
    for prompt in &ir.prompts {
        for param in &prompt.params {
            visit(&param.ty, &mut seen, &mut order);
        }
        visit(&prompt.return_ty, &mut seen, &mut order);
    }

    order
}

pub(super) fn collect_option_types(ir: &IrFile) -> Vec<Type> {
    use std::collections::BTreeSet;
    let mut seen: BTreeSet<Type> = BTreeSet::new();
    let mut order: Vec<Type> = Vec::new();

    fn visit(ty: &Type, seen: &mut BTreeSet<Type>, order: &mut Vec<Type>) {
        match ty {
            Type::Option(inner) => {
                visit(inner, seen, order);
                if option_uses_wrapper(ty) && seen.insert(ty.clone()) {
                    order.push(ty.clone());
                }
            }
            Type::Result(ok, err) => {
                visit(ok, seen, order);
                visit(err, seen, order);
            }
            Type::List(inner) | Type::Weak(inner, _) => visit(inner, seen, order),
            Type::Function { params, ret, .. } => {
                for param in params {
                    visit(param, seen, order);
                }
                visit(ret, seen, order);
            }
            _ => {}
        }
    }

    for agent in &ir.agents {
        for param in &agent.params {
            visit(&param.ty, &mut seen, &mut order);
        }
        visit(&agent.return_ty, &mut seen, &mut order);
        visit_block_types(&agent.body, &mut seen, &mut order, &visit);
    }
    for ty in &ir.types {
        for field in &ty.fields {
            visit(&field.ty, &mut seen, &mut order);
        }
    }
    for tool in &ir.tools {
        for param in &tool.params {
            visit(&param.ty, &mut seen, &mut order);
        }
        visit(&tool.return_ty, &mut seen, &mut order);
    }
    for prompt in &ir.prompts {
        for param in &prompt.params {
            visit(&param.ty, &mut seen, &mut order);
        }
        visit(&prompt.return_ty, &mut seen, &mut order);
    }

    order
}

/// Walk an `IrBlock` and visit every expression's `ty` through the
/// caller's closure. Catches list literals and other list-producing
/// expressions that don't surface in signatures.
fn visit_block_types(
    block: &IrBlock,
    seen: &mut std::collections::BTreeSet<Type>,
    order: &mut Vec<Type>,
    visit: &dyn Fn(&Type, &mut std::collections::BTreeSet<Type>, &mut Vec<Type>),
) {
    for stmt in &block.stmts {
        match stmt {
            IrStmt::Let { value, ty, .. } => {
                visit(ty, seen, order);
                visit_expr_types(value, seen, order, visit);
            }
            IrStmt::Expr { expr, .. } => visit_expr_types(expr, seen, order, visit),
            IrStmt::Yield { value, .. } => visit_expr_types(value, seen, order, visit),
            IrStmt::Return { value: Some(e), .. } => visit_expr_types(e, seen, order, visit),
            IrStmt::Return { value: None, .. } => {}
            IrStmt::If { cond, then_block, else_block, .. } => {
                visit_expr_types(cond, seen, order, visit);
                visit_block_types(then_block, seen, order, visit);
                if let Some(eb) = else_block {
                    visit_block_types(eb, seen, order, visit);
                }
            }
            IrStmt::For { iter, body, .. } => {
                visit_expr_types(iter, seen, order, visit);
                visit_block_types(body, seen, order, visit);
            }
            IrStmt::Approve { args, .. } => {
                for a in args {
                    visit_expr_types(a, seen, order, visit);
                }
            }
            IrStmt::Break { .. } | IrStmt::Continue { .. } | IrStmt::Pass { .. } => {}
            IrStmt::Dup { .. } | IrStmt::Drop { .. } => {}
        }
    }
}

fn visit_expr_types(
    e: &IrExpr,
    seen: &mut std::collections::BTreeSet<Type>,
    order: &mut Vec<Type>,
    visit: &dyn Fn(&Type, &mut std::collections::BTreeSet<Type>, &mut Vec<Type>),
) {
    visit(&e.ty, seen, order);
    match &e.kind {
        IrExprKind::Literal(_) | IrExprKind::Local { .. } | IrExprKind::Decl { .. } => {}
        IrExprKind::BinOp { left, right, .. } => {
            visit_expr_types(left, seen, order, visit);
            visit_expr_types(right, seen, order, visit);
        }
        IrExprKind::UnOp { operand, .. } => {
            visit_expr_types(operand, seen, order, visit);
        }
        IrExprKind::Call { args, .. } => {
            for a in args {
                visit_expr_types(a, seen, order, visit);
            }
        }
        IrExprKind::FieldAccess { target, .. } => {
            visit_expr_types(target, seen, order, visit);
        }
        IrExprKind::Index { target, index } => {
            visit_expr_types(target, seen, order, visit);
            visit_expr_types(index, seen, order, visit);
        }
        IrExprKind::List { items } => {
            for el in items {
                visit_expr_types(el, seen, order, visit);
            }
        }
        // Result/Option/?/try-retry IR variants. The
        // visit_expr_types pass collects list-element types for
        // typeinfo emission. Result/Option don't appear in list-
        // element positions in 17c (their codegen lands in 18d),
        // but we still recurse into their sub-expressions so any
        // List<T> *nested* inside them is still seen.
        IrExprKind::WeakNew { strong: inner }
        | IrExprKind::WeakUpgrade { weak: inner }
        | IrExprKind::ResultOk { inner }
        | IrExprKind::ResultErr { inner }
        | IrExprKind::OptionSome { inner }
        | IrExprKind::TryPropagate { inner } => {
            visit_expr_types(inner, seen, order, visit);
        }
        IrExprKind::OptionNone => {}
        IrExprKind::TryRetry { body, .. } => {
            visit_expr_types(body, seen, order, visit);
        }
        IrExprKind::Replay { trace, arms, else_body } => {
            visit_expr_types(trace, seen, order, visit);
            for arm in arms {
                visit_expr_types(&arm.body, seen, order, visit);
            }
            visit_expr_types(else_body, seen, order, visit);
        }
    }
}

/// Helper: emit `corvid_retain(value)` if the value is refcounted
/// (i.e., non-immortal at runtime). Caller decides whether the value
/// needs ownership at this point — the helper just emits the call.
pub(super) fn emit_retain(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    v: ClValue,
) {
    let callee = module.declare_func_in_func(runtime.retain, builder.func);
    builder.ins().call(callee, &[v]);
}

/// Helper: emit `corvid_release(value)`.
pub(super) fn emit_release(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    v: ClValue,
) {
    let callee = module.declare_func_in_func(runtime.release, builder.func);
    builder.ins().call(callee, &[v]);
}

/// Whether a Corvid value of this type lives behind a refcounted heap
/// allocation. When true, the codegen tracks ownership: `retain` on
/// bind, `release` on scope exit, etc.
///
/// Today `String` is refcounted. Future work extends this to `Struct`
/// (12f), `List` (12g) — both will return true here.
pub(super) fn is_refcounted_type(ty: &Type) -> bool {
    match ty {
        Type::String | Type::Struct(_) | Type::List(_) | Type::Weak(_, _) | Type::Result(_, _) => true,
        Type::Option(inner) => is_native_wide_option_type(ty) || is_refcounted_type(inner),
        _ => false,
    }
}

pub(super) fn is_native_value_type(ty: &Type) -> bool {
    match ty {
        Type::Int | Type::Bool | Type::Float | Type::String => true,
        Type::Struct(_) | Type::List(_) | Type::Weak(_, _) => true,
        Type::Option(_) => is_native_option_type(ty),
        Type::Result(ok, err) => is_native_value_type(ok) && is_native_value_type(err),
        Type::Grounded(inner) => is_native_value_type(inner),
        // TraceId is a string-backed opaque handle at runtime;
        // treat it as a value type for native emission purposes.
        Type::TraceId => true,
        Type::Nothing | Type::Function { .. } | Type::Stream(_) | Type::Unknown => false,
    }
}

pub(super) fn is_native_wide_option_type(ty: &Type) -> bool {
    matches!(ty, Type::Option(inner) if matches!(&**inner, Type::Int | Type::Bool | Type::Float))
}

pub(super) fn is_native_option_type(ty: &Type) -> bool {
    match ty {
        Type::Option(inner) => is_refcounted_type(inner) || is_native_wide_option_type(ty),
        _ => false,
    }
}

pub(super) fn is_native_option_expr_type(ty: &Type) -> bool {
    matches!(ty, Type::Option(inner) if matches!(**inner, Type::Unknown))
        || is_native_option_type(ty)
}

pub(super) fn is_native_result_type(ty: &Type) -> bool {
    matches!(ty, Type::Result(ok, err) if is_native_value_type(ok) && is_native_value_type(err))
}

// ---- runtime helper symbols ----
//
// The C runtime in `runtime/{alloc,strings}.c` exports these symbols.
// `lower_file` declares them once per module as `Linkage::Import`; each
// per-function lowering uses `module.declare_func_in_func` to get a
// FuncRef, then `builder.ins().call`.

/// `void corvid_retain(void* payload)` — atomic refcount increment.
pub(super) const RETAIN_SYMBOL: &str = "corvid_retain";

/// `void corvid_release(void* payload)` — atomic refcount decrement;
/// frees the underlying block when refcount hits zero.
pub(super) const RELEASE_SYMBOL: &str = "corvid_release";

/// `void* corvid_string_concat(void* a, void* b)` — allocates a fresh
/// String (refcount = 1) containing `a` followed by `b`.
pub(super) const STRING_CONCAT_SYMBOL: &str = "corvid_string_concat";

/// `long long corvid_string_eq(void* a, void* b)` — bytewise equality.
pub(super) const STRING_EQ_SYMBOL: &str = "corvid_string_eq";

/// `long long corvid_string_cmp(void* a, void* b)` — bytewise compare.
pub(super) const STRING_CMP_SYMBOL: &str = "corvid_string_cmp";

/// `void* corvid_alloc_typed(long long payload_bytes, const corvid_typeinfo* ti)`
/// — heap-allocate an N-byte payload behind a 16-byte typed header.
/// The typed allocator collapsed the old `corvid_alloc` + `corvid_alloc_with_destructor`
/// pair: every allocation now carries a typeinfo pointer, and
/// `corvid_release` dispatches through `typeinfo->destroy_fn` (NULL
/// = no refcounted children, equivalent to the old plain-alloc case).
pub(super) const ALLOC_TYPED_SYMBOL: &str = "corvid_alloc_typed";

/// `void corvid_destroy_list(void* payload)` — shared runtime
/// destructor installed in every refcounted-element list type's
/// typeinfo. Walks length at offset 0 and `corvid_release`s each
/// element. Primitive-element lists leave `destroy_fn` NULL.
pub(super) const LIST_DESTROY_SYMBOL: &str = "corvid_destroy_list";

/// `void corvid_trace_list(void*, void(*)(void*, void*), void*)` —
/// shared runtime tracer installed in every list type's typeinfo.
/// Reads its own typeinfo's `elem_typeinfo` to decide whether to
/// walk elements (NULL = primitive elements = no-op). Codegen
/// emits it for every list; the collector's mark walk invokes it.
pub(super) const LIST_TRACE_SYMBOL: &str = "corvid_trace_list";
pub(super) const WEAK_NEW_SYMBOL: &str = "corvid_weak_new";
pub(super) const WEAK_UPGRADE_SYMBOL: &str = "corvid_weak_upgrade";
pub(super) const WEAK_CLEAR_SELF_SYMBOL: &str = "corvid_weak_clear_self";
pub(super) const WEAK_BOX_TYPEINFO_SYMBOL: &str = "corvid_typeinfo_WeakBox";

/// Built-in `corvid_typeinfo_String` — the runtime provides this
/// symbol in `alloc.c`. Static string literals in `.rodata` and
/// runtime-internal String allocations both reference it so the
/// codegen doesn't have to emit a stray typeinfo per compilation
/// for string-less programs.
pub(super) const STRING_TYPEINFO_SYMBOL: &str = "corvid_typeinfo_String";

// Entry-agent helpers (argv decoding, result printing,
// arity reporting, atexit). Called from the codegen-emitted `main`.

pub(super) const ENTRY_INIT_SYMBOL: &str = "corvid_init";
pub(super) const ENTRY_ARITY_MISMATCH_SYMBOL: &str = "corvid_arity_mismatch";
pub(super) const PARSE_I64_SYMBOL: &str = "corvid_parse_i64";
pub(super) const PARSE_F64_SYMBOL: &str = "corvid_parse_f64";
pub(super) const PARSE_BOOL_SYMBOL: &str = "corvid_parse_bool";
pub(super) const STRING_FROM_CSTR_SYMBOL: &str = "corvid_string_from_cstr";
pub(super) const PRINT_I64_SYMBOL: &str = "corvid_print_i64";
pub(super) const PRINT_BOOL_SYMBOL: &str = "corvid_print_bool";
pub(super) const PRINT_F64_SYMBOL: &str = "corvid_print_f64";
pub(super) const PRINT_STRING_SYMBOL: &str = "corvid_print_string";
pub(super) const BENCH_SERVER_ENABLED_SYMBOL: &str = "corvid_bench_server_enabled";
pub(super) const BENCH_NEXT_TRIAL_SYMBOL: &str = "corvid_bench_next_trial";
pub(super) const BENCH_FINISH_TRIAL_SYMBOL: &str = "corvid_bench_finish_trial";

// Async tool dispatch bridge. Signature in Rust:
//   corvid_tool_call_sync_int(name_ptr: *const u8, name_len: usize) -> i64
// Returns i64::MIN on error (tool-not-found, tool-errored, non-integer
// return). The narrow bridge only supports the `() -> Int` tool
// signature; the typed bridge handles the general JSON arg + return
// marshalling.
pub(super) const TOOL_CALL_SYNC_INT_SYMBOL: &str = "corvid_tool_call_sync_int";
pub(super) const RUNTIME_IS_REPLAY_SYMBOL: &str = "corvid_runtime_is_replay";
pub(super) const REPLAY_TOOL_CALL_NOTHING_SYMBOL: &str = "corvid_replay_tool_call_nothing";
pub(super) const REPLAY_TOOL_CALL_INT_SYMBOL: &str = "corvid_replay_tool_call_int";
pub(super) const REPLAY_TOOL_CALL_BOOL_SYMBOL: &str = "corvid_replay_tool_call_bool";
pub(super) const REPLAY_TOOL_CALL_FLOAT_SYMBOL: &str = "corvid_replay_tool_call_float";
pub(super) const REPLAY_TOOL_CALL_STRING_SYMBOL: &str = "corvid_replay_tool_call_string";

// Scalar-to-String stringification helpers. Used by the
// Cranelift codegen for `IrCallKind::Prompt` lowering when a
// non-String argument is interpolated into a prompt template. Each
// returns a refcount-1 Corvid String the caller must release.
pub(super) const STRING_FROM_INT_SYMBOL: &str = "corvid_string_from_int";
pub(super) const STRING_FROM_BOOL_SYMBOL: &str = "corvid_string_from_bool";
pub(super) const STRING_FROM_FLOAT_SYMBOL: &str = "corvid_string_from_float";

// Typed prompt-dispatch bridges. One per return type;
// each takes 4 CorvidString args (prompt name, signature, rendered
// template, model) and returns the typed value. Built-in
// retry-with-validation + function-signature context — see the
// Rust-side implementations in `corvid-runtime::ffi_bridge`.
pub(super) const PROMPT_CALL_INT_SYMBOL: &str = "corvid_prompt_call_int";
pub(super) const PROMPT_CALL_BOOL_SYMBOL: &str = "corvid_prompt_call_bool";
pub(super) const PROMPT_CALL_FLOAT_SYMBOL: &str = "corvid_prompt_call_float";
pub(super) const PROMPT_CALL_STRING_SYMBOL: &str = "corvid_prompt_call_string";
pub(super) const APPROVE_SYNC_SYMBOL: &str = "corvid_approve_sync";
pub(super) const TRACE_RUN_STARTED_SYMBOL: &str = "corvid_trace_run_started";
pub(super) const TRACE_RUN_COMPLETED_INT_SYMBOL: &str = "corvid_trace_run_completed_int";
pub(super) const TRACE_RUN_COMPLETED_BOOL_SYMBOL: &str = "corvid_trace_run_completed_bool";
pub(super) const TRACE_RUN_COMPLETED_FLOAT_SYMBOL: &str = "corvid_trace_run_completed_float";
pub(super) const TRACE_RUN_COMPLETED_STRING_SYMBOL: &str = "corvid_trace_run_completed_string";
pub(super) const TRACE_TOOL_CALL_SYMBOL: &str = "corvid_trace_tool_call";
pub(super) const TRACE_TOOL_RESULT_NULL_SYMBOL: &str = "corvid_trace_tool_result_null";
pub(super) const TRACE_TOOL_RESULT_INT_SYMBOL: &str = "corvid_trace_tool_result_int";
pub(super) const TRACE_TOOL_RESULT_BOOL_SYMBOL: &str = "corvid_trace_tool_result_bool";
pub(super) const TRACE_TOOL_RESULT_FLOAT_SYMBOL: &str = "corvid_trace_tool_result_float";
pub(super) const TRACE_TOOL_RESULT_STRING_SYMBOL: &str = "corvid_trace_tool_result_string";

// Runtime bridge init/shutdown called from `corvid_init`
// at the start of codegen-emitted `main` when the program uses any
// tool/prompt/approve construct. Tool-free programs skip these
// calls to preserve startup benchmark numbers.
pub(super) const RUNTIME_INIT_SYMBOL: &str = "corvid_runtime_init";
pub(super) const RUNTIME_SHUTDOWN_SYMBOL: &str = "corvid_runtime_shutdown";
pub(super) const RUNTIME_EMBED_INIT_SYMBOL: &str = "corvid_runtime_embed_init_default";
pub(super) const SLEEP_MS_SYMBOL: &str = "corvid_sleep_ms";
pub(super) const STRING_INTO_CSTR_SYMBOL: &str = "corvid_string_into_cstr";

/// Per-struct payload uses fixed 8-byte field slots for simple offset
/// math. Tighter packing is a later optimization.
pub(super) const STRUCT_FIELD_SLOT_BYTES: i32 = 8;

/// Bytes per struct field when computing alloc size.
pub(super) fn struct_payload_bytes(n_fields: usize) -> i64 {
    (n_fields as i64) * (STRUCT_FIELD_SLOT_BYTES as i64)
}

/// Bundle of imported runtime helper FuncIds, declared once per module
/// in `lower_file` and threaded through every lowering function.
/// Replaces the previous bare `overflow_func_id: FuncId` parameter so
/// call sites get every helper in one place.
///
/// `literal_counter` is a `Cell` so recursive lowering paths can take
/// `&self` and still bump the counter for unique `.rodata` symbol names.
pub(super) struct RuntimeFuncs {
    pub overflow: FuncId,
    pub retain: FuncId,
    pub release: FuncId,
    pub string_concat: FuncId,
    pub string_eq: FuncId,
    pub string_cmp: FuncId,
    /// Single typed allocator replaces the older
    /// `alloc`/`alloc_with_destructor` pair. Signature:
    /// `(size: i64, typeinfo_ptr: i64) -> i64`.
    pub alloc_typed: FuncId,
    /// Shared runtime destructor installed in every
    /// refcounted-element list type's typeinfo. Replaces the
    /// pre-17a `list_destroy_refcounted`.
    pub list_destroy: FuncId,
    /// Shared runtime tracer installed in every list's
    /// typeinfo; the collector's mark walk will invoke it.
    pub list_trace: FuncId,
    pub weak_new: FuncId,
    pub weak_upgrade: FuncId,
    pub weak_clear_self: FuncId,
    /// Runtime-provided `corvid_typeinfo_String` data
    /// symbol. Imported so codegen can relocate its address into
    /// static string literals and List<String>'s elem_typeinfo slot.
    pub string_typeinfo: cranelift_module::DataId,
    pub weak_box_typeinfo: cranelift_module::DataId,
    // Entry helpers used by the codegen-emitted `main`.
    pub entry_init: FuncId,
    pub entry_arity_mismatch: FuncId,
    pub parse_i64: FuncId,
    pub parse_f64: FuncId,
    pub parse_bool: FuncId,
    pub string_from_cstr: FuncId,
    pub print_i64: FuncId,
    pub print_bool: FuncId,
    pub print_f64: FuncId,
    pub print_string: FuncId,
    pub bench_server_enabled: FuncId,
    pub bench_next_trial: FuncId,
    pub bench_finish_trial: FuncId,
    // Async tool bridge + runtime init/shutdown.
    pub tool_call_sync_int: FuncId,
    pub runtime_is_replay: FuncId,
    pub replay_tool_call_nothing: FuncId,
    pub replay_tool_call_int: FuncId,
    pub replay_tool_call_bool: FuncId,
    pub replay_tool_call_float: FuncId,
    pub replay_tool_call_string: FuncId,
    pub runtime_init: FuncId,
    pub runtime_shutdown: FuncId,
    pub runtime_embed_init: FuncId,
    pub sleep_ms: FuncId,
    pub string_into_cstr: FuncId,
    // Scalar->String helpers for prompt-template interpolation.
    pub string_from_int: FuncId,
    pub string_from_bool: FuncId,
    pub string_from_float: FuncId,
    pub approve_sync: FuncId,
    // Typed prompt bridges, one per return type.
    pub prompt_call_int: FuncId,
    pub prompt_call_bool: FuncId,
    pub prompt_call_float: FuncId,
    pub prompt_call_string: FuncId,
    pub trace_run_started: FuncId,
    pub trace_run_completed_int: FuncId,
    pub trace_run_completed_bool: FuncId,
    pub trace_run_completed_float: FuncId,
    pub trace_run_completed_string: FuncId,
    pub trace_tool_call: FuncId,
    pub trace_tool_result_null: FuncId,
    pub trace_tool_result_int: FuncId,
    pub trace_tool_result_bool: FuncId,
    pub trace_tool_result_float: FuncId,
    pub trace_tool_result_string: FuncId,
    pub literal_counter: std::cell::Cell<u64>,
    /// When true, codegen-level scattered
    /// `emit_retain` / `emit_release` sites are skipped because the
    /// dataflow-driven pass in `crate::dup_drop` has already inserted
    /// the equivalent `IrStmt::Dup` / `IrStmt::Drop` ops into the IR.
    /// Set from `CORVID_DUP_DROP_PASS` in `lower_file`. Default true
    /// (pass is active); set to false to fall back to pre-17b-1b.6c
    /// behavior for A/B debugging.
    pub dup_drop_enabled: bool,
    /// Per-struct-type destructors generated in `lower_file` for
    /// structs with at least one refcounted field. Missing entries
    /// mean the struct has no refcounted fields (typeinfo.destroy_fn
    /// stays NULL; corvid_release skips dispatch).
    pub struct_destructors: HashMap<DefId, FuncId>,
    /// Per-struct-type trace functions. Emitted for every
    /// refcounted struct type (including those with no refcounted
    /// fields — those trace fns are empty bodies, kept for uniform
    /// dispatch in the collector mark walk without a per-object NULL check).
    pub struct_traces: HashMap<DefId, FuncId>,
    /// Per-struct-type typeinfo data symbols. Every
    /// refcounted struct allocation references its block via
    /// `corvid_alloc_typed(size, &typeinfo)`.
    pub struct_typeinfos: HashMap<DefId, cranelift_module::DataId>,
    /// Per-concrete-list-type typeinfo data symbols,
    /// keyed by the element `Type` (so `List<Int>` maps on `Type::Int`,
    /// `List<List<String>>` maps on `Type::List(Box::new(Type::String))`).
    /// Populated in `lower_file` by walking every `Type::List(_)` the
    /// IR mentions before agent bodies are lowered — so expression-
    /// level list literals just look up by element type.
    pub list_typeinfos: HashMap<Type, cranelift_module::DataId>,
    /// Per-concrete-result-type destructors for wrappers whose active
    /// branch may hold a refcounted payload.
    pub result_destructors: HashMap<Type, FuncId>,
    /// Per-concrete-result-type trace functions.
    pub result_traces: HashMap<Type, FuncId>,
    /// Per-concrete-result-type typeinfo data symbols.
    pub result_typeinfos: HashMap<Type, cranelift_module::DataId>,
    /// Per-concrete-wide-option typeinfo data symbols. Wide scalar
    /// `Option<T>` uses a typed heap wrapper for `Some(...)` and the
    /// zero pointer for `None`.
    pub option_typeinfos: HashMap<Type, cranelift_module::DataId>,
    /// Owned copy of the IR's struct type metadata, keyed by `DefId`.
    /// Cloned into `RuntimeFuncs` in `lower_file` so the per-agent
    /// lowering functions can resolve struct layouts (for field
    /// offsets, constructor arity checks, destructor lookup) without
    /// threading `&IrFile` through every call site.
    pub ir_types: HashMap<DefId, corvid_ir::IrType>,
    /// Tool declarations, keyed by `DefId`. The codegen
    /// needs to know the declared signature (param types, return type)
    /// to emit a correctly-typed direct call to the `#[tool]` wrapper
    /// symbol. Cloned in from the `IrFile` the same way `ir_types` is.
    pub ir_tools: HashMap<DefId, corvid_ir::IrTool>,
    /// Cache of imported `__corvid_tool_<name>` FuncIds so
    /// repeated calls to the same tool re-use one declaration. First
    /// sight declares; later sights re-use.
    pub tool_wrapper_ids: std::cell::RefCell<HashMap<DefId, FuncId>>,
    /// Prompt declarations, keyed by `DefId`. Codegen reads
    /// each prompt's params + template + return type to emit
    /// signature-aware bridge calls.
    pub ir_prompts: HashMap<DefId, corvid_ir::IrPrompt>,
    pub prompt_pins: HashMap<Span, BTreeSet<LocalId>>,
    /// Per-agent borrow signature, populated from
    /// `IrAgent.borrow_sig` during `lower_file`. Consumed at
    /// `IrCallKind::Agent` call sites to decide per-arg whether to
    /// apply the caller-side borrow peephole: if the callee slot
    /// is `Borrowed` AND the argument expression is a bare Local,
    /// skip the pre-call retain (via `lower_expr`) AND the post-call
    /// release.
    pub agent_borrow_sigs: HashMap<DefId, Vec<corvid_ir::ParamBorrow>>,
    /// Per-function stack maps accumulated by
    /// `define_function_with_stack_maps` at each compile site.
    /// After `module.finish()` this is read by `emit_stack_map_table`
    /// to produce the `corvid_stack_maps` `.rodata` symbol that
    /// the cycle collector's mark walk will consult via
    /// `corvid_stack_maps_find(return_pc)`.
    ///
    /// Keyed by `FuncId` so the post-finish emission can resolve
    /// each entry's `return_pc` as `func_sym_addr + code_offset`
    /// via a function-pointer relocation.
    ///
    /// `RefCell` because destructor + trace emission paths hold
    /// `&RuntimeFuncs` (immutable) but still need to push their
    /// stack maps; the ownership-pass integration point had the
    /// same constraint with `tool_wrapper_ids`.
    pub stack_maps: std::cell::RefCell<
        HashMap<FuncId, Vec<(CodeOffset, u32, UserStackMap)>>,
    >,
}

impl RuntimeFuncs {
    /// Allocate the next unique literal symbol number.
    pub(super) fn next_literal_id(&self) -> u64 {
        let n = self.literal_counter.get();
        self.literal_counter.set(n + 1);
        n
    }
}

pub(super) struct TracePayload {
    pub type_tags: ClValue,
    pub count: ClValue,
    pub values_ptr: ClValue,
}

pub(super) fn emit_trace_payload(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    values: &[ClValue],
    tys: &[Type],
    span: Span,
) -> Result<TracePayload, CodegenError> {
    debug_assert_eq!(values.len(), tys.len());
    let tags = tys
        .iter()
        .map(trace_tag_for_type)
        .collect::<Result<String, _>>()?;
    let type_tags = lower_string_literal(builder, module, runtime, &tags, span)?;
    let count = builder.ins().iconst(I64, values.len() as i64);
    let values_ptr = if values.is_empty() {
        builder.ins().iconst(I64, 0)
    } else {
        let stack_slot = builder.create_sized_stack_slot(clir::StackSlotData::new(
            clir::StackSlotKind::ExplicitSlot,
            (values.len() as u32) * 8,
            3,
        ));
        for (idx, (value, ty)) in values.iter().zip(tys.iter()).enumerate() {
            let offset = (idx as i32) * 8;
            match ty {
                Type::Int | Type::String => {
                    builder.ins().stack_store(*value, stack_slot, offset);
                }
                Type::Bool => {
                    let widened = builder.ins().uextend(I64, *value);
                    builder.ins().stack_store(widened, stack_slot, offset);
                }
                Type::Float => {
                    builder.ins().stack_store(*value, stack_slot, offset);
                }
                other => {
                    return Err(CodegenError::not_supported(
                        format!(
                            "native trace payload does not yet support values of type `{}`",
                            other.display_name()
                        ),
                        span,
                    ));
                }
            }
        }
        builder.ins().stack_addr(I64, stack_slot, 0)
    };
    Ok(TracePayload {
        type_tags,
        count,
        values_ptr,
    })
}

fn trace_tag_for_type(ty: &Type) -> Result<char, CodegenError> {
    match ty {
        Type::Int => Ok('i'),
        Type::Bool => Ok('b'),
        Type::Float => Ok('f'),
        Type::String => Ok('s'),
        other => Err(CodegenError::not_supported(
            format!(
                "native trace payload does not yet support values of type `{}`",
                other.display_name()
            ),
            Span::new(0, 0),
        )),
    }
}

/// Loop context entry recorded on the `loop_stack` at `for` entry,
/// consumed by `break` / `continue` statements nested inside.
#[derive(Clone, Copy)]
pub(super) struct LoopCtx {
    /// Block that increments the index counter and jumps to the loop
    /// header. `continue` jumps here.
    pub step_block: clir::Block,
    /// Block that the loop exits to. `break` jumps here.
    pub exit_block: clir::Block,
    /// `scope_stack.len()` at the point the loop was entered, BEFORE
    /// the loop body pushed its own scope. `break` / `continue` walk
    /// scopes from the current depth down to (but not including) this
    /// value, releasing refcounted locals as they go.
    pub scope_depth_at_entry: usize,
}

/// Per-agent mutable state: the `LocalId → Variable` map, the
/// monotonic Variable index, and the scope stack tracking refcounted
/// locals for end-of-scope releases.
///
/// `scope_stack` mirrors Corvid's lexical scoping rather than
/// Cranelift's flat-Variable model: each `if`/`else` branch pushes its
/// own scope; locals declared inside a branch get released at branch
/// exit; function-root locals release at function exit.
pub(super) struct LocalsCtx {
    /// Bound locals: id → (Cranelift variable, declared width).
    pub env: HashMap<LocalId, (Variable, clir::Type)>,
    /// Monotonic Variable id counter — unique per agent.
    pub var_idx: usize,
    /// Stack of nested scopes, innermost on top. Each scope holds the
    /// refcounted locals declared *in that scope*.
    pub scope_stack: Vec<Vec<(LocalId, Variable)>>,
}

impl LocalsCtx {
    pub(super) fn new() -> Self {
        Self {
            env: HashMap::new(),
            var_idx: 0,
            scope_stack: Vec::new(),
        }
    }

    /// Push a fresh scope onto the stack. Call at every block entry.
    pub(super) fn enter_scope(&mut self) {
        self.scope_stack.push(Vec::new());
    }

    /// Pop the current scope and return its refcounted locals so the
    /// caller can emit `release` calls *before* the block terminator.
    pub(super) fn exit_scope(&mut self) -> Vec<(LocalId, Variable)> {
        self.scope_stack.pop().unwrap_or_default()
    }

    /// Register a refcounted local in the current scope. Called from
    /// `IrStmt::Let` when a *new* binding (not reassignment) is made
    /// for a String / Struct / List type.
    pub(super) fn track_refcounted(&mut self, local_id: LocalId, var: Variable) {
        if let Some(top) = self.scope_stack.last_mut() {
            top.push((local_id, var));
        }
    }

    /// Iterate over every refcounted local across all scopes,
    /// innermost first. Used by `IrStmt::Return` to release all live
    /// locals before transferring the return value to the caller.
    pub(super) fn all_refcounted_innermost_first(&self) -> impl Iterator<Item = &(LocalId, Variable)> {
        self.scope_stack.iter().rev().flat_map(|s| s.iter().rev())
    }
}

impl Default for LocalsCtx {
    fn default() -> Self {
        Self::new()
    }
}


/// Symbol name used by the C entry shim to pick up the runtime
/// overflow handler. Declared here so both codegen and the shim agree.
pub(super) const OVERFLOW_HANDLER_SYMBOL: &str = "corvid_runtime_overflow";

/// Stable symbol the C shim calls into. The codegen emits a trampoline
/// with this name that forwards to the user-chosen entry agent. Keeps
/// the shim source constant regardless of what the user named their
/// agent.
pub(super) const ENTRY_TRAMPOLINE_SYMBOL: &str = "corvid_entry";


