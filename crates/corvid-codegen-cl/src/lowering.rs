//! IR → Cranelift IR lowering.
//!
//! Slice 12a scope: `Int` parameters, `Int` return, `Int` literals,
//! `Int` arithmetic with overflow trap, agent-to-agent calls,
//! parameter loads, `return`. Everything else raises
//! `CodegenError::NotSupported` with a slice pointer so the boundary
//! is auditable.

use crate::errors::CodegenError;
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::types::{F64, I32, I64, I8};
use cranelift_codegen::ir::{
    self as clir, AbiParam, Function, InstBuilder, MemFlags, Signature, UserFuncName,
    Value as ClValue,
};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{DataDescription, FuncId, Linkage, Module};
use cranelift_object::ObjectModule;
use corvid_ast::{BinaryOp, Span, UnaryOp};
use corvid_ir::{IrAgent, IrBlock, IrCallKind, IrExpr, IrExprKind, IrFile, IrLiteral, IrStmt};
use corvid_resolve::{DefId, LocalId};
use corvid_types::Type;
use std::collections::HashMap;

const _: () = {
    // A readable reminder: slice 12a compiles only Int. Type checks
    // elsewhere should already have enforced this for well-typed programs.
};

#[inline(always)]
fn lowered_outcome_placeholder() -> BlockOutcome {
    BlockOutcome::Normal
}

/// Mangle a user agent's name into a link-safe symbol. Prevents
/// collisions with C runtime symbols (`main`, `printf`, `malloc`, ...).
///
/// Phase 16 includes the agent's `DefId` in the symbol because methods
/// declared inside `extend T:` blocks share their unmangled names
/// across types (`Order.total`, `Line.total` both get the AST name
/// `total`). Including the DefId disambiguates without changing the
/// emitted .obj's user-visible behavior — symbols are internal-only
/// (`Linkage::Local`) so the suffix never leaks into a public API.
fn mangle_agent_symbol(user_name: &str, def_id: DefId) -> String {
    format!("corvid_agent_{user_name}_{}", def_id.0)
}

/// Declare the imported runtime helpers (retain, release, string
/// concat / eq / cmp) and bundle their FuncIds with the overflow
/// handler into a `RuntimeFuncs`.
fn declare_runtime_funcs(
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

    // ---- slice 12i entry helpers ----
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

    // Phase 13 bridge imports.
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

    // Phase 15 stringification helpers. Each takes a typed scalar
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

    // Phase 15 prompt bridges. Each takes 4 CorvidString descriptor
    // pointers (i64) — prompt name, signature, rendered template,
    // model — and returns the typed value.
    let make_prompt_sig =
        |module: &mut ObjectModule, ret_ty: cranelift_codegen::ir::Type| {
            let mut s = module.make_signature();
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
        string_typeinfo: string_typeinfo_id,
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
        tool_call_sync_int: tool_call_sync_int_id,
        runtime_init: runtime_init_id,
        runtime_shutdown: runtime_shutdown_id,
        string_from_int: string_from_int_id,
        string_from_bool: string_from_bool_id,
        string_from_float: string_from_float_id,
        prompt_call_int: prompt_call_int_id,
        prompt_call_bool: prompt_call_bool_id,
        prompt_call_float: prompt_call_float_id,
        prompt_call_string: prompt_call_string_id,
        literal_counter: std::cell::Cell::new(0),
        struct_destructors: HashMap::new(),
        struct_traces: HashMap::new(),
        struct_typeinfos: HashMap::new(),
        list_typeinfos: HashMap::new(),
        ir_types: HashMap::new(),
        ir_tools: HashMap::new(),
        tool_wrapper_ids: std::cell::RefCell::new(HashMap::new()),
        ir_prompts: HashMap::new(),
    })
}

/// Generate and define `corvid_destroy_<TypeName>(payload)` for a
/// struct type that has at least one refcounted field. The destructor
/// loads each refcounted field at its compile-time offset and calls
/// `corvid_release` on it. `corvid_release` then frees the struct's
/// own allocation after the destructor returns.
fn define_struct_destructor(
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

    module
        .define_function(func_id, &mut ctx)
        .map_err(|e| {
            CodegenError::cranelift(format!("define destructor `{symbol}`: {e}"), ty.span)
        })?;
    Ok(func_id)
}

/// Phase 17a — emit `corvid_trace_<TypeName>(payload, marker, ctx)` for
/// a refcounted struct type. Mirrors `define_struct_destructor` but
/// dispatches through an indirect marker function pointer on each
/// refcounted field instead of releasing it.
///
/// Trace fns are emitted for every refcounted struct — including
/// structs with zero refcounted fields — so the future (17d) mark
/// phase can dispatch uniformly without a per-object NULL check.
/// The linker folds duplicate empty bodies, so the cost is ~zero.
///
/// Marker signature: `fn(obj: i64, ctx: i64) -> ()`. Context-passing
/// (rather than stateless) so 17d's collector can thread a worklist
/// pointer through the walk without TLS or globals.
fn define_struct_trace(
    module: &mut ObjectModule,
    ty: &corvid_ir::IrType,
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

    module
        .define_function(func_id, &mut ctx)
        .map_err(|e| {
            CodegenError::cranelift(format!("define trace `{symbol}`: {e}"), ty.span)
        })?;
    Ok(func_id)
}

/// Phase 17a — on-disk typeinfo block layout. Must match exactly the
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

/// Emit `corvid_typeinfo_<TypeName>` as a .rodata data symbol with
/// function-pointer relocations to the type's destroy_fn (if any)
/// and trace_fn. Returns the DataId so allocations can reference it.
fn emit_struct_typeinfo(
    module: &mut ObjectModule,
    ty: &corvid_ir::IrType,
    destroy_fn: Option<FuncId>,
    trace_fn: FuncId,
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

    // flags: 0 in 17a (17e will set CYCLIC_CAPABLE for structs that
    // transitively self-reach; 17g will set HAS_WEAK_REFS).
    let flags: u32 = 0;
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

    module
        .define_data(data_id, &desc)
        .map_err(|e| {
            CodegenError::cranelift(format!("define typeinfo `{symbol}`: {e}"), ty.span)
        })?;
    Ok(data_id)
}

/// Phase 17a — emit `corvid_typeinfo_List_<elem>` for a concrete list
/// element type. Uses the runtime's shared `corvid_destroy_list` and
/// `corvid_trace_list` rather than per-type functions; the element
/// layout info lives entirely in `elem_typeinfo`.
///
/// `elem_typeinfo_data_id` is None for primitive-element lists
/// (List<Int>, List<Bool>, List<Float>); the runtime tracer checks
/// NULL and no-ops. Also sets destroy_fn=NULL for such lists —
/// `corvid_release` skips dispatch.
fn emit_list_typeinfo(
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
    let flags: u32 = TYPEINFO_FLAG_IS_LIST;
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

/// Stable, link-safe string from a Corvid `Type` for use in typeinfo
/// symbol names. `List<List<String>>` → `List_List_String`, etc.
fn mangle_type_name(ty: &Type) -> String {
    match ty {
        Type::Int => "Int".into(),
        Type::Float => "Float".into(),
        Type::Bool => "Bool".into(),
        Type::String => "String".into(),
        Type::Nothing => "Nothing".into(),
        Type::List(inner) => format!("List_{}", mangle_type_name(inner)),
        Type::Struct(def_id) => format!("Struct_{}", def_id.0),
        Type::Function { .. } => "Function".into(),
        Type::Unknown => "Unknown".into(),
    }
}

/// Phase 17a — walk every `Type::List(_)` the IR mentions (agent sigs,
/// struct fields, tool/prompt sigs, expression types) and produce the
/// set of unique list element types in a dependency-friendly order:
/// element types come before lists that contain them.
///
/// The returned `Vec<Type>` holds the *element* type of each list
/// (not the `List<T>` type itself). Emission iterates this vec
/// creating one `corvid_typeinfo_List_<elem>` per entry.
fn collect_list_element_types(ir: &IrFile) -> Vec<Type> {
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
    }
}

/// Helper: emit `corvid_retain(value)` if the value is refcounted
/// (i.e., non-immortal at runtime). Caller decides whether the value
/// needs ownership at this point — the helper just emits the call.
fn emit_retain(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    v: ClValue,
) {
    let callee = module.declare_func_in_func(runtime.retain, builder.func);
    builder.ins().call(callee, &[v]);
}

