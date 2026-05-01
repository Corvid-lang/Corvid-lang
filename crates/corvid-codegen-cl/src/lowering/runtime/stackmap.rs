//! Stack-map preservation and emission for the cycle collector.
//!
//! Cranelift produces user stack maps during `compile()` but the
//! standard `ObjectModule::define_function` path drops them on the
//! floor. `define_function_with_stack_maps` replicates the inner
//! compile→define_function_bytes pipeline so we can intercept them.
//! `emit_stack_map_table` then writes them out as the
//! `corvid_stack_maps` data symbol consumed by the runtime collector.

use super::*;

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
pub fn define_function_with_stack_maps(
    module: &mut ObjectModule,
    func_id: FuncId,
    ctx: &mut Context,
    runtime: &RuntimeFuncs,
    error_span: Span,
    error_context: &str,
) -> Result<(), CodegenError> {
    let mut ctrl_plane = ControlPlane::default();
    ctx.compile(module.isa(), &mut ctrl_plane).map_err(|e| {
        CodegenError::cranelift(format!("compile `{error_context}`: {e:?}"), error_span)
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
        runtime.stack_maps.borrow_mut().insert(func_id, extracted);
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
            CodegenError::cranelift(format!("define `{error_context}`: {e}"), error_span)
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
pub fn emit_stack_map_table(
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
            CodegenError::cranelift(format!("declare corvid_stack_maps: {e}"), Span::new(0, 0))
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
        CodegenError::cranelift(format!("define corvid_stack_maps: {e}"), Span::new(0, 0))
    })?;
    Ok(())
}
