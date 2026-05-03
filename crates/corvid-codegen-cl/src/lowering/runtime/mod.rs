use super::*;

mod declare;
mod destructors;
mod funcs;
mod stackmap;
mod symbols;
mod trace;
mod type_query;
mod typeinfo;
pub(super) use declare::declare_runtime_funcs;
pub(super) use destructors::{
    define_option_destructor, define_result_destructor, define_struct_destructor,
};
pub(super) use funcs::RuntimeFuncs;
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
