//! IR → Cranelift IR lowering.
//!
//! Slice 12a scope: `Int` parameters, `Int` return, `Int` literals,
//! `Int` arithmetic with overflow trap, agent-to-agent calls,
//! parameter loads, `return`. Everything else raises
//! `CodegenError::NotSupported` with a slice pointer so the boundary
//! is auditable.

use crate::errors::CodegenError;
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::types::{F64, I64, I8};
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
fn mangle_agent_symbol(user_name: &str) -> String {
    format!("corvid_agent_{user_name}")
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

    // corvid_alloc(i64) -> i64
    let mut alloc_sig = module.make_signature();
    alloc_sig.params.push(AbiParam::new(I64));
    alloc_sig.returns.push(AbiParam::new(I64));
    let alloc_id = module
        .declare_function(ALLOC_SYMBOL, Linkage::Import, &alloc_sig)
        .map_err(|e| CodegenError::cranelift(format!("declare alloc: {e}"), Span::new(0, 0)))?;

    // corvid_alloc_with_destructor(i64, fn_ptr) -> i64
    let mut alloc_dtor_sig = module.make_signature();
    alloc_dtor_sig.params.push(AbiParam::new(I64));
    alloc_dtor_sig.params.push(AbiParam::new(I64));
    alloc_dtor_sig.returns.push(AbiParam::new(I64));
    let alloc_dtor_id = module
        .declare_function(ALLOC_WITH_DESTRUCTOR_SYMBOL, Linkage::Import, &alloc_dtor_sig)
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare alloc_with_destructor: {e}"),
                Span::new(0, 0),
            )
        })?;

    // corvid_destroy_list_refcounted(payload) -> void
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

    Ok(RuntimeFuncs {
        overflow: overflow_func_id,
        retain: retain_id,
        release: release_id,
        string_concat: concat_id,
        string_eq: eq_id,
        string_cmp: cmp_id,
        alloc: alloc_id,
        alloc_with_destructor: alloc_dtor_id,
        list_destroy_refcounted: list_destroy_id,
        literal_counter: std::cell::Cell::new(0),
        struct_destructors: HashMap::new(),
        ir_types: HashMap::new(),
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

/// `void* corvid_alloc(long long payload_bytes)` — heap-allocate an
/// N-byte payload behind the 16-byte refcount header. Used by Struct
/// types that have no refcounted fields (no destructor needed).
pub const ALLOC_SYMBOL: &str = "corvid_alloc";

/// `void* corvid_alloc_with_destructor(long long size, void(*dtor)(void*))`
/// — like `corvid_alloc` but stores a destructor function pointer in
/// the header's `reserved` slot. Used by Struct types that have at
/// least one refcounted field; the destructor releases those fields
/// before the block is freed.
pub const ALLOC_WITH_DESTRUCTOR_SYMBOL: &str = "corvid_alloc_with_destructor";

/// `void corvid_destroy_list_refcounted(void* payload)` — the shared
/// runtime destructor for all refcounted-element list types. Walks
/// the length at offset 0 of the payload and calls `corvid_release`
/// on each element. Non-refcounted-element lists (List<Int> etc.)
/// don't need a destructor at all; they use plain `corvid_alloc`.
pub const LIST_DESTROY_SYMBOL: &str = "corvid_destroy_list_refcounted";

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
    pub alloc: FuncId,
    pub alloc_with_destructor: FuncId,
    /// Shared destructor for refcounted-element lists. One function
    /// handles all such lists at runtime — each element is an I64
    /// pointer, and `corvid_release` does the per-type cleanup via
    /// each element's own header chain.
    pub list_destroy_refcounted: FuncId,
    pub literal_counter: std::cell::Cell<u64>,
    /// Per-struct-type destructors generated in `lower_file` for
    /// structs with at least one refcounted field. Missing entries
    /// mean the struct has no refcounted fields (uses plain
    /// `corvid_alloc` at construction time, no destructor invocation).
    pub struct_destructors: HashMap<DefId, FuncId>,
    /// Owned copy of the IR's struct type metadata, keyed by `DefId`.
    /// Cloned into `RuntimeFuncs` in `lower_file` so the per-agent
    /// lowering functions can resolve struct layouts (for field
    /// offsets, constructor arity checks, destructor lookup) without
    /// threading `&IrFile` through every call site.
    pub ir_types: HashMap<DefId, corvid_ir::IrType>,
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

    // Declare and define per-struct-type destructors for every user
    // struct with at least one refcounted field. These must land
    // before agent bodies are lowered so `IrCallKind::StructConstructor`
    // can reference them via `runtime.struct_destructors`.
    for ty in &ir.types {
        if ty.fields.iter().any(|f| is_refcounted_type(&f.ty)) {
            let dtor_id = define_struct_destructor(module, ty, &runtime)?;
            runtime.struct_destructors.insert(ty.id, dtor_id);
        }
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
        let mangled = mangle_agent_symbol(&agent.name);
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
        let entry_return_ty = cl_type_for(&entry_agent.return_ty, entry_agent.span)?;
        emit_entry_trampoline(module, entry_func_id, entry_return_ty)?;
    }

    Ok(func_ids)
}

/// Emit `long long corvid_entry(void)` that calls `entry_func_id` and
/// returns its result. If the entry agent returns an `I8` (Bool), the
/// trampoline zero-extends to `I64` so the C shim's `long long` ABI is
/// satisfied on every path.
fn emit_entry_trampoline(
    module: &mut ObjectModule,
    entry_func_id: FuncId,
    entry_return_ty: clir::Type,
) -> Result<(), CodegenError> {
    let mut sig = module.make_signature();
    sig.returns.push(AbiParam::new(I64));
    let tramp_id = module
        .declare_function(ENTRY_TRAMPOLINE_SYMBOL, Linkage::Export, &sig)
        .map_err(|e| {
            CodegenError::cranelift(
                format!("declare corvid_entry trampoline: {e}"),
                Span::new(0, 0),
            )
        })?;

    let mut ctx = Context::new();
    ctx.func = Function::with_name_signature(
        UserFuncName::user(0, tramp_id.as_u32()),
        module
            .declarations()
            .get_function_decl(tramp_id)
            .signature
            .clone(),
    );

    let mut builder_ctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let callee_ref = module.declare_func_in_func(entry_func_id, builder.func);
        let call = builder.ins().call(callee_ref, &[]);
        let results = builder.inst_results(call);
        let raw = if results.is_empty() {
            builder.ins().iconst(I64, 0)
        } else {
            results[0]
        };
        // Widen Bool → I64 for the C shim's `long long` contract.
        let ret = if entry_return_ty == I8 {
            builder.ins().uextend(I64, raw)
        } else {
            raw
        };
        builder.ins().return_(&[ret]);
        builder.finalize();
    }

    module
        .define_function(tramp_id, &mut ctx)
        .map_err(|e| {
            CodegenError::cranelift(
                format!("define corvid_entry trampoline: {e}"),
                Span::new(0, 0),
            )
        })?;
    Ok(())
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
        IrStmt::Approve { span, .. } => Err(CodegenError::not_supported(
            "`approve` in compiled code — Phase 14 adds it alongside the tool registry",
            *span,
        )),
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
            IrCallKind::Tool { .. } | IrCallKind::Prompt { .. } => Err(CodegenError::not_supported(
                "tool / prompt calls in compiled code — Phase 14 adds them",
                expr.span,
            )),
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
            let elem_refcounted = is_refcounted_type(&elem_ty);
            // Allocation size: 8 (length) + 8 * N (elements).
            let total_bytes = 8 + 8 * items.len() as i64;
            let size_val = builder.ins().iconst(I64, total_bytes);
            // Choose allocator: with destructor if elements are refcounted.
            let list_ptr = if elem_refcounted {
                let alloc_ref =
                    module.declare_func_in_func(runtime.alloc_with_destructor, builder.func);
                let dtor_ref = module
                    .declare_func_in_func(runtime.list_destroy_refcounted, builder.func);
                let dtor_addr = builder.ins().func_addr(I64, dtor_ref);
                let call = builder.ins().call(alloc_ref, &[size_val, dtor_addr]);
                builder.inst_results(call)[0]
            } else {
                let alloc_ref = module.declare_func_in_func(runtime.alloc, builder.func);
                let call = builder.ins().call(alloc_ref, &[size_val]);
                builder.inst_results(call)[0]
            };
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
    // Pick the allocator based on whether this struct has a destructor.
    let struct_ptr = if let Some(&dtor_id) = runtime.struct_destructors.get(&ty.id) {
        let alloc_ref = module.declare_func_in_func(runtime.alloc_with_destructor, builder.func);
        let dtor_ref = module.declare_func_in_func(dtor_id, builder.func);
        let sig = module.declarations().get_function_decl(dtor_id).signature.clone();
        let sig_ref = builder.func.import_signature(sig);
        let dtor_addr = builder.ins().func_addr(I64, dtor_ref);
        let _ = sig_ref; // not strictly needed — we pass dtor as an opaque i64 pointer
        let call = builder.ins().call(alloc_ref, &[size, dtor_addr]);
        builder.inst_results(call)[0]
    } else {
        let alloc_ref = module.declare_func_in_func(runtime.alloc, builder.func);
        let call = builder.ins().call(alloc_ref, &[size]);
        builder.inst_results(call)[0]
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
    // reserved = 0 (already zeroed)
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
