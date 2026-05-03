pub(in crate::lowering) const RETAIN_SYMBOL: &str = "corvid_retain";

/// `void corvid_release(void* payload)` — atomic refcount decrement;
/// frees the underlying block when refcount hits zero.
pub(in crate::lowering) const RELEASE_SYMBOL: &str = "corvid_release";

/// `void* corvid_string_concat(void* a, void* b)` — allocates a fresh
/// String (refcount = 1) containing `a` followed by `b`.
pub(in crate::lowering) const STRING_CONCAT_SYMBOL: &str = "corvid_string_concat";

/// `long long corvid_string_eq(void* a, void* b)` — bytewise equality.
pub(in crate::lowering) const STRING_EQ_SYMBOL: &str = "corvid_string_eq";

/// `long long corvid_string_cmp(void* a, void* b)` — bytewise compare.
pub(in crate::lowering) const STRING_CMP_SYMBOL: &str = "corvid_string_cmp";
pub(in crate::lowering) const STRING_CHAR_LEN_SYMBOL: &str = "corvid_string_char_len";
pub(in crate::lowering) const STRING_CHAR_AT_SYMBOL: &str = "corvid_string_char_at";

/// `void* corvid_alloc_typed(long long payload_bytes, const corvid_typeinfo* ti)`
/// — heap-allocate an N-byte payload behind a 16-byte typed header.
/// The typed allocator collapsed the old `corvid_alloc` + `corvid_alloc_with_destructor`
/// pair: every allocation now carries a typeinfo pointer, and
/// `corvid_release` dispatches through `typeinfo->destroy_fn` (NULL
/// = no refcounted children, equivalent to the old plain-alloc case).
pub(in crate::lowering) const ALLOC_TYPED_SYMBOL: &str = "corvid_alloc_typed";

/// `void corvid_destroy_list(void* payload)` — shared runtime
/// destructor installed in every refcounted-element list type's
/// typeinfo. Walks length at offset 0 and `corvid_release`s each
/// element. Primitive-element lists leave `destroy_fn` NULL.
pub(in crate::lowering) const LIST_DESTROY_SYMBOL: &str = "corvid_destroy_list";

/// `void corvid_trace_list(void*, void(*)(void*, void*), void*)` —
/// shared runtime tracer installed in every list type's typeinfo.
/// Reads its own typeinfo's `elem_typeinfo` to decide whether to
/// walk elements (NULL = primitive elements = no-op). Codegen
/// emits it for every list; the collector's mark walk invokes it.
pub(in crate::lowering) const LIST_TRACE_SYMBOL: &str = "corvid_trace_list";
pub(in crate::lowering) const WEAK_NEW_SYMBOL: &str = "corvid_weak_new";
pub(in crate::lowering) const WEAK_UPGRADE_SYMBOL: &str = "corvid_weak_upgrade";
pub(in crate::lowering) const WEAK_CLEAR_SELF_SYMBOL: &str = "corvid_weak_clear_self";
pub(in crate::lowering) const WEAK_BOX_TYPEINFO_SYMBOL: &str = "corvid_typeinfo_WeakBox";

/// Built-in `corvid_typeinfo_String` — the runtime provides this
/// symbol in `alloc.c`. Static string literals in `.rodata` and
/// runtime-internal String allocations both reference it so the
/// codegen doesn't have to emit a stray typeinfo per compilation
/// for string-less programs.
pub(in crate::lowering) const STRING_TYPEINFO_SYMBOL: &str = "corvid_typeinfo_String";

// Entry-agent helpers (argv decoding, result printing,
// arity reporting, atexit). Called from the codegen-emitted `main`.

