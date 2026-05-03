use super::*;

pub(in crate::lowering) fn declare_runtime_funcs(
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
        .map_err(|e| {
            CodegenError::cranelift(format!("declare string_cmp: {e}"), Span::new(0, 0))
        })?;

    let mut char_len_sig = module.make_signature();
    char_len_sig.params.push(AbiParam::new(I64));
    char_len_sig.returns.push(AbiParam::new(I64));
    let string_char_len_id = module
        .declare_function(STRING_CHAR_LEN_SYMBOL, Linkage::Import, &char_len_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare string_char_len: {e}"), Span::new(0, 0))
        })?;

    let mut char_at_sig = module.make_signature();
    char_at_sig.params.push(AbiParam::new(I64));
    char_at_sig.params.push(AbiParam::new(I64));
    char_at_sig.returns.push(AbiParam::new(I64));
    let string_char_at_id = module
        .declare_function(STRING_CHAR_AT_SYMBOL, Linkage::Import, &char_at_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare string_char_at: {e}"), Span::new(0, 0))
        })?;

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
            CodegenError::cranelift(format!("declare list destroy: {e}"), Span::new(0, 0))
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
            CodegenError::cranelift(format!("declare list trace: {e}"), Span::new(0, 0))
        })?;

    let mut weak_unary_sig = module.make_signature();
    weak_unary_sig.params.push(AbiParam::new(I64));
    weak_unary_sig.returns.push(AbiParam::new(I64));
    let weak_new_id = module
        .declare_function(WEAK_NEW_SYMBOL, Linkage::Import, &weak_unary_sig)
        .map_err(|e| CodegenError::cranelift(format!("declare weak_new: {e}"), Span::new(0, 0)))?;
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
            CodegenError::cranelift(format!("declare weak_clear_self: {e}"), Span::new(0, 0))
        })?;

    // corvid_typeinfo_String — runtime-provided data symbol. Declared
    // here so codegen can reference it from static string literal
    // descriptors and from List<String>'s elem_typeinfo slot.
    let string_typeinfo_id = module
        .declare_data(STRING_TYPEINFO_SYMBOL, Linkage::Import, false, false)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare String typeinfo: {e}"), Span::new(0, 0))
        })?;
    let weak_box_typeinfo_id = module
        .declare_data(WEAK_BOX_TYPEINFO_SYMBOL, Linkage::Import, false, false)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare WeakBox typeinfo: {e}"), Span::new(0, 0))
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
            CodegenError::cranelift(format!("declare arity_mismatch: {e}"), Span::new(0, 0))
        })?;

    // parse helpers: (cstr_ptr, argv_index) -> typed value
    let mut parse_i64_sig = module.make_signature();
    parse_i64_sig.params.push(AbiParam::new(I64));
    parse_i64_sig.params.push(AbiParam::new(I64));
    parse_i64_sig.returns.push(AbiParam::new(I64));
    let parse_i64_id = module
        .declare_function(PARSE_I64_SYMBOL, Linkage::Import, &parse_i64_sig)
        .map_err(|e| CodegenError::cranelift(format!("declare parse_i64: {e}"), Span::new(0, 0)))?;

    let mut parse_f64_sig = module.make_signature();
    parse_f64_sig.params.push(AbiParam::new(I64));
    parse_f64_sig.params.push(AbiParam::new(I64));
    parse_f64_sig.returns.push(AbiParam::new(F64));
    let parse_f64_id = module
        .declare_function(PARSE_F64_SYMBOL, Linkage::Import, &parse_f64_sig)
        .map_err(|e| CodegenError::cranelift(format!("declare parse_f64: {e}"), Span::new(0, 0)))?;

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
        .map_err(|e| CodegenError::cranelift(format!("declare print_i64: {e}"), Span::new(0, 0)))?;

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
        .map_err(|e| CodegenError::cranelift(format!("declare print_f64: {e}"), Span::new(0, 0)))?;

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
        .declare_function(
            BENCH_SERVER_ENABLED_SYMBOL,
            Linkage::Import,
            &bench_enabled_sig,
        )
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
        .declare_function(
            BENCH_FINISH_TRIAL_SYMBOL,
            Linkage::Import,
            &bench_finish_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(format!("declare bench_finish_trial: {e}"), Span::new(0, 0))
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
            CodegenError::cranelift(format!("declare runtime_is_replay: {e}"), Span::new(0, 0))
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
            CodegenError::cranelift(format!("declare runtime_shutdown: {e}"), Span::new(0, 0))
        })?;

    let embed_init_sig = module.make_signature();
    let embed_init_id = module
        .declare_function(RUNTIME_EMBED_INIT_SYMBOL, Linkage::Import, &embed_init_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare runtime_embed_init: {e}"), Span::new(0, 0))
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
            CodegenError::cranelift(format!("declare string_into_cstr: {e}"), Span::new(0, 0))
        })?;

    let mut begin_direct_observation_sig = module.make_signature();
    begin_direct_observation_sig.params.push(AbiParam::new(F64));
    let begin_direct_observation_id = module
        .declare_function(
            BEGIN_DIRECT_OBSERVATION_SYMBOL,
            Linkage::Import,
            &begin_direct_observation_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare begin_direct_observation: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut finish_direct_observation_sig = module.make_signature();
    finish_direct_observation_sig
        .params
        .push(AbiParam::new(I64));
    let finish_direct_observation_id = module
        .declare_function(
            FINISH_DIRECT_OBSERVATION_SYMBOL,
            Linkage::Import,
            &finish_direct_observation_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare finish_direct_observation: {e}"),
                Span::new(0, 0),
            )
        })?;

    let grounded_capture_scalar_sig = module.make_signature();
    let grounded_capture_scalar_handle_id = module
        .declare_function(
            GROUNDED_CAPTURE_SCALAR_HANDLE_SYMBOL,
            Linkage::Import,
            &grounded_capture_scalar_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare grounded_capture_scalar_handle: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut grounded_capture_string_sig = module.make_signature();
    grounded_capture_string_sig.params.push(AbiParam::new(I64));
    grounded_capture_string_sig.returns.push(AbiParam::new(I64));
    let grounded_capture_string_handle_id = module
        .declare_function(
            GROUNDED_CAPTURE_STRING_HANDLE_SYMBOL,
            Linkage::Import,
            &grounded_capture_string_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare grounded_capture_string_handle: {e}"),
                Span::new(0, 0),
            )
        })?;

    let make_grounded_attest_sig =
        |module: &mut ObjectModule, value_ty: cranelift_codegen::ir::Type| {
            let mut sig = module.make_signature();
            sig.params.push(AbiParam::new(value_ty));
            sig.params.push(AbiParam::new(I64));
            sig.params.push(AbiParam::new(F64));
            sig.returns.push(AbiParam::new(value_ty));
            sig
        };
    let grounded_attest_int_sig = make_grounded_attest_sig(module, I64);
    let grounded_attest_int_id = module
        .declare_function(
            GROUNDED_ATTEST_INT_SYMBOL,
            Linkage::Import,
            &grounded_attest_int_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(format!("declare grounded_attest_int: {e}"), Span::new(0, 0))
        })?;
    let grounded_attest_bool_sig = make_grounded_attest_sig(module, I8);
    let grounded_attest_bool_id = module
        .declare_function(
            GROUNDED_ATTEST_BOOL_SYMBOL,
            Linkage::Import,
            &grounded_attest_bool_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare grounded_attest_bool: {e}"),
                Span::new(0, 0),
            )
        })?;
    let grounded_attest_float_sig = make_grounded_attest_sig(module, F64);
    let grounded_attest_float_id = module
        .declare_function(
            GROUNDED_ATTEST_FLOAT_SYMBOL,
            Linkage::Import,
            &grounded_attest_float_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare grounded_attest_float: {e}"),
                Span::new(0, 0),
            )
        })?;
    let grounded_attest_string_sig = make_grounded_attest_sig(module, I64);
    let grounded_attest_string_id = module
        .declare_function(
            GROUNDED_ATTEST_STRING_SYMBOL,
            Linkage::Import,
            &grounded_attest_string_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare grounded_attest_string: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut sleep_ms_sig = module.make_signature();
    sleep_ms_sig.params.push(AbiParam::new(I64));
    let sleep_ms_id = module
        .declare_function(SLEEP_MS_SYMBOL, Linkage::Import, &sleep_ms_sig)
        .map_err(|e| CodegenError::cranelift(format!("declare sleep_ms: {e}"), Span::new(0, 0)))?;

    // JSON encoder primitives backing the trace-payload `'j'` slot.
    let mut json_buffer_new_sig = module.make_signature();
    json_buffer_new_sig.returns.push(AbiParam::new(I64));
    let json_buffer_new_id = module
        .declare_function(
            JSON_BUFFER_NEW_SYMBOL,
            Linkage::Import,
            &json_buffer_new_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(format!("declare json_buffer_new: {e}"), Span::new(0, 0))
        })?;

    let mut json_buffer_finish_sig = module.make_signature();
    json_buffer_finish_sig.params.push(AbiParam::new(I64));
    json_buffer_finish_sig.returns.push(AbiParam::new(I64));
    let json_buffer_finish_id = module
        .declare_function(
            JSON_BUFFER_FINISH_SYMBOL,
            Linkage::Import,
            &json_buffer_finish_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(format!("declare json_buffer_finish: {e}"), Span::new(0, 0))
        })?;

    let mut json_buffer_append_raw_sig = module.make_signature();
    json_buffer_append_raw_sig.params.push(AbiParam::new(I64));
    json_buffer_append_raw_sig.params.push(AbiParam::new(I64));
    let json_buffer_append_raw_id = module
        .declare_function(
            JSON_BUFFER_APPEND_RAW_SYMBOL,
            Linkage::Import,
            &json_buffer_append_raw_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare json_buffer_append_raw: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut json_buffer_append_int_sig = module.make_signature();
    json_buffer_append_int_sig.params.push(AbiParam::new(I64));
    json_buffer_append_int_sig.params.push(AbiParam::new(I64));
    let json_buffer_append_int_id = module
        .declare_function(
            JSON_BUFFER_APPEND_INT_SYMBOL,
            Linkage::Import,
            &json_buffer_append_int_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare json_buffer_append_int: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut json_buffer_append_float_sig = module.make_signature();
    json_buffer_append_float_sig.params.push(AbiParam::new(I64));
    json_buffer_append_float_sig.params.push(AbiParam::new(F64));
    let json_buffer_append_float_id = module
        .declare_function(
            JSON_BUFFER_APPEND_FLOAT_SYMBOL,
            Linkage::Import,
            &json_buffer_append_float_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare json_buffer_append_float: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut json_buffer_append_bool_sig = module.make_signature();
    json_buffer_append_bool_sig.params.push(AbiParam::new(I64));
    json_buffer_append_bool_sig.params.push(AbiParam::new(I8));
    let json_buffer_append_bool_id = module
        .declare_function(
            JSON_BUFFER_APPEND_BOOL_SYMBOL,
            Linkage::Import,
            &json_buffer_append_bool_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare json_buffer_append_bool: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut json_buffer_append_null_sig = module.make_signature();
    json_buffer_append_null_sig.params.push(AbiParam::new(I64));
    let json_buffer_append_null_id = module
        .declare_function(
            JSON_BUFFER_APPEND_NULL_SYMBOL,
            Linkage::Import,
            &json_buffer_append_null_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare json_buffer_append_null: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut json_buffer_append_string_sig = module.make_signature();
    json_buffer_append_string_sig
        .params
        .push(AbiParam::new(I64));
    json_buffer_append_string_sig
        .params
        .push(AbiParam::new(I64));
    let json_buffer_append_string_id = module
        .declare_function(
            JSON_BUFFER_APPEND_STRING_SYMBOL,
            Linkage::Import,
            &json_buffer_append_string_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare json_buffer_append_string: {e}"),
                Span::new(0, 0),
            )
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
    let make_prompt_sig = |module: &mut ObjectModule, ret_ty: cranelift_codegen::ir::Type| {
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
        .declare_function(
            PROMPT_CALL_STRING_SYMBOL,
            Linkage::Import,
            &prompt_string_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(format!("declare prompt_call_string: {e}"), Span::new(0, 0))
        })?;

    let mut citation_verify_sig = module.make_signature();
    citation_verify_sig.params.push(AbiParam::new(I64));
    citation_verify_sig.params.push(AbiParam::new(I64));
    citation_verify_sig.params.push(AbiParam::new(I64));
    citation_verify_sig.returns.push(AbiParam::new(I8));
    let citation_verify_or_panic_id = module
        .declare_function(
            CITATION_VERIFY_OR_PANIC_SYMBOL,
            Linkage::Import,
            &citation_verify_sig,
        )
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare citation_verify_or_panic: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut trace_run_started_sig = module.make_signature();
    trace_run_started_sig.params.push(AbiParam::new(I64));
    trace_run_started_sig.params.push(AbiParam::new(I64));
    trace_run_started_sig.params.push(AbiParam::new(I64));
    trace_run_started_sig.params.push(AbiParam::new(I64));
    let trace_run_started_id = module
        .declare_function(
            TRACE_RUN_STARTED_SYMBOL,
            Linkage::Import,
            &trace_run_started_sig,
        )
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
    trace_run_completed_float_sig
        .params
        .push(AbiParam::new(F64));
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
    trace_run_completed_string_sig
        .params
        .push(AbiParam::new(I64));
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
        .declare_function(
            TRACE_TOOL_CALL_SYMBOL,
            Linkage::Import,
            &trace_tool_call_sig,
        )
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
        string_char_len: string_char_len_id,
        string_char_at: string_char_at_id,
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
        json_buffer_new: json_buffer_new_id,
        json_buffer_finish: json_buffer_finish_id,
        json_buffer_append_raw: json_buffer_append_raw_id,
        json_buffer_append_int: json_buffer_append_int_id,
        json_buffer_append_float: json_buffer_append_float_id,
        json_buffer_append_bool: json_buffer_append_bool_id,
        json_buffer_append_null: json_buffer_append_null_id,
        json_buffer_append_string: json_buffer_append_string_id,
        string_into_cstr: string_into_cstr_id,
        begin_direct_observation: begin_direct_observation_id,
        finish_direct_observation: finish_direct_observation_id,
        grounded_capture_scalar_handle: grounded_capture_scalar_handle_id,
        grounded_capture_string_handle: grounded_capture_string_handle_id,
        grounded_attest_int: grounded_attest_int_id,
        grounded_attest_bool: grounded_attest_bool_id,
        grounded_attest_float: grounded_attest_float_id,
        grounded_attest_string: grounded_attest_string_id,
        string_from_int: string_from_int_id,
        string_from_bool: string_from_bool_id,
        string_from_float: string_from_float_id,
        approve_sync: approve_sync_id,
        prompt_call_int: prompt_call_int_id,
        prompt_call_bool: prompt_call_bool_id,
        prompt_call_float: prompt_call_float_id,
        prompt_call_string: prompt_call_string_id,
        citation_verify_or_panic: citation_verify_or_panic_id,
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