/// Helper: emit `corvid_release(value)`.
fn emit_release(
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
/// Slice 12e: only `String` is refcounted. Future slices add `Struct`
/// (12f), `List` (12g) — both will return true here.
fn is_refcounted_type(ty: &Type) -> bool {
    matches!(ty, Type::String | Type::Struct(_) | Type::List(_))
}

// ---- runtime helper symbols ----
//
// The C runtime in `runtime/{alloc,strings}.c` exports these symbols.
// `lower_file` declares them once per module as `Linkage::Import`; each
// per-function lowering uses `module.declare_func_in_func` to get a
// FuncRef, then `builder.ins().call`.

/// `void corvid_retain(void* payload)` — atomic refcount increment.
pub const RETAIN_SYMBOL: &str = "corvid_retain";

/// `void corvid_release(void* payload)` — atomic refcount decrement;
/// frees the underlying block when refcount hits zero.
pub const RELEASE_SYMBOL: &str = "corvid_release";

/// `void* corvid_string_concat(void* a, void* b)` — allocates a fresh
/// String (refcount = 1) containing `a` followed by `b`.
pub const STRING_CONCAT_SYMBOL: &str = "corvid_string_concat";

/// `long long corvid_string_eq(void* a, void* b)` — bytewise equality.
pub const STRING_EQ_SYMBOL: &str = "corvid_string_eq";

/// `long long corvid_string_cmp(void* a, void* b)` — bytewise compare.
pub const STRING_CMP_SYMBOL: &str = "corvid_string_cmp";

/// `void* corvid_alloc_typed(long long payload_bytes, const corvid_typeinfo* ti)`
/// — heap-allocate an N-byte payload behind a 16-byte typed header.
/// Phase 17a collapsed the old `corvid_alloc` + `corvid_alloc_with_destructor`
/// pair: every allocation now carries a typeinfo pointer, and
/// `corvid_release` dispatches through `typeinfo->destroy_fn` (NULL
/// = no refcounted children, equivalent to the old plain-alloc case).
pub const ALLOC_TYPED_SYMBOL: &str = "corvid_alloc_typed";

/// `void corvid_destroy_list(void* payload)` — shared runtime
/// destructor installed in every refcounted-element list type's
/// typeinfo. Walks length at offset 0 and `corvid_release`s each
/// element. Primitive-element lists leave `destroy_fn` NULL.
pub const LIST_DESTROY_SYMBOL: &str = "corvid_destroy_list";

/// `void corvid_trace_list(void*, void(*)(void*, void*), void*)` —
/// shared runtime tracer installed in every list type's typeinfo.
/// Reads its own typeinfo's `elem_typeinfo` to decide whether to
/// walk elements (NULL = primitive elements = no-op). Phase 17a
/// emits it for every list; 17d's mark phase will invoke it.
pub const LIST_TRACE_SYMBOL: &str = "corvid_trace_list";

/// Built-in `corvid_typeinfo_String` — the runtime provides this
/// symbol in `alloc.c`. Static string literals in `.rodata` and
/// runtime-internal String allocations both reference it so the
/// codegen doesn't have to emit a stray typeinfo per compilation
/// for string-less programs.
pub const STRING_TYPEINFO_SYMBOL: &str = "corvid_typeinfo_String";

// Slice 12i — entry-agent helpers (argv decoding, result printing,
// arity reporting, atexit). Called from the codegen-emitted `main`.

pub const ENTRY_INIT_SYMBOL: &str = "corvid_init";
pub const ENTRY_ARITY_MISMATCH_SYMBOL: &str = "corvid_arity_mismatch";
pub const PARSE_I64_SYMBOL: &str = "corvid_parse_i64";
pub const PARSE_F64_SYMBOL: &str = "corvid_parse_f64";
pub const PARSE_BOOL_SYMBOL: &str = "corvid_parse_bool";
pub const STRING_FROM_CSTR_SYMBOL: &str = "corvid_string_from_cstr";
pub const PRINT_I64_SYMBOL: &str = "corvid_print_i64";
pub const PRINT_BOOL_SYMBOL: &str = "corvid_print_bool";
pub const PRINT_F64_SYMBOL: &str = "corvid_print_f64";
pub const PRINT_STRING_SYMBOL: &str = "corvid_print_string";

// Phase 13 — async tool dispatch bridge. Signature in Rust:
//   corvid_tool_call_sync_int(name_ptr: *const u8, name_len: usize) -> i64
// Returns i64::MIN on error (tool-not-found, tool-errored, non-integer
// return). Phase 13 only supports the `() -> Int` tool signature;
// Phase 14 ships the generalised bridge with full JSON arg + return
// marshalling.
pub const TOOL_CALL_SYNC_INT_SYMBOL: &str = "corvid_tool_call_sync_int";

// Phase 15 — scalar-to-String stringification helpers. Used by the
// Cranelift codegen for `IrCallKind::Prompt` lowering when a
// non-String argument is interpolated into a prompt template. Each
// returns a refcount-1 Corvid String the caller must release.
pub const STRING_FROM_INT_SYMBOL: &str = "corvid_string_from_int";
pub const STRING_FROM_BOOL_SYMBOL: &str = "corvid_string_from_bool";
pub const STRING_FROM_FLOAT_SYMBOL: &str = "corvid_string_from_float";

// Phase 15 — typed prompt-dispatch bridges. One per return type;
// each takes 4 CorvidString args (prompt name, signature, rendered
// template, model) and returns the typed value. Built-in
// retry-with-validation + function-signature context — see the
// Rust-side implementations in `corvid-runtime::ffi_bridge`.
pub const PROMPT_CALL_INT_SYMBOL: &str = "corvid_prompt_call_int";
pub const PROMPT_CALL_BOOL_SYMBOL: &str = "corvid_prompt_call_bool";
pub const PROMPT_CALL_FLOAT_SYMBOL: &str = "corvid_prompt_call_float";
pub const PROMPT_CALL_STRING_SYMBOL: &str = "corvid_prompt_call_string";

// Phase 13 — runtime bridge init/shutdown called from `corvid_init`
// at the start of codegen-emitted `main` when the program uses any
// tool/prompt/approve construct. Tool-free programs skip these
// calls to preserve slice 12k's startup benchmark numbers.
pub const RUNTIME_INIT_SYMBOL: &str = "corvid_runtime_init";
pub const RUNTIME_SHUTDOWN_SYMBOL: &str = "corvid_runtime_shutdown";

/// Per-struct payload uses fixed 8-byte field slots for simple offset
/// math. Tight packing is a Phase-22 optimization.
pub const STRUCT_FIELD_SLOT_BYTES: i32 = 8;

/// Bytes per struct field when computing alloc size.
fn struct_payload_bytes(n_fields: usize) -> i64 {
    (n_fields as i64) * (STRUCT_FIELD_SLOT_BYTES as i64)
}

/// Bundle of imported runtime helper FuncIds, declared once per module
/// in `lower_file` and threaded through every lowering function.
/// Replaces the previous bare `overflow_func_id: FuncId` parameter so
/// call sites get every helper in one place.
///
/// `literal_counter` is a `Cell` so recursive lowering paths can take
/// `&self` and still bump the counter for unique `.rodata` symbol names.
pub struct RuntimeFuncs {
    pub overflow: FuncId,
    pub retain: FuncId,
    pub release: FuncId,
    pub string_concat: FuncId,
    pub string_eq: FuncId,
    pub string_cmp: FuncId,
    /// Phase 17a: single typed allocator replaces the pre-17a
    /// `alloc`/`alloc_with_destructor` pair. Signature:
    /// `(size: i64, typeinfo_ptr: i64) -> i64`.
    pub alloc_typed: FuncId,
    /// Phase 17a: shared runtime destructor installed in every
    /// refcounted-element list type's typeinfo. Replaces the
    /// pre-17a `list_destroy_refcounted`.
    pub list_destroy: FuncId,
    /// Phase 17a: shared runtime tracer installed in every list's
    /// typeinfo; 17d's mark phase will invoke it.
    pub list_trace: FuncId,
    /// Phase 17a: runtime-provided `corvid_typeinfo_String` data
    /// symbol. Imported so codegen can relocate its address into
    /// static string literals and List<String>'s elem_typeinfo slot.
    pub string_typeinfo: cranelift_module::DataId,
    // Slice 12i — entry helpers used by the codegen-emitted `main`.
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
    // Phase 13 — async tool bridge + runtime init/shutdown.
    pub tool_call_sync_int: FuncId,
    pub runtime_init: FuncId,
    pub runtime_shutdown: FuncId,
    // Phase 15 — scalar→String helpers for prompt-template interpolation.
    pub string_from_int: FuncId,
    pub string_from_bool: FuncId,
    pub string_from_float: FuncId,
    // Phase 15 — typed prompt bridges, one per return type.
    pub prompt_call_int: FuncId,
    pub prompt_call_bool: FuncId,
    pub prompt_call_float: FuncId,
    pub prompt_call_string: FuncId,
    pub literal_counter: std::cell::Cell<u64>,
    /// Per-struct-type destructors generated in `lower_file` for
    /// structs with at least one refcounted field. Missing entries
    /// mean the struct has no refcounted fields (typeinfo.destroy_fn
    /// stays NULL; corvid_release skips dispatch).
    pub struct_destructors: HashMap<DefId, FuncId>,
    /// Phase 17a — per-struct-type trace fns. Emitted for every
    /// refcounted struct type (including those with no refcounted
    /// fields — those trace fns are empty bodies, kept for uniform
    /// dispatch in 17d's mark phase without a per-object NULL check).
    pub struct_traces: HashMap<DefId, FuncId>,
    /// Phase 17a — per-struct-type typeinfo data symbols. Every
    /// refcounted struct allocation references its block via
    /// `corvid_alloc_typed(size, &typeinfo)`.
    pub struct_typeinfos: HashMap<DefId, cranelift_module::DataId>,
    /// Phase 17a — per-concrete-list-type typeinfo data symbols,
    /// keyed by the element `Type` (so `List<Int>` maps on `Type::Int`,
    /// `List<List<String>>` maps on `Type::List(Box::new(Type::String))`).
    /// Populated in `lower_file` by walking every `Type::List(_)` the
    /// IR mentions before agent bodies are lowered — so expression-
    /// level list literals just look up by element type.
    pub list_typeinfos: HashMap<Type, cranelift_module::DataId>,
    /// Owned copy of the IR's struct type metadata, keyed by `DefId`.
    /// Cloned into `RuntimeFuncs` in `lower_file` so the per-agent
    /// lowering functions can resolve struct layouts (for field
    /// offsets, constructor arity checks, destructor lookup) without
    /// threading `&IrFile` through every call site.
    pub ir_types: HashMap<DefId, corvid_ir::IrType>,
    /// Phase 14 — tool declarations, keyed by `DefId`. The codegen
    /// needs to know the declared signature (param types, return type)
    /// to emit a correctly-typed direct call to the `#[tool]` wrapper
    /// symbol. Cloned in from the `IrFile` the same way `ir_types` is.
    pub ir_tools: HashMap<DefId, corvid_ir::IrTool>,
    /// Phase 14 — cache of imported `__corvid_tool_<name>` FuncIds so
    /// repeated calls to the same tool re-use one declaration. First
    /// sight declares; later sights re-use.
    pub tool_wrapper_ids: std::cell::RefCell<HashMap<DefId, FuncId>>,
    /// Phase 15 — prompt declarations, keyed by `DefId`. Codegen reads
    /// each prompt's params + template + return type to emit
    /// signature-aware bridge calls.
    pub ir_prompts: HashMap<DefId, corvid_ir::IrPrompt>,
}

impl RuntimeFuncs {
    /// Allocate the next unique literal symbol number.
    pub fn next_literal_id(&self) -> u64 {
        let n = self.literal_counter.get();
        self.literal_counter.set(n + 1);
        n
    }
}

/// Loop context entry recorded on the `loop_stack` at `for` entry,
/// consumed by `break` / `continue` statements nested inside.
#[derive(Clone, Copy)]
pub struct LoopCtx {
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
pub struct LocalsCtx {
    /// Bound locals: id → (Cranelift variable, declared width).
    pub env: HashMap<LocalId, (Variable, clir::Type)>,
    /// Monotonic Variable id counter — unique per agent.
    pub var_idx: usize,
    /// Stack of nested scopes, innermost on top. Each scope holds the
    /// refcounted locals declared *in that scope*.
    pub scope_stack: Vec<Vec<(LocalId, Variable)>>,
}

impl LocalsCtx {
    pub fn new() -> Self {
        Self {
            env: HashMap::new(),
            var_idx: 0,
            scope_stack: Vec::new(),
        }
    }

    /// Push a fresh scope onto the stack. Call at every block entry.
    pub fn enter_scope(&mut self) {
        self.scope_stack.push(Vec::new());
    }

    /// Pop the current scope and return its refcounted locals so the
    /// caller can emit `release` calls *before* the block terminator.
    pub fn exit_scope(&mut self) -> Vec<(LocalId, Variable)> {
        self.scope_stack.pop().unwrap_or_default()
    }

    /// Register a refcounted local in the current scope. Called from
    /// `IrStmt::Let` when a *new* binding (not reassignment) is made
    /// for a String / Struct / List type.
    pub fn track_refcounted(&mut self, local_id: LocalId, var: Variable) {
        if let Some(top) = self.scope_stack.last_mut() {
            top.push((local_id, var));
        }
    }

    /// Iterate over every refcounted local across all scopes,
    /// innermost first. Used by `IrStmt::Return` to release all live
    /// locals before transferring the return value to the caller.
    pub fn all_refcounted_innermost_first(&self) -> impl Iterator<Item = &(LocalId, Variable)> {
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
pub const OVERFLOW_HANDLER_SYMBOL: &str = "corvid_runtime_overflow";

/// Stable symbol the C shim calls into. The codegen emits a trampoline
/// with this name that forwards to the user-chosen entry agent. Keeps
/// the shim source constant regardless of what the user named their
/// agent.
pub const ENTRY_TRAMPOLINE_SYMBOL: &str = "corvid_entry";

/// Lower every agent in `ir` into `module`, plus a `corvid_entry`
/// trampoline that calls `entry_agent_name` and returns its result.
/// Returns a name-indexed map of function ids so the driver can
/// confirm the entry agent is present.
pub fn lower_file(
    ir: &IrFile,
    module: &mut ObjectModule,
    entry_agent_name: Option<&str>,
) -> Result<HashMap<String, FuncId>, CodegenError> {
    let mut func_ids: HashMap<String, FuncId> = HashMap::new();
    let mut func_ids_by_def: HashMap<DefId, FuncId> = HashMap::new();

    // Declare the runtime overflow handler as an imported symbol: void(void).
    let mut overflow_sig = module.make_signature();
    overflow_sig.params.clear();
    overflow_sig.returns.clear();
    let overflow_func_id = module
        .declare_function(OVERFLOW_HANDLER_SYMBOL, Linkage::Import, &overflow_sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare overflow handler: {e}"), Span::new(0, 0))
        })?;

    // Declare the refcount + string runtime helpers. All take and
    // return `i64` (pointers / integers); we use I64 across the board
    // so the calling convention is uniform.
    let mut runtime = declare_runtime_funcs(module, overflow_func_id)?;

    // Populate the IR type registry so lowering functions can resolve
    // field offsets and constructor arities without threading `&IrFile`.
    for ty in &ir.types {
        runtime.ir_types.insert(ty.id, ty.clone());
    }

    // Phase 14 — same pattern for tool declarations so the Cranelift
    // `IrCallKind::Tool` lowering can look up param + return types and
    // declare the matching `__corvid_tool_<name>` wrapper import.
    for tool in &ir.tools {
        runtime.ir_tools.insert(tool.id, tool.clone());
    }

    // Phase 15 — same pattern for prompt declarations. `IrCallKind::Prompt`
    // lowering reads each prompt's params + template + return type to
    // emit signature-aware bridge calls.
    for prompt in &ir.prompts {
        runtime.ir_prompts.insert(prompt.id, prompt.clone());
    }

    // Phase 17a — emit per-type metadata in dependency order:
    //
    //   1. Struct destructors (existing): release refcounted fields
    //      when rc→0. Only for structs with refcounted fields.
    //   2. Struct trace fns (new in 17a): walk refcounted fields for
    //      17d's mark phase. Emitted for every refcounted struct —
    //      even ones with no refcounted fields get an empty-body
    //      trace so dispatch is uniform (linker folds duplicates).
    //   3. Struct typeinfo blocks (new): .rodata record referenced
    //      from the allocation header. Relocations point at the
    //      destructor + trace fns emitted above.
    //   4. List typeinfo blocks (new): one per concrete List<T> type
    //      walked out of the IR. Element-types emit first so outer
    //      list typeinfos can reference them via elem_typeinfo.
    //
    // All must land before agent bodies are lowered so
    // IrCallKind::StructConstructor and IrExprKind::List have
    // typeinfos to reference at allocation sites.

    // Structs: destructors (only for refcounted fields), traces
    // (every struct, empty body if no refcounted fields — linker
    // folds duplicates), typeinfos (every struct, uniform allocation
    // path). The pre-17a "primitive-only structs skip typeinfo"
    // short-circuit is gone — uniformity means 17d doesn't need a
    // special case for them in the mark phase.
    for ty in &ir.types {
        let has_refcounted_field = ty.fields.iter().any(|f| is_refcounted_type(&f.ty));
        let destroy_id = if has_refcounted_field {
            let id = define_struct_destructor(module, ty, &runtime)?;
            runtime.struct_destructors.insert(ty.id, id);
            Some(id)
        } else {
            None
        };
        let trace_id = define_struct_trace(module, ty)?;
        runtime.struct_traces.insert(ty.id, trace_id);
        let typeinfo_id = emit_struct_typeinfo(module, ty, destroy_id, trace_id)?;
        runtime.struct_typeinfos.insert(ty.id, typeinfo_id);
    }

    // Lists: collect every concrete element type the IR mentions, in
    // dependency order (inner before outer), then emit one typeinfo
    // per concrete list type. For refcounted element types the
    // elem_typeinfo slot gets a relocation to the element's typeinfo
    // (String built-in, struct, or nested list).
    let list_elem_types = collect_list_element_types(ir);
    for elem_ty in list_elem_types {
        let elem_typeinfo_data_id = match &elem_ty {
            Type::String => Some(runtime.string_typeinfo),
            Type::Struct(def_id) => runtime.struct_typeinfos.get(def_id).copied(),
            Type::List(inner_inner) => {
                // Nested list: look up the already-emitted typeinfo
                // for the inner list's element type.
                runtime.list_typeinfos.get(&(**inner_inner)).copied()
            }
            // Primitive elements: NULL elem_typeinfo means "don't
            // trace" — prevents List<Int>'s Int slots from being
            // mis-interpreted as pointers.
            _ => None,
        };
        let list_ti_id =
            emit_list_typeinfo(module, &elem_ty, elem_typeinfo_data_id, &runtime)?;
        runtime.list_typeinfos.insert(elem_ty, list_ti_id);
    }

    // Pass 1: declare every agent. Signatures are Int + Bool only for
    // slice 12b. Symbols are mangled so user agent names (e.g. `main`,
    // `printf`, `malloc`) cannot collide with C runtime symbols at link
    // time. Only the trampoline is exported (`corvid_entry`).
    for agent in &ir.agents {
        reject_unsupported_types(agent)?;
        let mut sig = module.make_signature();
        for p in &agent.params {
            sig.params.push(AbiParam::new(cl_type_for(&p.ty, p.span)?));
        }
        sig.returns
            .push(AbiParam::new(cl_type_for(&agent.return_ty, agent.span)?));
        let mangled = mangle_agent_symbol(&agent.name, agent.id);
        let id = module
            .declare_function(&mangled, Linkage::Local, &sig)
            .map_err(|e| {
                CodegenError::cranelift(
                    format!("declare agent `{}` (as `{mangled}`): {e}", agent.name),
                    agent.span,
                )
            })?;
        func_ids.insert(agent.name.clone(), id);
        func_ids_by_def.insert(agent.id, id);
    }

    // Pass 2: define each agent's body.
    for agent in &ir.agents {
        let &func_id = func_ids_by_def
            .get(&agent.id)
            .expect("declared in pass 1");
        define_agent(module, agent, func_id, &func_ids_by_def, &runtime)?;
    }

    // Pass 3: emit the `corvid_entry` trampoline if an entry agent was
    // requested. It calls the chosen agent (must be parameter-less and
    // Int-returning, checked by the driver) and returns its value.
    if let Some(entry_name) = entry_agent_name {
        let entry_agent = ir
            .agents
            .iter()
            .find(|a| a.name == entry_name)
            .ok_or_else(|| {
                CodegenError::cranelift(
                    format!("entry agent `{entry_name}` not present in IR"),
                    Span::new(0, 0),
                )
            })?;
        let entry_func_id = *func_ids_by_def
            .get(&entry_agent.id)
            .expect("declared in pass 1");
        // Phase 13: codegen-emitted main calls `corvid_runtime_init()`
        // and registers `corvid_runtime_shutdown` via atexit ONLY if the
        // program actually uses the async runtime. Pure-computation
        // programs skip these calls to preserve the slice 12k startup
        // benchmark numbers — multi-thread tokio startup is ~5-10ms on
        // Windows, which would otherwise regress every tool-free
        // `corvid run` invocation.
        let uses_runtime = ir_uses_runtime(ir);
        emit_entry_main(
            module,
            entry_agent,
            entry_func_id,
            &runtime,
            uses_runtime,
        )?;
    }

    Ok(func_ids)
}

/// Does this IR contain any construct that needs the async runtime
/// bridge at execution time? Tool calls, prompt calls, and approve
/// statements all route through the tokio runtime.
fn ir_uses_runtime(ir: &IrFile) -> bool {
    ir.agents.iter().any(|a| block_uses_runtime(&a.body))
}

fn block_uses_runtime(block: &IrBlock) -> bool {
    block.stmts.iter().any(stmt_uses_runtime)
}

fn stmt_uses_runtime(stmt: &IrStmt) -> bool {
    match stmt {
        IrStmt::Let { value, .. } => expr_uses_runtime(value),
        IrStmt::Return { value: Some(e), .. } => expr_uses_runtime(e),
        IrStmt::Return { value: None, .. } => false,
        IrStmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            expr_uses_runtime(cond)
                || block_uses_runtime(then_block)
                || else_block
                    .as_ref()
                    .map(block_uses_runtime)
                    .unwrap_or(false)
        }
        IrStmt::For { iter, body, .. } => expr_uses_runtime(iter) || block_uses_runtime(body),
        IrStmt::Approve { .. } => true,
        IrStmt::Expr { expr, .. } => expr_uses_runtime(expr),
        IrStmt::Break { .. } | IrStmt::Continue { .. } | IrStmt::Pass { .. } => false,
    }
}

fn expr_uses_runtime(expr: &IrExpr) -> bool {
    match &expr.kind {
        IrExprKind::Literal(_) | IrExprKind::Local { .. } | IrExprKind::Decl { .. } => false,
        IrExprKind::Call { kind, args, .. } => {
            let self_needs = matches!(kind, IrCallKind::Tool { .. } | IrCallKind::Prompt { .. });
            self_needs || args.iter().any(expr_uses_runtime)
        }
        IrExprKind::FieldAccess { target, .. } => expr_uses_runtime(target),
        IrExprKind::Index { target, index } => {
            expr_uses_runtime(target) || expr_uses_runtime(index)
        }
        IrExprKind::BinOp { left, right, .. } => {
            expr_uses_runtime(left) || expr_uses_runtime(right)
        }
        IrExprKind::UnOp { operand, .. } => expr_uses_runtime(operand),
        IrExprKind::List { items } => items.iter().any(expr_uses_runtime),
    }
}

/// Emit a signature-aware `int main(int argc, char** argv)` that:
///
///   1. Calls `corvid_init()` to register the leak-counter atexit.
///   2. Verifies `argc - 1 == n_params`, calling `corvid_arity_mismatch`
///      and aborting if not.
///   3. For each entry parameter, decodes `argv[i+1]` via the
///      appropriate parse helper (or `corvid_string_from_cstr` for
///      String parameters).
///   4. Calls the entry agent.
///   5. Prints the result via the matching helper.
///   6. Returns 0.
///
/// Replaces the slice-12a `corvid_entry` trampoline. Now that the
/// codegen knows the entry signature at emit time, generating `main`
/// directly avoids the C-shim-with-introspection trap.
fn emit_entry_main(
    module: &mut ObjectModule,
    entry_agent: &IrAgent,
    entry_func_id: FuncId,
    runtime: &RuntimeFuncs,
    // Phase 13: emit `corvid_runtime_init()` + `atexit(corvid_runtime_shutdown)`
    // only if the program actually needs the async runtime. Passing `false`
    // keeps compiled binaries as small + fast-starting as they were in
    // slice 12k.
    uses_runtime: bool,
) -> Result<(), CodegenError> {
    // I32 is imported at file scope since Phase 13 needs it in
    // `declare_runtime_funcs` too.

    // Validate that every entry parameter and the return type are
    // representable at the command-line / stdout boundary. Struct and
    // List are deliberately excluded — they need a serialization slice.
    for p in &entry_agent.params {
        check_entry_boundary_type(&p.ty, p.span, "parameter")?;
    }
    check_entry_boundary_type(&entry_agent.return_ty, entry_agent.span, "return")?;

    // main signature: (i32 argc, i64 argv) -> i32
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(I32));
    sig.params.push(AbiParam::new(I64));
    sig.returns.push(AbiParam::new(I32));
    let main_id = module
        .declare_function("main", Linkage::Export, &sig)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare main: {e}"), entry_agent.span)
        })?;

    let mut ctx = Context::new();
    ctx.func = Function::with_name_signature(
        UserFuncName::user(0, main_id.as_u32()),
        module.declarations().get_function_decl(main_id).signature.clone(),
    );

    let mut bctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut bctx);
        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        builder.switch_to_block(entry_block);
        builder.seal_block(entry_block);

        let argc_i32 = builder.block_params(entry_block)[0];
        let argv = builder.block_params(entry_block)[1];

        // 1. corvid_init() — registers atexit handler for leak counters.
        let init_ref = module.declare_func_in_func(runtime.entry_init, builder.func);
        builder.ins().call(init_ref, &[]);

        // 1a. (Phase 13) If the program uses the async runtime, build
        // the tokio + corvid runtime globals NOW, eagerly. Shutdown is
        // registered via `atexit` so worker threads join cleanly at
        // exit. Shutdown runs BEFORE the leak-counter atexit (atexit
        // is LIFO), so any refcount activity from the runtime settles
        // before the counter prints — that's the intended ordering.
        if uses_runtime {
            let rt_init_ref =
                module.declare_func_in_func(runtime.runtime_init, builder.func);
            builder.ins().call(rt_init_ref, &[]);

            // Register corvid_runtime_shutdown via libc atexit. atexit
            // itself isn't in RuntimeFuncs because nothing else needs
            // it; declare it inline. Signature: int atexit(void (*)(void)).
            let mut atexit_sig = module.make_signature();
            atexit_sig.params.push(AbiParam::new(I64));
            atexit_sig.returns.push(AbiParam::new(I32));
            let atexit_id = module
                .declare_function("atexit", Linkage::Import, &atexit_sig)
                .map_err(|e| {
                    CodegenError::cranelift(
                        format!("declare atexit: {e}"),
                        entry_agent.span,
                    )
                })?;
            let atexit_ref = module.declare_func_in_func(atexit_id, builder.func);
            let shutdown_ref =
                module.declare_func_in_func(runtime.runtime_shutdown, builder.func);
            let shutdown_addr = builder.ins().func_addr(I64, shutdown_ref);
            builder.ins().call(atexit_ref, &[shutdown_addr]);
        }

        // 2. Arity check.
        let n_params = entry_agent.params.len() as i64;
        let argc_i64 = builder.ins().sextend(I64, argc_i32);
        let expected = builder.ins().iconst(I64, n_params + 1);
        let matches = builder.ins().icmp(IntCC::Equal, argc_i64, expected);
        let proceed_b = builder.create_block();
        let mismatch_b = builder.create_block();
        builder.ins().brif(matches, proceed_b, &[], mismatch_b, &[]);

        // mismatch path: call the helper, trap (it never returns).
        builder.switch_to_block(mismatch_b);
        builder.seal_block(mismatch_b);
        let arity_ref =
            module.declare_func_in_func(runtime.entry_arity_mismatch, builder.func);
        let expected_const = builder.ins().iconst(I64, n_params);
        let got = builder.ins().iadd_imm(argc_i64, -1);
        builder.ins().call(arity_ref, &[expected_const, got]);
        builder
            .ins()
            .trap(cranelift_codegen::ir::TrapCode::INTEGER_OVERFLOW);

        // 3. Decode each parameter from argv[i+1].
        builder.switch_to_block(proceed_b);
        builder.seal_block(proceed_b);
        let mut decoded_args: Vec<ClValue> = Vec::with_capacity(entry_agent.params.len());
        let mut decoded_refcounted: Vec<bool> = Vec::with_capacity(entry_agent.params.len());
        for (i, p) in entry_agent.params.iter().enumerate() {
            // Load argv[i+1] (a C string pointer).
            let cstr = builder.ins().load(
                I64,
                cranelift_codegen::ir::MemFlags::trusted(),
                argv,
                ((i as i32) + 1) * 8,
            );
            let argv_index = builder.ins().iconst(I64, (i as i64) + 1);
            let value = match &p.ty {
                Type::Int => {
                    let r =
                        module.declare_func_in_func(runtime.parse_i64, builder.func);
                    let c = builder.ins().call(r, &[cstr, argv_index]);
                    builder.inst_results(c)[0]
                }
                Type::Float => {
                    let r =
                        module.declare_func_in_func(runtime.parse_f64, builder.func);
                    let c = builder.ins().call(r, &[cstr, argv_index]);
                    builder.inst_results(c)[0]
                }
                Type::Bool => {
                    let r =
                        module.declare_func_in_func(runtime.parse_bool, builder.func);
                    let c = builder.ins().call(r, &[cstr, argv_index]);
                    builder.inst_results(c)[0]
                }
                Type::String => {
                    let r = module
                        .declare_func_in_func(runtime.string_from_cstr, builder.func);
                    let c = builder.ins().call(r, &[cstr]);
                    builder.inst_results(c)[0]
                }
                _ => {
                    // Already rejected by check_entry_boundary_type above;
                    // unreachable in practice.
                    return Err(CodegenError::cranelift(
                        format!(
                            "entry parameter `{}` of type `{}` reached lowering despite the boundary check",
                            p.name,
                            p.ty.display_name()
                        ),
                        p.span,
                    ));
                }
            };
            decoded_args.push(value);
            decoded_refcounted.push(is_refcounted_type(&p.ty));
        }

        // 4. Call the entry agent.
        let entry_ref = module.declare_func_in_func(entry_func_id, builder.func);
        let entry_call = builder.ins().call(entry_ref, &decoded_args);
        let result = builder.inst_results(entry_call).first().copied();

        // Release each refcounted argument's +1 — the callee took its
        // own ownership via parameter retain (per the +0 ABI).
        for (v, is_ref) in decoded_args.iter().zip(decoded_refcounted.iter()) {
            if *is_ref {
                emit_release(&mut builder, module, runtime, *v);
            }
        }

        // 5. Print the result based on return type.
        if let Some(result_val) = result {
            match &entry_agent.return_ty {
                Type::Int => {
                    let r = module.declare_func_in_func(runtime.print_i64, builder.func);
                    builder.ins().call(r, &[result_val]);
                }
                Type::Bool => {
                    let widened = builder.ins().uextend(I64, result_val);
                    let r =
                        module.declare_func_in_func(runtime.print_bool, builder.func);
                    builder.ins().call(r, &[widened]);
                }
                Type::Float => {
                    let r = module.declare_func_in_func(runtime.print_f64, builder.func);
                    builder.ins().call(r, &[result_val]);
                }
                Type::String => {
                    let r =
                        module.declare_func_in_func(runtime.print_string, builder.func);
                    builder.ins().call(r, &[result_val]);
                    // Release the entry's returned String (Owned +1).
                    emit_release(&mut builder, module, runtime, result_val);
                }
                _ => unreachable!("boundary check rejected non-printable returns"),
            }
        }

        // 6. Return 0.
        let zero = builder.ins().iconst(I32, 0);
        builder.ins().return_(&[zero]);
        builder.finalize();
    }

    module
        .define_function(main_id, &mut ctx)
        .map_err(|e| {
            CodegenError::cranelift(format!("define main: {e}"), entry_agent.span)
        })?;
    Ok(())
}

