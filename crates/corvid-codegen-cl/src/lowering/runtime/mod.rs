use super::*;

mod declare;
mod destructors;
mod stackmap;
mod symbols;
mod trace;
mod type_query;
mod typeinfo;
pub(super) use declare::declare_runtime_funcs;
pub(super) use destructors::{
    define_option_destructor, define_result_destructor, define_struct_destructor,
};
pub(super) use stackmap::{define_function_with_stack_maps, emit_stack_map_table};
pub(super) use symbols::*;
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
    RESULT_PAYLOAD_BYTES, RESULT_PAYLOAD_OFFSET, RESULT_TAG_ERR, RESULT_TAG_OFFSET, RESULT_TAG_OK,
};

// ---- runtime helper symbols ----
//
// The C runtime in `runtime/{alloc,strings}.c` exports these symbols.
// `lower_file` declares them once per module as `Linkage::Import`; each
// per-function lowering uses `module.declare_func_in_func` to get a
// FuncRef, then `builder.ins().call`.

/// `void corvid_retain(void* payload)` — atomic refcount increment.

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
