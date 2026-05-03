use super::*;

/// Per-struct payload uses fixed 8-byte field slots for simple offset
/// math. Tighter packing is a later optimization.
pub(in crate::lowering) const STRUCT_FIELD_SLOT_BYTES: i32 = 8;

/// Bytes per struct field when computing alloc size.
pub(in crate::lowering) fn struct_payload_bytes(n_fields: usize) -> i64 {
    (n_fields as i64) * (STRUCT_FIELD_SLOT_BYTES as i64)
}

pub(in crate::lowering) struct TracePayload {
    pub type_tags: ClValue,
    pub count: ClValue,
    pub values_ptr: ClValue,
    pub owned_values: Vec<ClValue>,
}

/// Loop context entry recorded on the `loop_stack` at `for` entry,
/// consumed by `break` / `continue` statements nested inside.
#[derive(Clone, Copy)]
pub(in crate::lowering) struct LoopCtx {
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