/// Validate that a type is one of the four supported at the
/// command-line / stdout boundary. Struct and List need a
/// dedicated serialization slice; Nothing isn't a sensible
/// CLI value either.
fn check_entry_boundary_type(
    ty: &Type,
    span: Span,
    role: &str,
) -> Result<(), CodegenError> {
    match ty {
        Type::Int | Type::Bool | Type::Float | Type::String => Ok(()),
        Type::Struct(_) | Type::List(_) | Type::Nothing => {
            Err(CodegenError::not_supported(
                format!(
                    "entry agent {role} of type `{}` — slice 12i supports `Int` / `Bool` / `Float` / `String` only at the command-line boundary; structured types arrive in a future serialization slice (use a wrapper agent that converts internally)",
                    ty.display_name()
                ),
                span,
            ))
        }
        Type::Function { .. } | Type::Unknown => Err(CodegenError::cranelift(
            format!("entry agent {role} has un-printable type `{}`", ty.display_name()),
            span,
        )),
    }
}

fn reject_unsupported_types(agent: &IrAgent) -> Result<(), CodegenError> {
    for p in &agent.params {
        cl_type_for(&p.ty, p.span).map_err(|_| {
            CodegenError::not_supported(
                format!(
                    "parameter `{}: {}` — slice 12d supports `Int`, `Bool`, and `Float` (`String` / `Struct` / `List` land in slices 12e–g)",
                    p.name,
                    p.ty.display_name()
                ),
                p.span,
            )
        })?;
    }
    cl_type_for(&agent.return_ty, agent.span).map_err(|_| {
        CodegenError::not_supported(
            format!(
                "agent `{}` returns `{}` — slice 12d supports `Int`, `Bool`, and `Float` returns",
                agent.name,
                agent.return_ty.display_name()
            ),
            agent.span,
        )
    })?;
    Ok(())
}

