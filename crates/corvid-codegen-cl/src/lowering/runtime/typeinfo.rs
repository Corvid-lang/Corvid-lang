//! Per-type typeinfo emission.
//!
//! Every refcounted type has a `corvid_typeinfo_<TypeName>`
//! .rodata symbol that pins its destroy_fn, trace_fn, weak_fn,
//! and (for lists) elem_typeinfo. The runtime allocator
//! `corvid_alloc_typed` writes a typeinfo pointer into each
//! object's header; `corvid_release` and the future cycle
//! collector both dispatch through it.
//!
//! Layout constants here must match `corvid_typeinfo` in
//! `crates/corvid-runtime/runtime/alloc.c` exactly.

use super::*;

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

pub const RESULT_PAYLOAD_BYTES: i64 = 16;
pub const RESULT_TAG_OFFSET: i32 = 0;
pub const RESULT_PAYLOAD_OFFSET: i32 = 8;
pub const RESULT_TAG_OK: i64 = 0;
pub const RESULT_TAG_ERR: i64 = 1;
pub const OPTION_PAYLOAD_BYTES: i64 = 8;
pub const OPTION_PAYLOAD_OFFSET: i32 = 0;

/// Emit `corvid_typeinfo_<TypeName>` as a .rodata data symbol with
/// function-pointer relocations to the type's destroy_fn (if any)
/// and trace_fn. Returns the DataId so allocations can reference it.
pub fn emit_struct_typeinfo(
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

    module.define_data(data_id, &desc).map_err(|e| {
        CodegenError::cranelift(format!("define typeinfo `{symbol}`: {e}"), ty.span)
    })?;
    Ok(data_id)
}

pub fn emit_result_typeinfo(
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

    module.define_data(data_id, &desc).map_err(|e| {
        CodegenError::cranelift(format!("define typeinfo `{symbol}`: {e}"), Span::new(0, 0))
    })?;
    Ok(data_id)
}

pub fn emit_option_typeinfo(
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
            CodegenError::cranelift(
                format!("declare option typeinfo `{symbol}`: {e}"),
                Span::new(0, 0),
            )
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

    module.define_data(data_id, &desc).map_err(|e| {
        CodegenError::cranelift(
            format!("define option typeinfo `{symbol}`: {e}"),
            Span::new(0, 0),
        )
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
pub fn emit_list_typeinfo(
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

    module.define_data(data_id, &desc).map_err(|e| {
        CodegenError::cranelift(
            format!("define list typeinfo `{symbol}`: {e}"),
            Span::new(0, 0),
        )
    })?;
    Ok(data_id)
}

pub fn typeinfo_data_for_refcounted_payload(
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