pub(in crate::lowering) const ENTRY_INIT_SYMBOL: &str = "corvid_init";
pub(in crate::lowering) const ENTRY_ARITY_MISMATCH_SYMBOL: &str = "corvid_arity_mismatch";
pub(in crate::lowering) const PARSE_I64_SYMBOL: &str = "corvid_parse_i64";
pub(in crate::lowering) const PARSE_F64_SYMBOL: &str = "corvid_parse_f64";
pub(in crate::lowering) const PARSE_BOOL_SYMBOL: &str = "corvid_parse_bool";
pub(in crate::lowering) const STRING_FROM_CSTR_SYMBOL: &str = "corvid_string_from_cstr";
pub(in crate::lowering) const PRINT_I64_SYMBOL: &str = "corvid_print_i64";
pub(in crate::lowering) const PRINT_BOOL_SYMBOL: &str = "corvid_print_bool";
pub(in crate::lowering) const PRINT_F64_SYMBOL: &str = "corvid_print_f64";
pub(in crate::lowering) const PRINT_STRING_SYMBOL: &str = "corvid_print_string";
pub(in crate::lowering) const BENCH_SERVER_ENABLED_SYMBOL: &str = "corvid_bench_server_enabled";
pub(in crate::lowering) const BENCH_NEXT_TRIAL_SYMBOL: &str = "corvid_bench_next_trial";
pub(in crate::lowering) const BENCH_FINISH_TRIAL_SYMBOL: &str = "corvid_bench_finish_trial";

// Async tool dispatch bridge. Signature in Rust:
pub(in crate::lowering) const RUNTIME_IS_REPLAY_SYMBOL: &str = "corvid_runtime_is_replay";
pub(in crate::lowering) const REPLAY_TOOL_CALL_NOTHING_SYMBOL: &str =
    "corvid_replay_tool_call_nothing";
pub(in crate::lowering) const REPLAY_TOOL_CALL_INT_SYMBOL: &str = "corvid_replay_tool_call_int";
pub(in crate::lowering) const REPLAY_TOOL_CALL_BOOL_SYMBOL: &str = "corvid_replay_tool_call_bool";
pub(in crate::lowering) const REPLAY_TOOL_CALL_FLOAT_SYMBOL: &str = "corvid_replay_tool_call_float";
pub(in crate::lowering) const REPLAY_TOOL_CALL_STRING_SYMBOL: &str =
    "corvid_replay_tool_call_string";

// JSON encoder primitives backing the trace-payload `'j'` slot. The
// Cranelift codegen walks each non-scalar tool/prompt/approve argument
// type, appends its JSON representation to a buffer via these calls,
// and finalizes the buffer into a refcounted Corvid String descriptor
// stored in the trace slot. Implementations live in `runtime/json.c`.
pub(in crate::lowering) const JSON_BUFFER_NEW_SYMBOL: &str = "corvid_json_buffer_new";
pub(in crate::lowering) const JSON_BUFFER_FINISH_SYMBOL: &str = "corvid_json_buffer_finish";
pub(in crate::lowering) const JSON_BUFFER_APPEND_RAW_SYMBOL: &str = "corvid_json_buffer_append_raw";
pub(in crate::lowering) const JSON_BUFFER_APPEND_INT_SYMBOL: &str = "corvid_json_buffer_append_int";
pub(in crate::lowering) const JSON_BUFFER_APPEND_FLOAT_SYMBOL: &str =
    "corvid_json_buffer_append_float";
pub(in crate::lowering) const JSON_BUFFER_APPEND_BOOL_SYMBOL: &str =
    "corvid_json_buffer_append_bool";
pub(in crate::lowering) const JSON_BUFFER_APPEND_NULL_SYMBOL: &str =
    "corvid_json_buffer_append_null";
pub(in crate::lowering) const JSON_BUFFER_APPEND_STRING_SYMBOL: &str =
    "corvid_json_buffer_append_string";

// Scalar-to-String stringification helpers. Used by the
// Cranelift codegen for `IrCallKind::Prompt` lowering when a
// non-String argument is interpolated into a prompt template. Each
// returns a refcount-1 Corvid String the caller must release.
pub(in crate::lowering) const STRING_FROM_INT_SYMBOL: &str = "corvid_string_from_int";
pub(in crate::lowering) const STRING_FROM_BOOL_SYMBOL: &str = "corvid_string_from_bool";
pub(in crate::lowering) const STRING_FROM_FLOAT_SYMBOL: &str = "corvid_string_from_float";

// Typed prompt-dispatch bridges. One per return type;
// each takes 4 CorvidString args (prompt name, signature, rendered
// template, model) and returns the typed value. Built-in
// retry-with-validation + function-signature context — see the
// Rust-side implementations in `corvid-runtime::ffi_bridge`.
pub(in crate::lowering) const PROMPT_CALL_INT_SYMBOL: &str = "corvid_prompt_call_int";
pub(in crate::lowering) const PROMPT_CALL_BOOL_SYMBOL: &str = "corvid_prompt_call_bool";
pub(in crate::lowering) const PROMPT_CALL_FLOAT_SYMBOL: &str = "corvid_prompt_call_float";
pub(in crate::lowering) const PROMPT_CALL_STRING_SYMBOL: &str = "corvid_prompt_call_string";
pub(in crate::lowering) const CITATION_VERIFY_OR_PANIC_SYMBOL: &str =
    "corvid_citation_verify_or_panic";