/// Map a Corvid `Type` to the Cranelift IR type width we compile it to.
/// `Int` → `I64`, `Bool` → `I8`. Everything else raises
/// `CodegenError::NotSupported` with a pointer to the slice that
/// introduces it.
fn cl_type_for(ty: &Type, span: Span) -> Result<clir::Type, CodegenError> {
    match ty {
        Type::Int => Ok(I64),
        Type::Bool => Ok(I8),
        Type::Float => Ok(F64),
        // String is a pointer to a 16-byte descriptor that lives behind
        // a 16-byte refcount header. Single I64 in registers/env keeps
        // the calling convention uniform with future Struct/List types.
        Type::String => Ok(I64),
        // Struct values are descriptor pointers (like String) — single
        // I64 in registers/env. The actual layout lives behind the
        // refcount header: `[header (16) | field0 (8) | ... | fieldN (8)]`.
        Type::Struct(_) => Ok(I64),
        // List values are descriptor pointers (like String, Struct) —
        // single I64 pointing at `[length (8) | elements...]` after the
        // 16-byte refcount header.
        Type::List(_) => Ok(I64),
        Type::Nothing => Err(CodegenError::not_supported(
            "`Nothing` — slice 12d pairs it with bare `return`",
            span,
        )),
        Type::Function { .. } => Err(CodegenError::not_supported(
            "function types as values — Phase 14 revisits first-class callables",
            span,
        )),
        Type::Unknown => Err(CodegenError::cranelift(
            "encountered `Unknown` type at codegen — typecheck should have caught this",
            span,
        )),
    }
}

fn define_agent(
    module: &mut ObjectModule,
    agent: &IrAgent,
    func_id: FuncId,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    runtime: &RuntimeFuncs,
) -> Result<(), CodegenError> {
    let mut ctx = Context::new();
    ctx.func = Function::with_name_signature(
        UserFuncName::user(0, func_id.as_u32()),
        module.declarations().get_function_decl(func_id).signature.clone(),
    );

    let mut builder_ctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);

        let mut env: HashMap<LocalId, (Variable, clir::Type)> = HashMap::new();
        let mut var_idx: usize = 0;
        // The function-root scope. `lower_block` for the agent body
        // does NOT push its own scope (it would double-push); this is
        // it. Branch blocks inside `if`/`else` push/pop their own.
        let mut scope_stack: Vec<Vec<(LocalId, Variable)>> = vec![Vec::new()];
        // Loop context stack — empty at function entry. `for` pushes
        // on entry, `break`/`continue` consult the top entry, `for`
        // pops on exit.
        let mut loop_stack: Vec<LoopCtx> = Vec::new();

        for (i, p) in agent.params.iter().enumerate() {
            let block_arg = builder.block_params(entry)[i];
            let var = Variable::from_u32(var_idx as u32);
            var_idx += 1;
            let ty = cl_type_for(&p.ty, p.span)?;
            builder.declare_var(var, ty);
            builder.def_var(var, block_arg);
            env.insert(p.local_id, (var, ty));
            // +0 ABI: caller passed without bumping refcount. Callee
            // takes ownership by retaining and tracking in the
            // function-root scope. Symmetric: scope exit releases.
            if is_refcounted_type(&p.ty) {
                emit_retain(&mut builder, module, runtime, block_arg);
                scope_stack[0].push((p.local_id, var));
            }
        }

        let lowered = lower_block(
            &mut builder,
            &agent.body,
            &mut env,
            &mut var_idx,
            &mut scope_stack,
            &mut loop_stack,
            func_ids_by_def,
            module,
            runtime,
        );
        if let Err(e) = lowered {
            return Err(e);
        }

        // Defensive fallthrough terminator. `lower_block` returns
        // `BlockOutcome::Terminated` after emitting a `return`, so we
        // only reach here for agents whose type checker somehow allowed
        // a missing return (shouldn't happen for `Int` agents). Terminate
        // with a trap so the verifier doesn't reject the function.
        match lowered_outcome_placeholder() {
            _ => {
                if builder.current_block().is_some() {
                    let cur = builder.current_block().unwrap();
                    if !builder.func.layout.is_block_inserted(cur) {
                        // Should not happen — a block was switched to but
                        // not inserted. Terminator still needed.
                    }
                    // If Cranelift considers the block unterminated,
                    // trap — an untyped fallthrough on an `Int` return
                    // is already a frontend bug.
                    let last_inst = builder.func.layout.last_inst(cur);
                    let terminated = last_inst
                        .map(|i| builder.func.dfg.insts[i].opcode().is_terminator())
                        .unwrap_or(false);
                    if !terminated {
                        builder
                            .ins()
                            .trap(cranelift_codegen::ir::TrapCode::INTEGER_OVERFLOW);
                    }
                }
            }
        }

        builder.finalize();
    }

    module
        .define_function(func_id, &mut ctx)
        .map_err(|e| CodegenError::cranelift(format!("define agent `{}`: {e}", agent.name), agent.span))?;
    Ok(())
}

#[derive(Clone, Copy)]
enum BlockOutcome {
    Normal,
    Terminated,
}

