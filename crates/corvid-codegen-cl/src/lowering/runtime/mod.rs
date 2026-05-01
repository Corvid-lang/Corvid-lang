use super::*;

mod destructors;
mod stackmap;
mod trace;
mod type_query;
mod typeinfo;
pub(super) use destructors::{
    define_option_destructor, define_result_destructor, define_struct_destructor,
};
pub(super) use stackmap::{define_function_with_stack_maps, emit_stack_map_table};
pub(super) use trace::{
    define_option_trace, define_result_trace, define_struct_trace, emit_trace_payload,
};
pub(super) use type_query::{
    collect_list_element_types, collect_option_types, collect_result_types, emit_release,
    emit_retain, is_native_option_expr_type, is_native_option_type, is_native_result_type,
    is_native_wide_option_type, is_refcounted_type, mangle_type_name, option_uses_wrapper,
};
pub(super) use typeinfo::{
    emit_list_typeinfo, emit_option_typeinfo, emit_result_typeinfo, emit_struct_typeinfo,
    typeinfo_data_for_refcounted_payload, OPTION_PAYLOAD_BYTES, OPTION_PAYLOAD_OFFSET,
    RESULT_PAYLOAD_BYTES, RESULT_PAYLOAD_OFFSET, RESULT_TAG_ERR, RESULT_TAG_OFFSET,
    RESULT_TAG_OK,
};

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
pub(super) const STRING_CHAR_LEN_SYMBOL: &str = "corvid_string_char_len";
pub(super) const STRING_CHAR_AT_SYMBOL: &str = "corvid_string_char_at";

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
pub(super) const RUNTIME_IS_REPLAY_SYMBOL: &str = "corvid_runtime_is_replay";
pub(super) const REPLAY_TOOL_CALL_NOTHING_SYMBOL: &str = "corvid_replay_tool_call_nothing";
pub(super) const REPLAY_TOOL_CALL_INT_SYMBOL: &str = "corvid_replay_tool_call_int";
pub(super) const REPLAY_TOOL_CALL_BOOL_SYMBOL: &str = "corvid_replay_tool_call_bool";
pub(super) const REPLAY_TOOL_CALL_FLOAT_SYMBOL: &str = "corvid_replay_tool_call_float";
pub(super) const REPLAY_TOOL_CALL_STRING_SYMBOL: &str = "corvid_replay_tool_call_string";

// JSON encoder primitives backing the trace-payload `'j'` slot. The
// Cranelift codegen walks each non-scalar tool/prompt/approve argument
// type, appends its JSON representation to a buffer via these calls,
// and finalizes the buffer into a refcounted Corvid String descriptor
// stored in the trace slot. Implementations live in `runtime/json.c`.
pub(super) const JSON_BUFFER_NEW_SYMBOL: &str = "corvid_json_buffer_new";
pub(super) const JSON_BUFFER_FINISH_SYMBOL: &str = "corvid_json_buffer_finish";
pub(super) const JSON_BUFFER_APPEND_RAW_SYMBOL: &str = "corvid_json_buffer_append_raw";
pub(super) const JSON_BUFFER_APPEND_INT_SYMBOL: &str = "corvid_json_buffer_append_int";
pub(super) const JSON_BUFFER_APPEND_FLOAT_SYMBOL: &str = "corvid_json_buffer_append_float";
pub(super) const JSON_BUFFER_APPEND_BOOL_SYMBOL: &str = "corvid_json_buffer_append_bool";
pub(super) const JSON_BUFFER_APPEND_NULL_SYMBOL: &str = "corvid_json_buffer_append_null";
pub(super) const JSON_BUFFER_APPEND_STRING_SYMBOL: &str = "corvid_json_buffer_append_string";

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
pub(super) const CITATION_VERIFY_OR_PANIC_SYMBOL: &str = "corvid_citation_verify_or_panic";
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
pub(super) const BEGIN_DIRECT_OBSERVATION_SYMBOL: &str = "corvid_begin_direct_observation";
pub(super) const FINISH_DIRECT_OBSERVATION_SYMBOL: &str = "corvid_finish_direct_observation";
pub(super) const GROUNDED_CAPTURE_SCALAR_HANDLE_SYMBOL: &str =
    "corvid_grounded_capture_scalar_handle";
pub(super) const GROUNDED_CAPTURE_STRING_HANDLE_SYMBOL: &str =
    "corvid_grounded_capture_string_handle";
pub(super) const GROUNDED_ATTEST_INT_SYMBOL: &str = "corvid_grounded_attest_int";
pub(super) const GROUNDED_ATTEST_BOOL_SYMBOL: &str = "corvid_grounded_attest_bool";
pub(super) const GROUNDED_ATTEST_FLOAT_SYMBOL: &str = "corvid_grounded_attest_float";
pub(super) const GROUNDED_ATTEST_STRING_SYMBOL: &str = "corvid_grounded_attest_string";

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
    pub string_char_len: FuncId,
    pub string_char_at: FuncId,
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
    // Runtime init/shutdown + replay bridges.
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
    pub json_buffer_new: FuncId,
    pub json_buffer_finish: FuncId,
    pub json_buffer_append_raw: FuncId,
    pub json_buffer_append_int: FuncId,
    pub json_buffer_append_float: FuncId,
    pub json_buffer_append_bool: FuncId,
    pub json_buffer_append_null: FuncId,
    pub json_buffer_append_string: FuncId,
    pub string_into_cstr: FuncId,
    pub begin_direct_observation: FuncId,
    pub finish_direct_observation: FuncId,
    pub grounded_capture_scalar_handle: FuncId,
    pub grounded_capture_string_handle: FuncId,
    pub grounded_attest_int: FuncId,
    pub grounded_attest_bool: FuncId,
    pub grounded_attest_float: FuncId,
    pub grounded_attest_string: FuncId,
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
    pub citation_verify_or_panic: FuncId,
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
    pub stack_maps: std::cell::RefCell<HashMap<FuncId, Vec<(CodeOffset, u32, UserStackMap)>>>,
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
    pub owned_values: Vec<ClValue>,
}


/// Symbol name used by the C entry shim to pick up the runtime
/// overflow handler. Declared here so both codegen and the shim agree.
pub(super) const OVERFLOW_HANDLER_SYMBOL: &str = "corvid_runtime_overflow";

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
    /// Native string iteration synthesizes owned per-iteration `String`
    /// values that are not represented as source-level owned locals in
    /// the dup/drop pass. This tracks that loop variable so
    /// `break`/`continue` can release it explicitly when needed.
    pub loop_owned_local: Option<Variable>,
}