pub(in crate::lowering) const APPROVE_SYNC_SYMBOL: &str = "corvid_approve_sync";
pub(in crate::lowering) const TRACE_RUN_STARTED_SYMBOL: &str = "corvid_trace_run_started";
pub(in crate::lowering) const TRACE_RUN_COMPLETED_INT_SYMBOL: &str =
    "corvid_trace_run_completed_int";
pub(in crate::lowering) const TRACE_RUN_COMPLETED_BOOL_SYMBOL: &str =
    "corvid_trace_run_completed_bool";
pub(in crate::lowering) const TRACE_RUN_COMPLETED_FLOAT_SYMBOL: &str =
    "corvid_trace_run_completed_float";
pub(in crate::lowering) const TRACE_RUN_COMPLETED_STRING_SYMBOL: &str =
    "corvid_trace_run_completed_string";
pub(in crate::lowering) const TRACE_TOOL_CALL_SYMBOL: &str = "corvid_trace_tool_call";
pub(in crate::lowering) const TRACE_TOOL_RESULT_NULL_SYMBOL: &str = "corvid_trace_tool_result_null";
pub(in crate::lowering) const TRACE_TOOL_RESULT_INT_SYMBOL: &str = "corvid_trace_tool_result_int";
pub(in crate::lowering) const TRACE_TOOL_RESULT_BOOL_SYMBOL: &str = "corvid_trace_tool_result_bool";
pub(in crate::lowering) const TRACE_TOOL_RESULT_FLOAT_SYMBOL: &str =
    "corvid_trace_tool_result_float";
pub(in crate::lowering) const TRACE_TOOL_RESULT_STRING_SYMBOL: &str =
    "corvid_trace_tool_result_string";

// Runtime bridge init/shutdown called from `corvid_init`
// at the start of codegen-emitted `main` when the program uses any
// tool/prompt/approve construct. Tool-free programs skip these
// calls to preserve startup benchmark numbers.
pub(in crate::lowering) const RUNTIME_INIT_SYMBOL: &str = "corvid_runtime_init";
pub(in crate::lowering) const RUNTIME_SHUTDOWN_SYMBOL: &str = "corvid_runtime_shutdown";
pub(in crate::lowering) const RUNTIME_EMBED_INIT_SYMBOL: &str = "corvid_runtime_embed_init_default";
pub(in crate::lowering) const SLEEP_MS_SYMBOL: &str = "corvid_sleep_ms";
pub(in crate::lowering) const STRING_INTO_CSTR_SYMBOL: &str = "corvid_string_into_cstr";
pub(in crate::lowering) const BEGIN_DIRECT_OBSERVATION_SYMBOL: &str =
    "corvid_begin_direct_observation";
pub(in crate::lowering) const FINISH_DIRECT_OBSERVATION_SYMBOL: &str =
    "corvid_finish_direct_observation";
pub(in crate::lowering) const GROUNDED_CAPTURE_SCALAR_HANDLE_SYMBOL: &str =
    "corvid_grounded_capture_scalar_handle";
pub(in crate::lowering) const GROUNDED_CAPTURE_STRING_HANDLE_SYMBOL: &str =
    "corvid_grounded_capture_string_handle";
pub(in crate::lowering) const GROUNDED_ATTEST_INT_SYMBOL: &str = "corvid_grounded_attest_int";
pub(in crate::lowering) const GROUNDED_ATTEST_BOOL_SYMBOL: &str = "corvid_grounded_attest_bool";
pub(in crate::lowering) const GROUNDED_ATTEST_FLOAT_SYMBOL: &str = "corvid_grounded_attest_float";
pub(in crate::lowering) const GROUNDED_ATTEST_STRING_SYMBOL: &str = "corvid_grounded_attest_string";

pub(in crate::lowering) const OVERFLOW_HANDLER_SYMBOL: &str = "corvid_runtime_overflow";