fn lower_block(
    builder: &mut FunctionBuilder,
    block: &IrBlock,
    env: &mut HashMap<LocalId, (Variable, clir::Type)>,
    var_idx: &mut usize,
    scope_stack: &mut Vec<Vec<(LocalId, Variable)>>,
    loop_stack: &mut Vec<LoopCtx>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<BlockOutcome, CodegenError> {
    for stmt in &block.stmts {
        match lower_stmt(
            builder,
            stmt,
            env,
            var_idx,
            scope_stack,
            loop_stack,
            func_ids_by_def,
            module,
            runtime,
        )? {
            BlockOutcome::Terminated => return Ok(BlockOutcome::Terminated),
            BlockOutcome::Normal => {}
        }
    }
    Ok(BlockOutcome::Normal)
}

fn lower_stmt(
    builder: &mut FunctionBuilder,
    stmt: &IrStmt,
    env: &mut HashMap<LocalId, (Variable, clir::Type)>,
    var_idx: &mut usize,
    scope_stack: &mut Vec<Vec<(LocalId, Variable)>>,
    loop_stack: &mut Vec<LoopCtx>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<BlockOutcome, CodegenError> {
    match stmt {
        IrStmt::Return { value, span } => {
            let value_ty = value.as_ref().map(|e| e.ty.clone());
            let v = match value {
                Some(e) => lower_expr(builder, e, env, func_ids_by_def, module, runtime)?,
                None => {
                    return Err(CodegenError::not_supported(
                        "bare `return` (Nothing type not supported in slice 12a)",
                        *span,
                    ));
                }
            };
            // The return value is an Owned temp (per the three-state
            // ownership model — every `lower_expr` returns Owned for
            // refcounted types). The caller will receive the +1 we
            // hold; nothing more to do for the value itself.
            //
            // Release every refcounted local across all live scopes
            // before transferring control. Walk innermost-first to
            // mirror lexical scope exit order (matters only if the
            // `release` call has side effects we care about, which it
            // doesn't, but the ordering is conventional).
            let _ = value_ty; // currently unused but shows intent
            for scope in scope_stack.iter().rev() {
                for (_, var) in scope.iter().rev() {
                    let v_local = builder.use_var(*var);
                    emit_release(builder, module, runtime, v_local);
                }
            }
            builder.ins().return_(&[v]);
            Ok(BlockOutcome::Terminated)
        }
        IrStmt::Expr { expr, .. } => {
            let v = lower_expr(builder, expr, env, func_ids_by_def, module, runtime)?;
            // Discarded statement-expression: if the value is a
            // refcounted Owned temp, it has no owner — release it.
            if is_refcounted_type(&expr.ty) {
                emit_release(builder, module, runtime, v);
            }
            Ok(BlockOutcome::Normal)
        }
        IrStmt::Let {
            local_id,
            ty,
            value,
            span,
            ..
        } => {
            let cl_ty = cl_type_for(ty, *span)?;
            let refcounted = is_refcounted_type(ty);
            // Declare-or-reuse: a fresh `LocalId` gets a new Cranelift
            // `Variable`; reassignment to a name already bound in this
            // function reuses the existing Variable. A type change on
            // reassignment is a typechecker bug — we surface it as a
            // clean `CodegenError` instead of letting Cranelift panic.
            let (var, is_reassignment) = match env.get(local_id) {
                Some(&(existing_var, existing_ty)) => {
                    if existing_ty != cl_ty {
                        return Err(CodegenError::cranelift(
                            format!(
                                "variable redeclared with different type: was {existing_ty}, now {cl_ty} — typechecker should have caught this"
                            ),
                            *span,
                        ));
                    }
                    (existing_var, true)
                }
                None => {
                    let new_var = Variable::from_u32(*var_idx as u32);
                    *var_idx += 1;
                    builder.declare_var(new_var, cl_ty);
                    env.insert(*local_id, (new_var, cl_ty));
                    (new_var, false)
                }
            };
            // For reassignment of a refcounted local: read the old
            // value first, release it, THEN bind the new value (which
            // came pre-Owned from `lower_expr`).
            if refcounted && is_reassignment {
                let old = builder.use_var(var);
                emit_release(builder, module, runtime, old);
            }
            let v = lower_expr(builder, value, env, func_ids_by_def, module, runtime)?;
            builder.def_var(var, v);
            // Track this binding in the current scope so it gets
            // released at scope exit. Only on first binding — a
            // reassignment is already tracked by the original Let.
            if refcounted && !is_reassignment {
                if let Some(top) = scope_stack.last_mut() {
                    top.push((*local_id, var));
                }
            }
            Ok(BlockOutcome::Normal)
        }
        IrStmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => lower_if(
            builder,
            cond,
            then_block,
            else_block.as_ref(),
            env,
            var_idx,
            scope_stack,
            loop_stack,
            func_ids_by_def,
            module,
            runtime,
        ),
        IrStmt::For {
            var_local,
            iter,
            body,
            span,
            ..
        } => lower_for(
            builder,
            *var_local,
            iter,
            body,
            *span,
            env,
            var_idx,
            scope_stack,
            loop_stack,
            func_ids_by_def,
            module,
            runtime,
        ),
        IrStmt::Approve { args, .. } => {
            // Phase 14: `approve` compiles to a no-op. The effect
            // checker (Phase 5) statically verifies that every
            // dangerous-tool call is preceded by a matching approve
            // — that's Corvid's primary enforcement mechanism and
            // it already runs before codegen. Runtime approve
            // verification (belt-and-braces against malicious IR
            // that bypasses the checker) lands in Phase 20 alongside
            // the rest of the effect-row machinery — at that point
            // approve gains a full runtime stack with typed args.
            // Today we still lower the arg expressions so their side
            // effects + refcount work happens (an approve with heap
            // String args in its argument position must still
            // release those Strings at end of scope), we just don't
            // push anything runtime-side.
            for a in args {
                let v = lower_expr(
                    builder,
                    a,
                    env,
                    func_ids_by_def,
                    module,
                    runtime,
                )?;
                if is_refcounted_type(&a.ty) {
                    emit_release(builder, module, runtime, v);
                }
            }
            Ok(BlockOutcome::Normal)
        }
        IrStmt::Pass { .. } => Ok(BlockOutcome::Normal),
        IrStmt::Break { span } => lower_break_or_continue(
            builder,
            true,
            *span,
            scope_stack,
            loop_stack,
            module,
            runtime,
        ),
        IrStmt::Continue { span } => lower_break_or_continue(
            builder,
            false,
            *span,
            scope_stack,
            loop_stack,
            module,
            runtime,
        ),
    }
}

fn lower_expr(
    builder: &mut FunctionBuilder,
    expr: &IrExpr,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<ClValue, CodegenError> {
    match &expr.kind {
        IrExprKind::Literal(IrLiteral::Int(n)) => Ok(builder.ins().iconst(I64, *n)),
        IrExprKind::Literal(IrLiteral::Bool(b)) => {
            Ok(builder.ins().iconst(I8, if *b { 1 } else { 0 }))
        }
        IrExprKind::Literal(IrLiteral::Float(n)) => Ok(builder.ins().f64const(*n)),
        IrExprKind::Literal(IrLiteral::String(s)) => {
            lower_string_literal(builder, module, runtime, s, expr.span)
        }
        IrExprKind::Literal(IrLiteral::Nothing) => Err(CodegenError::not_supported(
            "`nothing` literal — slice 12d adds it alongside the `Nothing` type",
            expr.span,
        )),
        IrExprKind::Local { local_id, name } => {
            let (var, _ty) = env.get(local_id).ok_or_else(|| {
                CodegenError::cranelift(
                    format!("no variable for local `{name}` — compiler bug"),
                    expr.span,
                )
            })?;
            let v = builder.use_var(*var);
            // Three-state ownership: `use_var` produces a Borrowed
            // reference. Convert to Owned by retaining so the caller
            // (bind / return / call-arg / discard) can dispose of it
            // uniformly. For non-refcounted types this is a no-op.
            if is_refcounted_type(&expr.ty) {
                emit_retain(builder, module, runtime, v);
            }
            Ok(v)
        }
        IrExprKind::Decl { .. } => Err(CodegenError::not_supported(
            "declaration reference (imports/functions as values)",
            expr.span,
        )),
        IrExprKind::BinOp { op, left, right } => {
            if matches!(op, BinaryOp::And | BinaryOp::Or) {
                return lower_short_circuit(
                    builder,
                    *op,
                    left,
                    right,
                    env,
                    func_ids_by_def,
                    module,
                    runtime,
                );
            }
            // String operands route through the String runtime helpers
            // and have their own ownership semantics (release inputs
            // after the helper call). Dispatch here so we have access
            // to the IR type information.
            if matches!(&left.ty, Type::String) && matches!(&right.ty, Type::String) {
                let l = lower_expr(builder, left, env, func_ids_by_def, module, runtime)?;
                let r = lower_expr(builder, right, env, func_ids_by_def, module, runtime)?;
                return lower_string_binop(builder, *op, l, r, expr.span, module, runtime);
            }
            let l = lower_expr(builder, left, env, func_ids_by_def, module, runtime)?;
            let r = lower_expr(builder, right, env, func_ids_by_def, module, runtime)?;
            lower_binop_strict(builder, *op, l, r, expr.span, module, runtime)
        }
        IrExprKind::UnOp { op, operand } => {
            let v = lower_expr(builder, operand, env, func_ids_by_def, module, runtime)?;
            lower_unop(builder, *op, v, expr.span, module, runtime)
        }
        IrExprKind::Call { kind, callee_name, args } => match kind {
            IrCallKind::Agent { def_id } => {
                let callee_id = func_ids_by_def.get(def_id).ok_or_else(|| {
                    CodegenError::cranelift(
                        format!("agent `{callee_name}` has no declared function id"),
                        expr.span,
                    )
                })?;
                let callee_ref = module.declare_func_in_func(*callee_id, builder.func);
                // Lower arguments. Each refcounted arg comes back from
                // `lower_expr` as Owned (+1). Per +0 ABI: caller does
                // not pre-bump when passing; callee retains on entry.
                // After the call returns, our caller's +1 is no longer
                // needed (the callee owns its own copy now), so we
                // release each refcounted arg.
                let mut arg_vals = Vec::with_capacity(args.len());
                let mut arg_refcounted: Vec<bool> = Vec::with_capacity(args.len());
                for a in args {
                    arg_vals.push(lower_expr(
                        builder,
                        a,
                        env,
                        func_ids_by_def,
                        module,
                        runtime,
                    )?);
                    arg_refcounted.push(is_refcounted_type(&a.ty));
                }
                let call = builder.ins().call(callee_ref, &arg_vals);
                let results = builder.inst_results(call);
                if results.len() != 1 {
                    return Err(CodegenError::cranelift(
                        format!(
                            "agent `{callee_name}` returned {} values; slice 12a expects exactly 1",
                            results.len()
                        ),
                        expr.span,
                    ));
                }
                let result = results[0];
                // Release each refcounted arg's +1 (the callee took its
                // own ownership via parameter retain).
                for (v, is_ref) in arg_vals.iter().zip(arg_refcounted.iter()) {
                    if *is_ref {
                        emit_release(builder, module, runtime, *v);
                    }
                }
                Ok(result)
            }
            IrCallKind::Tool { def_id, .. } => {
                // Phase 14: emit a DIRECT typed call to the tool's
                // `#[tool]`-generated wrapper symbol. No JSON, no
                // dynamic dispatch — just a `call` instruction against
                // a named import. Link-time symbol resolution catches
                // missing tool implementations; Cranelift-level
                // type-matching catches wrong-type mismatches at
                // parity-harness or codegen time.
                let tool = runtime.ir_tools.get(def_id).cloned().ok_or_else(|| {
                    CodegenError::cranelift(
                        format!(
                            "tool `{callee_name}` metadata missing from ir_tools — declare-pass invariant violated"
                        ),
                        expr.span,
                    )
                })?;

                // Arity cross-check — belt-and-braces vs. the
                // typechecker's check that already ran.
                if tool.params.len() != args.len() {
                    return Err(CodegenError::cranelift(
                        format!(
                            "tool `{callee_name}` declared with {} param(s) but called with {}",
                            tool.params.len(),
                            args.len()
                        ),
                        expr.span,
                    ));
                }

                // Declare or re-use the wrapper-symbol import.
                let wrapper_id = {
                    let mut cache = runtime.tool_wrapper_ids.borrow_mut();
                    if let Some(id) = cache.get(def_id) {
                        *id
                    } else {
                        let mut sig = module.make_signature();
                        for p in &tool.params {
                            sig.params.push(AbiParam::new(cl_type_for(&p.ty, p.span)?));
                        }
                        if !matches!(tool.return_ty, Type::Nothing) {
                            sig.returns
                                .push(AbiParam::new(cl_type_for(&tool.return_ty, tool.span)?));
                        }
                        let symbol = tool_wrapper_symbol(&tool.name);
                        let id = module
                            .declare_function(&symbol, Linkage::Import, &sig)
                            .map_err(|e| {
                                CodegenError::cranelift(
                                    format!(
                                        "declare tool wrapper `{symbol}`: {e}"
                                    ),
                                    expr.span,
                                )
                            })?;
                        cache.insert(*def_id, id);
                        id
                    }
                };

                // Tool-call ABI (Phase 14): refcount lifecycle matches
                // the agent-call convention (slice 12f) — caller
                // produces an Owned (+1) refcounted arg via the
                // existing `lower_expr` path (use_var retains to
                // convert Borrowed→Owned), the `#[tool]` wrapper
                // reads bytes without touching refcount
                // (`abi::FromCorvidAbi for String` is borrow-only so
                // the wrapper neither retains nor releases), and
                // after the call returns the caller releases its +1.
                // Net effect: one retain + one release around the
                // call = zero net refcount change, which is what a
                // borrow-style FFI boundary should look like. Without
                // the release, the +1 leaks.
                let mut arg_vals = Vec::with_capacity(args.len());
                let mut arg_refcounted: Vec<bool> = Vec::with_capacity(args.len());
                for a in args {
                    arg_vals.push(lower_expr(
                        builder,
                        a,
                        env,
                        func_ids_by_def,
                        module,
                        runtime,
                    )?);
                    arg_refcounted.push(is_refcounted_type(&a.ty));
                }

                let fref = module.declare_func_in_func(wrapper_id, builder.func);
                let call = builder.ins().call(fref, &arg_vals);
                let result_vals: Vec<ClValue> =
                    builder.inst_results(call).iter().copied().collect();

                // Release the +1 we put on each refcounted arg. For
                // literals (refcount = i64::MIN sentinel) this is a
                // no-op; for heap values it decrements the refcount
                // we bumped pre-call.
                for (v, is_ref) in arg_vals.iter().zip(arg_refcounted.iter()) {
                    if *is_ref {
                        emit_release(builder, module, runtime, *v);
                    }
                }

                // Return-value unpacking. For Nothing-returning tools
                // there's no result to hand back; synthesize a
                // zero-Int so the expr-result contract stays uniform.
                if matches!(expr.ty, Type::Nothing) {
                    Ok(builder.ins().iconst(I64, 0))
                } else if result_vals.len() == 1 {
                    Ok(result_vals[0])
                } else {
                    Err(CodegenError::cranelift(
                        format!(
                            "tool `{callee_name}` wrapper returned {} values; expected 1 for type `{}`",
                            result_vals.len(),
                            expr.ty.display_name()
                        ),
                        expr.span,
                    ))
                }
            }
            IrCallKind::Prompt { def_id, .. } => {
                lower_prompt_call(
                    builder,
                    module,
                    runtime,
                    *def_id,
                    callee_name,
                    args,
                    env,
                    func_ids_by_def,
                    &expr.ty,
                    expr.span,
                )
            }
            IrCallKind::StructConstructor { def_id } => {
                let ir_type = runtime.ir_types.get(def_id).cloned().ok_or_else(|| {
                    CodegenError::cranelift(
                        format!("struct type `{callee_name}` metadata missing from ir_types"),
                        expr.span,
                    )
                })?;
                lower_struct_constructor(
                    builder,
                    module,
                    runtime,
                    &ir_type,
                    args,
                    env,
                    func_ids_by_def,
                    expr.span,
                )
            }
            IrCallKind::Unknown => Err(CodegenError::cranelift(
                format!("call to `{callee_name}` did not resolve — typecheck should have caught this"),
                expr.span,
            )),
        },
        IrExprKind::FieldAccess { target, field } => {
            let def_id = match &target.ty {
                Type::Struct(id) => *id,
                other => {
                    return Err(CodegenError::cranelift(
                        format!(
                            "field access target has non-struct type `{other:?}` — typecheck should have caught this"
                        ),
                        expr.span,
                    ));
                }
            };
            let ir_type = runtime.ir_types.get(&def_id).cloned().ok_or_else(|| {
                CodegenError::cranelift(
                    format!("struct metadata missing for field access to `{field}`"),
                    expr.span,
                )
            })?;
            let (i, field_meta) = ir_type
                .fields
                .iter()
                .enumerate()
                .find(|(_, f)| &f.name == field)
                .ok_or_else(|| {
                    CodegenError::cranelift(
                        format!("field `{field}` not found on struct `{}`", ir_type.name),
                        expr.span,
                    )
                })?;
            let offset = (i as i32) * STRUCT_FIELD_SLOT_BYTES;
            let field_cl_ty = cl_type_for(&field_meta.ty, field_meta.span)?;

            let struct_ptr =
                lower_expr(builder, target, env, func_ids_by_def, module, runtime)?;
            let field_val = builder.ins().load(
                field_cl_ty,
                cranelift_codegen::ir::MemFlags::trusted(),
                struct_ptr,
                offset,
            );
            // Retain refcounted field so caller gets an Owned ref.
            if is_refcounted_type(&field_meta.ty) {
                emit_retain(builder, module, runtime, field_val);
            }
            // Release the temp +1 on the struct pointer (the struct
            // itself remains owned by its binding or deeper expr).
            emit_release(builder, module, runtime, struct_ptr);
            Ok(field_val)
        }
        IrExprKind::Index { target, index } => {
            // Element type from the Index expression's annotated type
            // (the type checker attaches the element type).
            let elem_ty = expr.ty.clone();
            let elem_cl_ty = cl_type_for(&elem_ty, expr.span)?;
            let elem_refcounted = is_refcounted_type(&elem_ty);

            let list_ptr = lower_expr(builder, target, env, func_ids_by_def, module, runtime)?;
            let idx_val = lower_expr(builder, index, env, func_ids_by_def, module, runtime)?;

            // Bounds check: trap if `idx_val >= length` or `idx_val < 0`.
            let length = builder.ins().load(
                I64,
                cranelift_codegen::ir::MemFlags::trusted(),
                list_ptr,
                0,
            );
            let in_range_hi =
                builder.ins().icmp(IntCC::SignedLessThan, idx_val, length);
            let zero = builder.ins().iconst(I64, 0);
            let in_range_lo =
                builder.ins().icmp(IntCC::SignedGreaterThanOrEqual, idx_val, zero);
            let in_range = builder.ins().band(in_range_hi, in_range_lo);
            let trap_block = builder.create_block();
            let cont_block = builder.create_block();
            builder
                .ins()
                .brif(in_range, cont_block, &[], trap_block, &[]);
            builder.switch_to_block(trap_block);
            builder.seal_block(trap_block);
            let callee_ref =
                module.declare_func_in_func(runtime.overflow, builder.func);
            builder.ins().call(callee_ref, &[]);
            builder
                .ins()
                .trap(cranelift_codegen::ir::TrapCode::INTEGER_OVERFLOW);
            builder.switch_to_block(cont_block);
            builder.seal_block(cont_block);

            // Element address = list_ptr + 8 + idx * 8.
            let offset = builder.ins().imul_imm(idx_val, 8);
            let base = builder.ins().iadd_imm(list_ptr, 8);
            let elem_addr = builder.ins().iadd(base, offset);
            let elem_val = builder.ins().load(
                elem_cl_ty,
                cranelift_codegen::ir::MemFlags::trusted(),
                elem_addr,
                0,
            );
            if elem_refcounted {
                emit_retain(builder, module, runtime, elem_val);
            }
            // Release the temp +1 on the list pointer (caller's Owned
            // reference obtained from lower_expr of the target).
            emit_release(builder, module, runtime, list_ptr);
            Ok(elem_val)
        }
        IrExprKind::List { items } => {
            // Element type taken from the List's annotated type.
            let elem_ty = match &expr.ty {
                Type::List(elem) => (**elem).clone(),
                other => {
                    return Err(CodegenError::cranelift(
                        format!(
                            "list literal has non-list type `{other:?}` — typecheck should have caught this"
                        ),
                        expr.span,
                    ));
                }
            };
            let _elem_refcounted = is_refcounted_type(&elem_ty);
            // Allocation size: 8 (length) + 8 * N (elements).
            let total_bytes = 8 + 8 * items.len() as i64;
            let size_val = builder.ins().iconst(I64, total_bytes);
            // Phase 17a: single typed allocator. The typeinfo block
            // (pre-emitted in lower_file) carries destroy_fn
            // (corvid_destroy_list for refcounted elements, NULL
            // otherwise) and trace_fn (corvid_trace_list always).
            let list_ti_id = runtime
                .list_typeinfos
                .get(&elem_ty)
                .copied()
                .ok_or_else(|| {
                    CodegenError::cranelift(
                        format!(
                            "no typeinfo pre-emitted for List<{}> — collect_list_element_types missed this site",
                            mangle_type_name(&elem_ty)
                        ),
                        expr.span,
                    )
                })?;
            let list_ti_gv = module.declare_data_in_func(list_ti_id, builder.func);
            let ti_addr = builder.ins().symbol_value(I64, list_ti_gv);
            let alloc_ref =
                module.declare_func_in_func(runtime.alloc_typed, builder.func);
            let call = builder.ins().call(alloc_ref, &[size_val, ti_addr]);
            let list_ptr = builder.inst_results(call)[0];
            // Store length at offset 0.
            let length_val = builder.ins().iconst(I64, items.len() as i64);
            builder.ins().store(
                cranelift_codegen::ir::MemFlags::trusted(),
                length_val,
                list_ptr,
                0,
            );
            // Store each element at offset 8 + i * 8. The element's
            // Owned +1 transfers into the list.
            for (i, item) in items.iter().enumerate() {
                let item_val =
                    lower_expr(builder, item, env, func_ids_by_def, module, runtime)?;
                let offset = 8 + (i as i32) * 8;
                builder.ins().store(
                    cranelift_codegen::ir::MemFlags::trusted(),
                    item_val,
                    list_ptr,
                    offset,
                );
            }
            Ok(list_ptr)
        }
    }
}

/// Strict (eager) binary operator lowering: arithmetic and comparison
/// for both `Int` and `Float`. Mixed `Int + Float` operands are
/// promoted to `F64` first (matches the interpreter's widening rule).
/// `Int` arithmetic traps on overflow / div-zero; `Float` follows IEEE
/// 754 (no trap, NaN/Inf propagate naturally).
fn lower_binop_strict(
    builder: &mut FunctionBuilder,
    op: BinaryOp,
    l: ClValue,
    r: ClValue,
    span: Span,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<ClValue, CodegenError> {
    // Promote mixed Int + Float operands to F64 — same widening the
    // interpreter applies in `eval_arithmetic`.
    let (l, r, dom) = promote_arith(builder, l, r, span)?;

    match (op, dom) {
        // ---- Int arithmetic, overflow-trapping ------------------------
        (BinaryOp::Add, ArithDomain::Int) => {
            with_overflow_trap(builder, l, r, module, runtime, |b| {
                b.ins().sadd_overflow(l, r)
            })
        }
        (BinaryOp::Sub, ArithDomain::Int) => {
            with_overflow_trap(builder, l, r, module, runtime, |b| {
                b.ins().ssub_overflow(l, r)
            })
        }
        (BinaryOp::Mul, ArithDomain::Int) => {
            with_overflow_trap(builder, l, r, module, runtime, |b| {
                b.ins().smul_overflow(l, r)
            })
        }
        (BinaryOp::Div, ArithDomain::Int) => {
            trap_on_zero(builder, r, module, runtime);
            Ok(builder.ins().sdiv(l, r))
        }
        (BinaryOp::Mod, ArithDomain::Int) => {
            trap_on_zero(builder, r, module, runtime);
            Ok(builder.ins().srem(l, r))
        }

        // ---- Float arithmetic, IEEE 754 (no trap) ---------------------
        (BinaryOp::Add, ArithDomain::Float) => Ok(builder.ins().fadd(l, r)),
        (BinaryOp::Sub, ArithDomain::Float) => Ok(builder.ins().fsub(l, r)),
        (BinaryOp::Mul, ArithDomain::Float) => Ok(builder.ins().fmul(l, r)),
        (BinaryOp::Div, ArithDomain::Float) => Ok(builder.ins().fdiv(l, r)),
        (BinaryOp::Mod, ArithDomain::Float) => {
            // Cranelift has no `frem`. Compute `a - trunc(a / b) * b`,
            // matching Rust's `f64::%` semantics.
            let div = builder.ins().fdiv(l, r);
            let trunc = builder.ins().trunc(div);
            let mul = builder.ins().fmul(trunc, r);
            Ok(builder.ins().fsub(l, mul))
        }

        // ---- Comparisons -----------------------------------------------
        (BinaryOp::Eq, ArithDomain::Int) => Ok(builder.ins().icmp(IntCC::Equal, l, r)),
        (BinaryOp::NotEq, ArithDomain::Int) => Ok(builder.ins().icmp(IntCC::NotEqual, l, r)),
        (BinaryOp::Lt, ArithDomain::Int) => Ok(builder.ins().icmp(IntCC::SignedLessThan, l, r)),
        (BinaryOp::LtEq, ArithDomain::Int) => {
            Ok(builder.ins().icmp(IntCC::SignedLessThanOrEqual, l, r))
        }
        (BinaryOp::Gt, ArithDomain::Int) => {
            Ok(builder.ins().icmp(IntCC::SignedGreaterThan, l, r))
        }
        (BinaryOp::GtEq, ArithDomain::Int) => {
            Ok(builder.ins().icmp(IntCC::SignedGreaterThanOrEqual, l, r))
        }
        // Float comparisons: IEEE-correct NaN handling. Rust's `==`
        // returns false when either side is NaN; `!=` returns true.
        // FloatCC::Equal matches `==`; UnorderedOrNotEqual matches `!=`.
        // The ordered LessThan / LessThanOrEqual / GreaterThan /
        // GreaterThanOrEqual variants all return false on NaN, matching
        // Rust's lt/le/gt/ge.
        (BinaryOp::Eq, ArithDomain::Float) => Ok(builder.ins().fcmp(FloatCC::Equal, l, r)),
        (BinaryOp::NotEq, ArithDomain::Float) => {
            Ok(builder.ins().fcmp(FloatCC::NotEqual, l, r))
        }
        (BinaryOp::Lt, ArithDomain::Float) => Ok(builder.ins().fcmp(FloatCC::LessThan, l, r)),
        (BinaryOp::LtEq, ArithDomain::Float) => {
            Ok(builder.ins().fcmp(FloatCC::LessThanOrEqual, l, r))
        }
        (BinaryOp::Gt, ArithDomain::Float) => {
            Ok(builder.ins().fcmp(FloatCC::GreaterThan, l, r))
        }
        (BinaryOp::GtEq, ArithDomain::Float) => {
            Ok(builder.ins().fcmp(FloatCC::GreaterThanOrEqual, l, r))
        }

        (BinaryOp::And | BinaryOp::Or, _) => {
            let _ = span;
            unreachable!("and/or is short-circuited upstream and never reaches lower_binop_strict")
        }
    }
}

/// Which arithmetic family this binop operates in after operand
/// promotion. `Bool == Bool` lands in `Int` because `I8` is integer
/// from Cranelift's perspective.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArithDomain {
    Int,
    Float,
}

/// String operators: `+` (concat), `==` / `!=`, `<` / `<=` / `>` / `>=`.
/// Both operands arrive as Owned String pointers (per the three-state
/// ownership model). The runtime helpers read but don't retain; we
/// release each input after the call.
fn lower_string_binop(
    builder: &mut FunctionBuilder,
    op: BinaryOp,
    l: ClValue,
    r: ClValue,
    span: Span,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<ClValue, CodegenError> {
    match op {
        BinaryOp::Add => {
            // Concat returns +1 from `corvid_alloc` inside the helper.
            let callee = module.declare_func_in_func(runtime.string_concat, builder.func);
            let call = builder.ins().call(callee, &[l, r]);
            let result = builder.inst_results(call)[0];
            // Release the input Owned references.
            emit_release(builder, module, runtime, l);
            emit_release(builder, module, runtime, r);
            Ok(result)
        }
        BinaryOp::Eq | BinaryOp::NotEq => {
            let callee = module.declare_func_in_func(runtime.string_eq, builder.func);
            let call = builder.ins().call(callee, &[l, r]);
            let eq_i64 = builder.inst_results(call)[0];
            // Narrow i64 (0/1) → i8 (0/1) to match Bool's Cranelift width.
            let eq_i8 = builder.ins().ireduce(I8, eq_i64);
            let result = if matches!(op, BinaryOp::Eq) {
                eq_i8
            } else {
                // != : flip 0↔1.
                let zero = builder.ins().iconst(I8, 0);
                builder.ins().icmp(IntCC::Equal, eq_i8, zero)
            };
            emit_release(builder, module, runtime, l);
            emit_release(builder, module, runtime, r);
            Ok(result)
        }
        BinaryOp::Lt | BinaryOp::LtEq | BinaryOp::Gt | BinaryOp::GtEq => {
            let callee = module.declare_func_in_func(runtime.string_cmp, builder.func);
            let call = builder.ins().call(callee, &[l, r]);
            let cmp_i64 = builder.inst_results(call)[0];
            let zero = builder.ins().iconst(I64, 0);
            let cc = match op {
                BinaryOp::Lt => IntCC::SignedLessThan,
                BinaryOp::LtEq => IntCC::SignedLessThanOrEqual,
                BinaryOp::Gt => IntCC::SignedGreaterThan,
                BinaryOp::GtEq => IntCC::SignedGreaterThanOrEqual,
                _ => unreachable!(),
            };
            let result = builder.ins().icmp(cc, cmp_i64, zero);
            emit_release(builder, module, runtime, l);
            emit_release(builder, module, runtime, r);
            Ok(result)
        }
        BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => Err(
            CodegenError::not_supported(
                format!("`{op:?}` is not defined for `String` operands"),
                span,
            ),
        ),
        BinaryOp::And | BinaryOp::Or => unreachable!("and/or short-circuited upstream"),
    }
}

/// Lower a `for x in iter: body` loop. Expects `iter` to be a List.
///
/// Block layout:
/// ```text
///   entry:   init i=0, loop-var=0; jump header
///   header:  brif (i < length) → body : exit
///   body:    load element[i]; release-on-rebind loop-var; retain if
///            refcounted; def_var(loop-var, element); lower body;
///            on fallthrough → jump step
///   step:    increment i; jump header
///   exit:    after the loop
/// ```
/// `continue` jumps to `step`, `break` jumps to `exit`. Both release
/// any refcounted locals deeper than the scope depth recorded at loop
/// entry.
#[allow(clippy::too_many_arguments)]
fn lower_for(
    builder: &mut FunctionBuilder,
    var_local: LocalId,
    iter: &IrExpr,
    body: &IrBlock,
    span: Span,
    env: &mut HashMap<LocalId, (Variable, clir::Type)>,
    var_idx: &mut usize,
    scope_stack: &mut Vec<Vec<(LocalId, Variable)>>,
    loop_stack: &mut Vec<LoopCtx>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<BlockOutcome, CodegenError> {
    // Element type comes from the iterator's list type.
    let elem_ty = match &iter.ty {
        Type::List(elem) => (**elem).clone(),
        Type::String => {
            return Err(CodegenError::not_supported(
                "`for c in string` iteration in native code — future slice (needs iterator \
                 protocol or string-specific lowering)",
                span,
            ));
        }
        other => {
            return Err(CodegenError::cranelift(
                format!(
                    "`for` iterator has non-list type `{other:?}` — typecheck should have caught this"
                ),
                span,
            ));
        }
    };
    let elem_cl_ty = cl_type_for(&elem_ty, span)?;
    let elem_refcounted = is_refcounted_type(&elem_ty);

    // Lower the iterator expression — Owned +1 if refcounted list.
    let list_ptr = lower_expr(builder, iter, env, func_ids_by_def, module, runtime)?;

    // Load length from offset 0 of the list payload.
    let length = builder.ins().load(
        I64,
        cranelift_codegen::ir::MemFlags::trusted(),
        list_ptr,
        0,
    );

    // Declare + init loop variable. Starts at 0 (null if refcounted)
    // so the release-on-rebind on the first iteration is a no-op.
    let loop_var = Variable::from_u32(*var_idx as u32);
    *var_idx += 1;
    builder.declare_var(loop_var, elem_cl_ty);
    let zero_elem = builder.ins().iconst(elem_cl_ty, 0);
    builder.def_var(loop_var, zero_elem);
    env.insert(var_local, (loop_var, elem_cl_ty));
    // Track the loop variable in the enclosing (not yet pushed) body
    // scope's sibling — the CURRENT scope, so it releases when the
    // enclosing block exits. That ensures the final iteration's value
    // gets released.
    if elem_refcounted {
        if let Some(top) = scope_stack.last_mut() {
            top.push((var_local, loop_var));
        }
    }

    // Declare + init index counter.
    let i_var = Variable::from_u32(*var_idx as u32);
    *var_idx += 1;
    builder.declare_var(i_var, I64);
    let zero_i = builder.ins().iconst(I64, 0);
    builder.def_var(i_var, zero_i);

    // Create the four blocks.
    let header_b = builder.create_block();
    let body_b = builder.create_block();
    let step_b = builder.create_block();
    let exit_b = builder.create_block();

    // Record the loop context so break/continue can find their targets.
    let scope_depth_at_entry = scope_stack.len();
    loop_stack.push(LoopCtx {
        step_block: step_b,
        exit_block: exit_b,
        scope_depth_at_entry,
    });

    builder.ins().jump(header_b, &[]);

    // --- header ---
    builder.switch_to_block(header_b);
    let i_now = builder.use_var(i_var);
    let keep_going = builder.ins().icmp(IntCC::SignedLessThan, i_now, length);
    builder.ins().brif(keep_going, body_b, &[], exit_b, &[]);

    // --- body ---
    builder.switch_to_block(body_b);
    builder.seal_block(body_b);
    // Compute element address: list_ptr + 8 + i * 8.
    let offset = builder.ins().imul_imm(i_now, 8);
    let base = builder.ins().iadd_imm(list_ptr, 8);
    let elem_addr = builder.ins().iadd(base, offset);
    let elem_val = builder.ins().load(
        elem_cl_ty,
        cranelift_codegen::ir::MemFlags::trusted(),
        elem_addr,
        0,
    );
    // Rebind loop-var: release old value (null-safe), retain new if
    // refcounted, def_var.
    if elem_refcounted {
        let old = builder.use_var(loop_var);
        emit_release(builder, module, runtime, old);
        emit_retain(builder, module, runtime, elem_val);
    }
    builder.def_var(loop_var, elem_val);

    // Push body scope so body-local Lets get released at end of each
    // iteration (or on break/continue/return from inside body).
    scope_stack.push(Vec::new());
    let body_outcome = lower_block(
        builder,
        body,
        env,
        var_idx,
        scope_stack,
        loop_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;
    match body_outcome {
        BlockOutcome::Normal => {
            // Release body-scope locals before jumping to step.
            let body_scope = scope_stack.pop().unwrap_or_default();
            for (_, v) in body_scope.iter().rev() {
                let x = builder.use_var(*v);
                emit_release(builder, module, runtime, x);
            }
            builder.ins().jump(step_b, &[]);
        }
        BlockOutcome::Terminated => {
            // Body returned — the return already emitted releases for
            // all live scopes. Just pop the body scope.
            scope_stack.pop();
        }
    }

    // --- step ---
    builder.switch_to_block(step_b);
    builder.seal_block(step_b);
    let i_next = builder.ins().iadd_imm(i_now, 1);
    builder.def_var(i_var, i_next);
    builder.ins().jump(header_b, &[]);

    // Now both predecessors of header_b (entry + step) have been
    // emitted, so we can seal it.
    builder.seal_block(header_b);

    // --- exit ---
    builder.switch_to_block(exit_b);
    builder.seal_block(exit_b);
    loop_stack.pop();

    // Release the list pointer we retained at the top (lower_expr
    // returned an Owned ref if refcounted).
    if is_refcounted_type(&iter.ty) {
        emit_release(builder, module, runtime, list_ptr);
    }
    Ok(BlockOutcome::Normal)
}

/// Release refcounted locals deeper than `floor_depth`, then jump to
/// the given block. Shared by `break` and `continue`.
fn lower_break_or_continue(
    builder: &mut FunctionBuilder,
    is_break: bool,
    span: Span,
    scope_stack: &mut Vec<Vec<(LocalId, Variable)>>,
    loop_stack: &mut Vec<LoopCtx>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<BlockOutcome, CodegenError> {
    let ctx = loop_stack.last().ok_or_else(|| {
        CodegenError::cranelift(
            format!(
                "`{}` outside of a loop — typecheck or parser should have caught this",
                if is_break { "break" } else { "continue" }
            ),
            span,
        )
    })?;
    // Walk scopes deeper than `scope_depth_at_entry`, releasing
    // refcounted locals. Don't pop — the lower_block that created
    // those scopes is still on the stack above us.
    for depth in (ctx.scope_depth_at_entry..scope_stack.len()).rev() {
        let scope = &scope_stack[depth];
        for (_, v) in scope.iter().rev() {
            let x = builder.use_var(*v);
            emit_release(builder, module, runtime, x);
        }
    }
    let target = if is_break {
        ctx.exit_block
    } else {
        ctx.step_block
    };
    builder.ins().jump(target, &[]);
    Ok(BlockOutcome::Terminated)
}

/// Lower a struct constructor: allocate, store each field at its
/// offset, return the struct pointer (refcount = 1, Owned).
///
/// Each constructor argument is lowered as an Owned temp; the store
/// transfers ownership into the struct (no extra retain, no release).
/// If the struct has a per-type destructor (at least one refcounted
/// field), we use `corvid_alloc_with_destructor` so `corvid_release`
/// will release those fields when the struct is eventually freed.
fn lower_struct_constructor(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    ty: &corvid_ir::IrType,
    args: &[IrExpr],
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    span: Span,
) -> Result<ClValue, CodegenError> {
    if args.len() != ty.fields.len() {
        return Err(CodegenError::cranelift(
            format!(
                "struct `{}` expects {} field(s), got {}",
                ty.name,
                ty.fields.len(),
                args.len()
            ),
            span,
        ));
    }
    let size = builder
        .ins()
        .iconst(I64, struct_payload_bytes(ty.fields.len()));
    // Phase 17a: structs with refcounted fields use the typeinfo-
    // driven allocator. Structs with no refcounted fields currently
    // skip typeinfo entirely (lower_file bypasses emission for them)
    // — they allocate with NULL destroy_fn via a runtime-owned
    // empty typeinfo? For 17a we keep the pre-typed behavior for
    // these: only emit typed allocation when the struct actually
    // has refcounted fields requiring dispatch. Non-refcounted
    // structs remain on the old path *only temporarily* — slice
    // 17a's follow-up will uniformize this once 17d needs every
    // heap object traceable. Tracked as a TODO in ROADMAP.
    let struct_ptr = if let Some(&ti_id) = runtime.struct_typeinfos.get(&ty.id) {
        let ti_gv = module.declare_data_in_func(ti_id, builder.func);
        let ti_addr = builder.ins().symbol_value(I64, ti_gv);
        let alloc_ref = module.declare_func_in_func(runtime.alloc_typed, builder.func);
        let call = builder.ins().call(alloc_ref, &[size, ti_addr]);
        builder.inst_results(call)[0]
    } else {
        return Err(CodegenError::cranelift(
            format!(
                "struct `{}` has no typeinfo emitted — 17a should cover every refcounted struct; is this a non-refcounted struct that still hits this path?",
                ty.name
            ),
            span,
        ));
    };

    // Store each field at offset i * STRUCT_FIELD_SLOT_BYTES. Each
    // field arg is lowered as an Owned temp; the store transfers that
    // +1 ownership into the struct — no extra retain, no release.
    for (i, arg) in args.iter().enumerate() {
        let value = lower_expr(builder, arg, env, func_ids_by_def, module, runtime)?;
        let offset = (i as i32) * STRUCT_FIELD_SLOT_BYTES;
        builder.ins().store(
            cranelift_codegen::ir::MemFlags::trusted(),
            value,
            struct_ptr,
            offset,
        );
    }
    Ok(struct_ptr)
}

/// Lower a String literal into a static `.rodata` block and return the
/// descriptor pointer (the value the runtime expects).
///
/// Compute the linker-visible symbol of the `#[tool]`-generated
/// wrapper for a given Corvid tool declaration name.
///
/// Must stay aligned with `corvid_macros::mangle_tool_name`. If the
/// two drift, link errors point at `__corvid_tool_<one-name>` while
/// the user's crate defines `__corvid_tool_<other-name>`. Mangling
/// rule: every non-ASCII-alphanumeric character becomes `_`.
// ------------------------------------------------------------
// Phase 15 — prompt call lowering.
//
// Codegen knows the prompt's template + signature + return type at
// compile time. The strategy:
//   1. Parse the template into segments (Literal | Param) at codegen
//      time. Each Param segment names a parameter to interpolate.
//   2. At the call site, emit a sequence of string-concat operations
//      that builds the rendered prompt: literal text + stringified
//      arg + literal text + ...
//   3. Build literal CorvidStrings for the prompt name, the human-
//      readable signature ("foo(x: Int) -> String"), and the model
//      (empty — runtime falls back to default_model from CORVID_MODEL).
//   4. Call the typed bridge by return type, passing the four
//      CorvidString args.
//   5. Receive the typed return.
// ------------------------------------------------------------

#[derive(Debug)]
enum TemplateSegment<'a> {
    Literal(&'a str),
    Param(usize), // index into the prompt's params
}

/// Parse a prompt template into literal + `{param_name}` segments.
/// Param names that aren't in `params` produce a codegen error —
/// matches what the typechecker should already enforce, kept as
/// belt-and-braces.
fn parse_prompt_template<'a>(
    template: &'a str,
    params: &[corvid_ir::IrParam],
    span: Span,
) -> Result<Vec<TemplateSegment<'a>>, CodegenError> {
    let mut out: Vec<TemplateSegment<'a>> = Vec::new();
    let mut cursor = 0;
    let bytes = template.as_bytes();
    while cursor < bytes.len() {
        if let Some(open_rel) = template[cursor..].find('{') {
            let open = cursor + open_rel;
            // Emit literal up to the brace.
            if open > cursor {
                out.push(TemplateSegment::Literal(&template[cursor..open]));
            }
            // Find closing brace.
            let close_rel = template[open + 1..].find('}').ok_or_else(|| {
                CodegenError::cranelift(
                    format!(
                        "prompt template has unmatched `{{` near offset {open}: `{template}`"
                    ),
                    span,
                )
            })?;
            let close = open + 1 + close_rel;
            let name = template[open + 1..close].trim();
            let idx = params.iter().position(|p| p.name == name).ok_or_else(|| {
                CodegenError::cranelift(
                    format!(
                        "prompt template references `{{{name}}}` but no such parameter — typechecker should have caught this; available: {:?}",
                        params.iter().map(|p| &p.name).collect::<Vec<_>>()
                    ),
                    span,
                )
            })?;
            out.push(TemplateSegment::Param(idx));
            cursor = close + 1;
        } else {
            out.push(TemplateSegment::Literal(&template[cursor..]));
            break;
        }
    }
    Ok(out)
}

/// Emit a CorvidString from a `&str` literal — wraps the existing
/// `lower_string_literal` helper so prompt-bridge call sites can
/// pass the prompt name / signature / model as ordinary string
/// constants without contortions.
fn emit_string_const(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    s: &str,
    span: Span,
) -> Result<ClValue, CodegenError> {
    lower_string_literal(builder, module, runtime, s, span)
}

/// Stringify a non-String scalar arg via the runtime helper that
/// matches the value's Cranelift type. Returns a fresh refcount-1
/// CorvidString. For String args the value is returned as-is.
fn emit_stringify_arg(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    arg_value: ClValue,
    arg_ty: &Type,
    span: Span,
) -> Result<ClValue, CodegenError> {
    match arg_ty {
        Type::String => Ok(arg_value),
        Type::Int => {
            let f = module.declare_func_in_func(runtime.string_from_int, builder.func);
            let call = builder.ins().call(f, &[arg_value]);
            let results: Vec<ClValue> =
                builder.inst_results(call).iter().copied().collect();
            Ok(results[0])
        }
        Type::Bool => {
            let f = module.declare_func_in_func(runtime.string_from_bool, builder.func);
            let call = builder.ins().call(f, &[arg_value]);
            let results: Vec<ClValue> =
                builder.inst_results(call).iter().copied().collect();
            Ok(results[0])
        }
        Type::Float => {
            let f = module.declare_func_in_func(runtime.string_from_float, builder.func);
            let call = builder.ins().call(f, &[arg_value]);
            let results: Vec<ClValue> =
                builder.inst_results(call).iter().copied().collect();
            Ok(results[0])
        }
        other => Err(CodegenError::not_supported(
            format!(
                "prompt argument type `{}` is not yet supported in template interpolation — Phase 15 supports Int / Bool / Float / String only; Struct / List defer to a later slice",
                other.display_name()
            ),
            span,
        )),
    }
}

/// Concatenate a sequence of CorvidString values into one. Releases
/// the intermediate +1 refcounts as concatenation proceeds so the
/// final CorvidString is the only live allocation.
fn emit_concat_chain(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    parts: Vec<ClValue>,
    span: Span,
) -> Result<ClValue, CodegenError> {
    if parts.is_empty() {
        // Empty rendered prompt — emit an empty literal.
        return emit_string_const(builder, module, runtime, "", span);
    }
    let mut acc = parts[0];
    let concat_fid =
        module.declare_func_in_func(runtime.string_concat, builder.func);
    for next in parts.into_iter().skip(1) {
        let call = builder.ins().call(concat_fid, &[acc, next]);
        let results: Vec<ClValue> =
            builder.inst_results(call).iter().copied().collect();
        let new_acc = results[0];
        // Release the previous accumulator + the just-consumed next.
        // string_concat returns a fresh +1; the inputs are consumed
        // (concat keeps its own copies if it needs them, but the
        // result is independent — release-on-consume is safe).
        emit_release(builder, module, runtime, acc);
        emit_release(builder, module, runtime, next);
        acc = new_acc;
    }
    Ok(acc)
}

#[allow(clippy::too_many_arguments)]
fn lower_prompt_call(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    def_id: DefId,
    callee_name: &str,
    args: &[IrExpr],
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    return_ty: &Type,
    span: Span,
) -> Result<ClValue, CodegenError> {
    let prompt = runtime
        .ir_prompts
        .get(&def_id)
        .cloned()
        .ok_or_else(|| {
            CodegenError::cranelift(
                format!(
                    "prompt `{callee_name}` metadata missing from ir_prompts — declare-pass invariant violated"
                ),
                span,
            )
        })?;

    if prompt.params.len() != args.len() {
        return Err(CodegenError::cranelift(
            format!(
                "prompt `{callee_name}` declared with {} param(s) but called with {}",
                prompt.params.len(),
                args.len()
            ),
            span,
        ));
    }

    // 1. Lower each arg expression. Refcounted args (Strings) come
    //    back as Owned (+1).
    let mut arg_vals: Vec<ClValue> = Vec::with_capacity(args.len());
    for a in args {
        arg_vals.push(lower_expr(builder, a, env, func_ids_by_def, module, runtime)?);
    }

    // 2. Parse the template into segments at codegen time.
    let segments = parse_prompt_template(&prompt.template, &prompt.params, span)?;

    // 3. Build the rendered-prompt CorvidString by emitting concat ops
    //    over (literal | stringified arg) parts.
    let mut parts: Vec<ClValue> = Vec::with_capacity(segments.len());
    // Track which arg values we used for stringification so we can
    // release the originals at scope end. Already-Owned values from
    // lower_expr need their +1 dropped after the call sequence.
    for seg in &segments {
        let part = match seg {
            TemplateSegment::Literal(text) => {
                emit_string_const(builder, module, runtime, text, span)?
            }
            TemplateSegment::Param(idx) => {
                let av = arg_vals[*idx];
                let aty = &args[*idx].ty;
                emit_stringify_arg(builder, module, runtime, av, aty, span)?
            }
        };
        parts.push(part);
    }
    let rendered = emit_concat_chain(builder, module, runtime, parts, span)?;

    // 4. Build the constant CorvidStrings for prompt name, signature,
    //    and model. The model is left empty so the runtime falls back
    //    to `default_model` from `CORVID_MODEL`.
    let prompt_name_val = emit_string_const(builder, module, runtime, &prompt.name, span)?;
    let signature_val = emit_string_const(
        builder,
        module,
        runtime,
        &format_prompt_signature(&prompt),
        span,
    )?;
    let model_val = emit_string_const(builder, module, runtime, "", span)?;

    // 5. Call the typed bridge by return type.
    let bridge_id = match return_ty {
        Type::Int => runtime.prompt_call_int,
        Type::Bool => runtime.prompt_call_bool,
        Type::Float => runtime.prompt_call_float,
        Type::String => runtime.prompt_call_string,
        other => {
            return Err(CodegenError::not_supported(
                format!(
                    "prompt `{callee_name}` returns `{}` — Phase 15 supports Int / Bool / Float / String returns; structured returns defer to Phase 20 (`Grounded<T>`)",
                    other.display_name()
                ),
                span,
            ));
        }
    };
    let fref = module.declare_func_in_func(bridge_id, builder.func);
    let call = builder
        .ins()
        .call(fref, &[prompt_name_val, signature_val, rendered, model_val]);
    let result_vals: Vec<ClValue> =
        builder.inst_results(call).iter().copied().collect();

    // 6. Release the four CorvidString constants we passed in (each
    //    came back from emit_string_const as Owned +1; the bridge
    //    is borrow-only on its String args same as #[tool] wrappers).
    emit_release(builder, module, runtime, prompt_name_val);
    emit_release(builder, module, runtime, signature_val);
    emit_release(builder, module, runtime, rendered);
    emit_release(builder, module, runtime, model_val);

    // Release the original arg values (we passed their stringified
    // copies into the rendered prompt, but the originals still hold
    // the +1 they came back from `lower_expr` with). For String args,
    // we passed the value through stringify-as-identity, so the +1
    // already got consumed by `emit_concat_chain`. For non-String,
    // the original is still held.
    for (v, a) in arg_vals.iter().zip(args.iter()) {
        if is_refcounted_type(&a.ty) {
            // String args: ownership transferred into the concat
            // chain (released there). Skip.
        } else {
            // Non-refcounted (Int/Bool/Float): nothing to release.
            let _ = v;
        }
    }

    if result_vals.len() != 1 {
        return Err(CodegenError::cranelift(
            format!(
                "prompt bridge returned {} values; expected 1 for return type `{}`",
                result_vals.len(),
                return_ty.display_name()
            ),
            span,
        ));
    }
    Ok(result_vals[0])
}

/// Render a prompt's signature for the LLM's system prompt context.
/// Format: `name(p1: T1, p2: T2) -> R`. Same as what a Corvid user
/// would write in the source — gives the LLM the typed function
/// contract to implement.
fn format_prompt_signature(p: &corvid_ir::IrPrompt) -> String {
    let params = p
        .params
        .iter()
        .map(|param| format!("{}: {}", param.name, param.ty.display_name()))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{}({}) -> {}", p.name, params, p.return_ty.display_name())
}

fn tool_wrapper_symbol(tool_name: &str) -> String {
    let mangled: String = tool_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("__corvid_tool_{mangled}")
}

/// Layout (single allocation):
/// ```text
///   offset 0:  refcount (8) = i64::MIN  (immortal sentinel)
///   offset 8:  reserved (8) = 0
///   offset 16: bytes_ptr (8) = self + 32 (relocated)
///   offset 24: length (8)
///   offset 32: bytes (length bytes)
/// ```
/// The compiled value is `symbol_value(self) + 16`, pointing at the
/// descriptor (matching what `corvid_alloc` returns for heap strings).
fn lower_string_literal(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    s: &str,
    span: Span,
) -> Result<ClValue, CodegenError> {
    let id = runtime.next_literal_id();
    let symbol_name = format!("corvid_lit_{id}");
    let bytes = s.as_bytes();
    let len = bytes.len() as i64;
    let total = 32 + bytes.len();
    let mut data = vec![0u8; total];
    // refcount = i64::MIN (immortal)
    data[0..8].copy_from_slice(&i64::MIN.to_le_bytes());
    // typeinfo_ptr (offset 8) — relocation below points it at
    // `corvid_typeinfo_String` so runtime tracers can dispatch
    // uniformly through the same typeinfo path as heap-allocated
    // strings.
    // bytes_ptr placeholder at offset 16 — written by the relocation
    // length at offset 24
    data[24..32].copy_from_slice(&len.to_le_bytes());
    // bytes at offset 32
    if !bytes.is_empty() {
        data[32..].copy_from_slice(bytes);
    }

    let data_id = module
        .declare_data(&symbol_name, Linkage::Local, false, false)
        .map_err(|e| {
            CodegenError::cranelift(format!("declare literal `{symbol_name}`: {e}"), span)
        })?;
    let mut desc = DataDescription::new();
    desc.set_align(8);
    desc.define(data.into_boxed_slice());
    // typeinfo_ptr at offset 8 → &corvid_typeinfo_String
    let ti_gv = module.declare_data_in_data(runtime.string_typeinfo, &mut desc);
    desc.write_data_addr(8, ti_gv, 0);
    // Self-relative relocation: at offset 16, write the address of
    // (this same symbol + 32) so `bytes_ptr` points at the inline bytes.
    let self_gv = module.declare_data_in_data(data_id, &mut desc);
    desc.write_data_addr(16, self_gv, 32);
    module
        .define_data(data_id, &desc)
        .map_err(|e| CodegenError::cranelift(format!("define literal `{symbol_name}`: {e}"), span))?;

    // The String value is the address of the descriptor (symbol + 16),
    // matching what `corvid_alloc` returns for heap strings.
    let gv = module.declare_data_in_func(data_id, builder.func);
    let symbol_addr = builder.ins().symbol_value(I64, gv);
    Ok(builder.ins().iadd_imm(symbol_addr, 16))
}

/// Implicit-promote mixed `Int + Float` operands to `Float`. Same rule
/// as the interpreter's `eval_arithmetic`. Returns the (possibly
/// promoted) operands and the resulting arithmetic domain.
fn promote_arith(
    builder: &mut FunctionBuilder,
    l: ClValue,
    r: ClValue,
    span: Span,
) -> Result<(ClValue, ClValue, ArithDomain), CodegenError> {
    let lt = builder.func.dfg.value_type(l);
    let rt = builder.func.dfg.value_type(r);
    if lt == F64 && rt == F64 {
        return Ok((l, r, ArithDomain::Float));
    }
    if lt == I64 && rt == I64 {
        return Ok((l, r, ArithDomain::Int));
    }
    // Bool == Bool is Int domain — both sides are I8.
    if lt == I8 && rt == I8 {
        return Ok((l, r, ArithDomain::Int));
    }
    if lt == I64 && rt == F64 {
        let l_promoted = builder.ins().fcvt_from_sint(F64, l);
        return Ok((l_promoted, r, ArithDomain::Float));
    }
    if lt == F64 && rt == I64 {
        let r_promoted = builder.ins().fcvt_from_sint(F64, r);
        return Ok((l, r_promoted, ArithDomain::Float));
    }
    Err(CodegenError::cranelift(
        format!(
            "unsupported operand width combination for binop: {lt:?} and {rt:?} — typecheck should have caught this"
        ),
        span,
    ))
}

/// Lower unary operators.
///
/// - `Not` flips a Bool via `icmp_eq(v, 0)` — 0→1, 1→0 — and produces `I8`.
/// - `Neg` on `Int` is `0 - x` with overflow trap, matching the
///   interpreter's `checked_neg` semantics for `i64::MIN`.
fn lower_unop(
    builder: &mut FunctionBuilder,
    op: UnaryOp,
    v: ClValue,
    span: Span,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<ClValue, CodegenError> {
    let vt = builder.func.dfg.value_type(v);
    match op {
        UnaryOp::Not => {
            let zero = builder.ins().iconst(I8, 0);
            Ok(builder.ins().icmp(IntCC::Equal, v, zero))
        }
        UnaryOp::Neg if vt == F64 => {
            // Float negation is IEEE — flips the sign bit, no trap. NaN
            // negation produces NaN with the sign flipped, also fine.
            Ok(builder.ins().fneg(v))
        }
        UnaryOp::Neg if vt == I64 => {
            // Int `-x` ≡ `0 - x`, trap on overflow (only at i64::MIN).
            let zero = builder.ins().iconst(I64, 0);
            with_overflow_trap(builder, zero, v, module, runtime, |b| {
                b.ins().ssub_overflow(zero, v)
            })
        }
        UnaryOp::Neg => Err(CodegenError::cranelift(
            format!("unary `-` applied to value of width {vt:?} — typecheck should have caught this"),
            span,
        )),
    }
}

/// Short-circuit `and`/`or`.
///
/// Implementation: evaluate the left operand; branch on it. The "short
/// path" skips the right operand entirely and jumps to the merge block
/// with a constant (0 for `and`, 1 for `or`). The "evaluate path"
/// executes the right operand and forwards its value. Merge block
/// receives an `I8` block parameter carrying the chosen result.
fn lower_short_circuit(
    builder: &mut FunctionBuilder,
    op: BinaryOp,
    left: &IrExpr,
    right: &IrExpr,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<ClValue, CodegenError> {
    let l = lower_expr(builder, left, env, func_ids_by_def, module, runtime)?;

    let right_block = builder.create_block();
    let merge_block = builder.create_block();
    let result = builder.append_block_param(merge_block, I8);

    match op {
        BinaryOp::And => {
            // l != 0 → eval right; l == 0 → short-circuit to false.
            let short_val = builder.ins().iconst(I8, 0);
            builder
                .ins()
                .brif(l, right_block, &[], merge_block, &[short_val.into()]);
        }
        BinaryOp::Or => {
            // l != 0 → short-circuit to true; l == 0 → eval right.
            let short_val = builder.ins().iconst(I8, 1);
            builder
                .ins()
                .brif(l, merge_block, &[short_val.into()], right_block, &[]);
        }
        _ => unreachable!("lower_short_circuit only handles And/Or"),
    }

    builder.switch_to_block(right_block);
    builder.seal_block(right_block);
    let r = lower_expr(builder, right, env, func_ids_by_def, module, runtime)?;
    builder.ins().jump(merge_block, &[r.into()]);

    builder.switch_to_block(merge_block);
    builder.seal_block(merge_block);
    Ok(result)
}

/// Lower an `if` / `else` statement into CL blocks.
///
/// Pattern: cond_block emits `brif`; then/else blocks lower their
/// bodies; both, if they fall through, `jump` to a merge block. If
/// neither falls through, merge is terminated with a trap (dead code)
/// and the enclosing `lower_block` is told the statement terminated.
fn lower_if(
    builder: &mut FunctionBuilder,
    cond: &IrExpr,
    then_block_ir: &IrBlock,
    else_block_ir: Option<&IrBlock>,
    env: &mut HashMap<LocalId, (Variable, clir::Type)>,
    var_idx: &mut usize,
    scope_stack: &mut Vec<Vec<(LocalId, Variable)>>,
    loop_stack: &mut Vec<LoopCtx>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<BlockOutcome, CodegenError> {
    let cond_val = lower_expr(builder, cond, env, func_ids_by_def, module, runtime)?;

    let then_b = builder.create_block();
    let else_b = if else_block_ir.is_some() {
        Some(builder.create_block())
    } else {
        None
    };
    let merge_b = builder.create_block();
    let false_target = else_b.unwrap_or(merge_b);
    builder.ins().brif(cond_val, then_b, &[], false_target, &[]);

    // `any_fell_through` starts true when there's no else because the
    // cond-false path flows straight to merge via the brif above.
    let mut any_fell_through = else_b.is_none();

    // Then branch — push a new scope for branch-local refcounted Lets;
    // pop after lowering, releasing each local's refcount if the
    // branch fell through normally.
    builder.switch_to_block(then_b);
    builder.seal_block(then_b);
    scope_stack.push(Vec::new());
    let then_outcome = lower_block(
        builder,
        then_block_ir,
        env,
        var_idx,
        scope_stack,
        loop_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;
    if matches!(then_outcome, BlockOutcome::Normal) {
        // Release branch-scope refcounted locals before jumping to merge.
        let scope = scope_stack.pop().unwrap_or_default();
        for (_, var) in scope.iter().rev() {
            let v = builder.use_var(*var);
            emit_release(builder, module, runtime, v);
        }
        builder.ins().jump(merge_b, &[]);
        any_fell_through = true;
    } else {
        // Branch terminated (return) — its return path already emitted
        // releases for all live locals across all scopes. Just pop.
        scope_stack.pop();
    }

    // Else branch (if present).
    if let (Some(else_b), Some(else_body)) = (else_b, else_block_ir) {
        builder.switch_to_block(else_b);
        builder.seal_block(else_b);
        scope_stack.push(Vec::new());
        let else_outcome = lower_block(
            builder,
            else_body,
            env,
            var_idx,
            scope_stack,
            loop_stack,
            func_ids_by_def,
            module,
            runtime,
        )?;
        if matches!(else_outcome, BlockOutcome::Normal) {
            let scope = scope_stack.pop().unwrap_or_default();
            for (_, var) in scope.iter().rev() {
                let v = builder.use_var(*var);
                emit_release(builder, module, runtime, v);
            }
            builder.ins().jump(merge_b, &[]);
            any_fell_through = true;
        } else {
            scope_stack.pop();
        }
    }

    builder.switch_to_block(merge_b);
    builder.seal_block(merge_b);
    if !any_fell_through {
        // Nothing flows here — both branches returned. Terminate the
        // unreachable merge with a trap so Cranelift's verifier is
        // satisfied, and tell the enclosing block the statement
        // terminated.
        builder
            .ins()
            .trap(cranelift_codegen::ir::TrapCode::INTEGER_OVERFLOW);
        return Ok(BlockOutcome::Terminated);
    }
    Ok(BlockOutcome::Normal)
}

/// Run an overflow-producing Cranelift op, branch to an overflow handler
/// block on the flag, and return the sum/diff/product value.
fn with_overflow_trap<F>(
    builder: &mut FunctionBuilder,
    _l: ClValue,
    _r: ClValue,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    op: F,
) -> Result<ClValue, CodegenError>
where
    F: FnOnce(&mut FunctionBuilder) -> (ClValue, ClValue),
{
    let (result, overflow) = op(builder);
    let overflow_block = builder.create_block();
    let cont_block = builder.create_block();
    builder
        .ins()
        .brif(overflow, overflow_block, &[], cont_block, &[]);

    builder.switch_to_block(overflow_block);
    builder.seal_block(overflow_block);
    let callee_ref = module.declare_func_in_func(runtime.overflow, builder.func);
    builder.ins().call(callee_ref, &[]);
    builder.ins().trap(cranelift_codegen::ir::TrapCode::INTEGER_OVERFLOW);

    builder.switch_to_block(cont_block);
    builder.seal_block(cont_block);
    Ok(result)
}

fn trap_on_zero(
    builder: &mut FunctionBuilder,
    divisor: ClValue,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) {
    let zero = builder.ins().iconst(I64, 0);
    let is_zero = builder.ins().icmp(IntCC::Equal, divisor, zero);
    let trap_block = builder.create_block();
    let cont_block = builder.create_block();
    builder
        .ins()
        .brif(is_zero, trap_block, &[], cont_block, &[]);
    builder.switch_to_block(trap_block);
    builder.seal_block(trap_block);
    let callee_ref = module.declare_func_in_func(runtime.overflow, builder.func);
    builder.ins().call(callee_ref, &[]);
    builder.ins().trap(cranelift_codegen::ir::TrapCode::INTEGER_OVERFLOW);
    builder.switch_to_block(cont_block);
    builder.seal_block(cont_block);
}

// Silence warnings for fields we'll start using in slice 12b.
#[allow(dead_code)]
fn _force_use(_: MemFlags, _: Signature) {}
