//! IR ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢ Cranelift IR lowering.
//!
//! Native lowering starts with the scalar command-line boundary:
//! `Int` parameters and returns, integer literals, integer arithmetic
//! with overflow traps, agent-to-agent calls, parameter loads, and
//! `return`. Unsupported features raise `CodegenError::NotSupported`
//! with a descriptive feature boundary.

use crate::errors::CodegenError;
use cranelift_codegen::binemit::CodeOffset;
use cranelift_codegen::control::ControlPlane;
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::types::{F64, I32, I64, I8};
use cranelift_codegen::ir::{
    self as clir, AbiParam, Function, InstBuilder, MemFlags, Signature, UserFuncName,
    UserStackMap, Value as ClValue,
};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{DataDescription, FuncId, Linkage, Module};
use cranelift_object::ObjectModule;
use corvid_ast::{Backoff, BinaryOp, Span, UnaryOp};
use corvid_ir::{IrAgent, IrBlock, IrCallKind, IrExpr, IrExprKind, IrFile, IrLiteral, IrStmt};
use corvid_resolve::{DefId, LocalId};
use corvid_types::Type;
use std::collections::{BTreeSet, HashMap};

const _: () = {
    // A readable reminder: the earliest native path compiled only Int. Type checks
    // elsewhere should already have enforced this for well-typed programs.
};

/// Mangle a user agent's name into a link-safe symbol. Prevents
/// collisions with C runtime symbols (`main`, `printf`, `malloc`, ...).
///
/// Include the agent's `DefId` in the symbol because methods
/// declared inside `extend T:` blocks share their unmangled names
/// across types (`Order.total`, `Line.total` both get the AST name
/// `total`). Including the DefId disambiguates without changing the
/// emitted .obj's user-visible behavior ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â symbols are internal-only
/// (`Linkage::Local`) so the suffix never leaks into a public API.
fn mangle_agent_symbol(user_name: &str, def_id: DefId) -> String {
    format!("corvid_agent_{user_name}_{}", def_id.0)
}

/// Declare the imported runtime helpers (retain, release, string
/// concat / eq / cmp) and bundle their FuncIds with the overflow
/// handler into a `RuntimeFuncs`.
mod runtime;
use runtime::*;
mod expr;
use expr::*;
mod stmt;
use stmt::*;
mod prompt;
use prompt::*;

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

    // Same pattern for tool declarations so the Cranelift
    // `IrCallKind::Tool` lowering can look up param + return types and
    // declare the matching `__corvid_tool_<name>` wrapper import.
    for tool in &ir.tools {
        runtime.ir_tools.insert(tool.id, tool.clone());
    }

    // Same pattern for prompt declarations. `IrCallKind::Prompt`
    // lowering reads each prompt's params + template + return type to
    // emit signature-aware bridge calls.
    for prompt in &ir.prompts {
        runtime.ir_prompts.insert(prompt.id, prompt.clone());
    }

    // Capture per-agent borrow_sig into the runtime
    // table so call-site lowering can consult it without threading
    // `&IrFile` through every function. Agents with `borrow_sig =
    // None` fall through to the pre-17b behavior (all params Owned).
    for agent in &ir.agents {
        if let Some(sig) = &agent.borrow_sig {
            runtime.agent_borrow_sigs.insert(agent.id, sig.clone());
        }
    }

    // Emit per-type metadata in dependency order:
    //
    //   1. Struct destructors (existing): release refcounted fields
    //      when rcÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢0. Only for structs with refcounted fields.
    //   2. Struct trace fns (new in 17a): walk refcounted fields for
    //      the collector's mark walk. Emitted for every refcounted struct ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â
    //      even ones with no refcounted fields get an empty-body
    //      trace so dispatch is uniform (linker folds duplicates).
    //   3. Struct typeinfo blocks (new): .rodata record referenced
    //      from the allocation header. Relocations point at the
    //      destructor + trace fns emitted above.
    //   4. Result destructors / traces / typeinfos: one per concrete
    //      Result<T, E> type mentioned in the IR.
    //   5. Wide Option<T> traces / typeinfos: one per concrete scalar
    //      Option<T> type mentioned in the IR.
    //   6. List typeinfo blocks (new): one per concrete List<T> type
    //      walked out of the IR. Element-types emit first so outer
    //      list typeinfos can reference them via elem_typeinfo.
    //
    // All must land before agent bodies are lowered so
    // IrCallKind::StructConstructor and IrExprKind::List have
    // typeinfos to reference at allocation sites.

    // Structs: destructors (only for refcounted fields), traces
    // (every struct, empty body if no refcounted fields ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â linker
    // folds duplicates), typeinfos (every struct, uniform allocation
    // path). The pre-17a "primitive-only structs skip typeinfo"
    // short-circuit is gone ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â uniformity means 17d doesn't need a
    // special case for them in the mark walk.
    for ty in &ir.types {
        let has_refcounted_field = ty.fields.iter().any(|f| is_refcounted_type(&f.ty));
        let destroy_id = if has_refcounted_field {
            let id = define_struct_destructor(module, ty, &runtime)?;
            runtime.struct_destructors.insert(ty.id, id);
            Some(id)
        } else {
            None
        };
        let trace_id = define_struct_trace(module, ty, &runtime)?;
        runtime.struct_traces.insert(ty.id, trace_id);
        let typeinfo_id = emit_struct_typeinfo(module, ty, destroy_id, trace_id, &runtime)?;
        runtime.struct_typeinfos.insert(ty.id, typeinfo_id);
    }

    // Results: fixed-size tagged wrappers. Each wrapper stores
    // `[tag: i64 | payload-slot: 8B]`. The concrete per-type
    // destructor/trace decides whether the active branch payload
    // needs refcount release/marking.
    for result_ty in collect_result_types(ir) {
        let (ok_ty, err_ty) = match &result_ty {
            Type::Result(ok, err) => ((**ok).clone(), (**err).clone()),
            _ => continue,
        };
        let has_refcounted_branch = is_refcounted_type(&ok_ty) || is_refcounted_type(&err_ty);
        let destroy_id = if has_refcounted_branch {
            let id = define_result_destructor(module, &result_ty, &ok_ty, &err_ty, &runtime)?;
            runtime.result_destructors.insert(result_ty.clone(), id);
            Some(id)
        } else {
            None
        };
        let trace_id = define_result_trace(module, &result_ty, &ok_ty, &err_ty, &runtime)?;
        runtime.result_traces.insert(result_ty.clone(), trace_id);
        let typeinfo_id = emit_result_typeinfo(module, &result_ty, destroy_id, trace_id, &runtime)?;
        runtime.result_typeinfos.insert(result_ty, typeinfo_id);
    }

    // Wrapper-backed options: `None` stays the zero pointer, while
    // `Some(...)` allocates a tiny typed wrapper storing the payload in
    // one slot. This is required both for wide scalar payloads and for
    // nested `Option<T>` shapes where bare nullable-pointer encoding
    // would collapse `Some(None)` into outer `None`.
    for option_ty in collect_option_types(ir) {
        let payload_ty = match &option_ty {
            Type::Option(inner) => (**inner).clone(),
            _ => continue,
        };
        let destroy_id = if is_refcounted_type(&payload_ty) {
            Some(define_option_destructor(module, &option_ty, &payload_ty, &runtime)?)
        } else {
            None
        };
        let trace_id = define_option_trace(module, &option_ty, &payload_ty, &runtime)?;
        let typeinfo_id = emit_option_typeinfo(module, &option_ty, destroy_id, trace_id, &runtime)?;
        runtime.option_typeinfos.insert(option_ty, typeinfo_id);
    }

    // Lists: collect every concrete element type the IR mentions, in
    // dependency order (inner before outer), then emit one typeinfo
    // per concrete list type. For refcounted element types the
    // elem_typeinfo slot gets a relocation to the element's typeinfo
    // (String built-in, struct, nested result, or nested list).
    let list_elem_types = collect_list_element_types(ir);
    for elem_ty in list_elem_types {
        let elem_typeinfo_data_id = typeinfo_data_for_refcounted_payload(&elem_ty, &runtime);
        let list_ti_id =
            emit_list_typeinfo(module, &elem_ty, elem_typeinfo_data_id, &runtime)?;
        runtime.list_typeinfos.insert(elem_ty, list_ti_id);
    }

    // Pass 1: declare every agent. Signatures are Int + Bool only for
    // the early native tier. Symbols are mangled so user agent names (e.g. `main`,
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

    // The unified ownership pass is active by
    // default (see `runtime.dup_drop_enabled` populated from
    // `CORVID_DUP_DROP_PASS` in `declare_runtime_funcs`). When on,
    // each agent's IR gets the `insert_dup_drop` transformation
    // BEFORE codegen sees it, and every scattered
    // `emit_retain`/`emit_release` site in the expression lowerings
    // guards itself on `runtime.dup_drop_enabled` so the two paths
    // don't double-count.

    // Pass 2: define each agent's body.
    for agent in &ir.agents {
        let &func_id = func_ids_by_def
            .get(&agent.id)
            .expect("declared in pass 1");
        if runtime.dup_drop_enabled {
            let transformed = crate::pair_elim::eliminate_pairs(crate::dup_drop::insert_dup_drop(agent));
            let effect_info = crate::scope_reduce::analyze_effects(&transformed);
            let transformed = crate::scope_reduce::reduce_scope(transformed, &effect_info);
            runtime.prompt_pins = crate::latency_rc::analyze_prompt_pins(&transformed)
                .pinned_by_span()
                .clone();
            define_agent(module, &transformed, func_id, &func_ids_by_def, &runtime)?;
        } else {
            runtime.prompt_pins.clear();
            define_agent(module, agent, func_id, &func_ids_by_def, &runtime)?;
        }
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
        // Codegen-emitted main calls `corvid_runtime_init()`
        // and registers `corvid_runtime_shutdown` via atexit ONLY if the
        // program actually uses the async runtime. Pure-computation
        // programs skip these calls to preserve startup
        // benchmark numbers ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â multi-thread tokio startup is ~5-10ms on
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

    // Emit the `corvid_stack_maps` table after every
    // function has been compiled (so `runtime.stack_maps` is fully
    // populated). Emit even when empty so downstream consumers
    // (`corvid_stack_maps_find` in the runtime) never hit unresolved-
    // symbol errors on programs with no refcounted values.
    emit_stack_map_table(module, &runtime)?;

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
        IrStmt::Yield { value, .. } => expr_uses_runtime(value),
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
        // Dup/Drop emit direct runtime calls (corvid_retain/release)
        // but those don't need the async bridge ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â runtime is a C ABI
        // library linked in regardless.
        IrStmt::Dup { .. } | IrStmt::Drop { .. } => false,
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
        // Result/Option IR variants recurse into sub-expressions. The
        // wrappers themselves don't use the async runtime, but a
        // `?`-propagated tool call inside `inner` still does.
        IrExprKind::WeakNew { strong: inner }
        | IrExprKind::WeakUpgrade { weak: inner }
        | IrExprKind::ResultOk { inner }
        | IrExprKind::ResultErr { inner }
        | IrExprKind::OptionSome { inner }
        | IrExprKind::TryPropagate { inner } => expr_uses_runtime(inner),
        IrExprKind::OptionNone => false,
        IrExprKind::TryRetry { body, .. } => expr_uses_runtime(body),
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
/// Replaces the original `corvid_entry` trampoline. Now that the
/// codegen knows the entry signature at emit time, generating `main`
/// directly avoids the C-shim-with-introspection trap.
fn emit_entry_main(
    module: &mut ObjectModule,
    entry_agent: &IrAgent,
    entry_func_id: FuncId,
    runtime: &RuntimeFuncs,
    // Emit `corvid_runtime_init()` + `atexit(corvid_runtime_shutdown)`
    // only if the program actually needs the async runtime. Passing `false`
    // keeps compiled binaries as small + fast-starting as they were in
    // startup benchmark baseline.
    uses_runtime: bool,
) -> Result<(), CodegenError> {
    // I32 is imported at file scope since runtime setup needs it in
    // `declare_runtime_funcs` too.

    // Validate that every entry parameter and the return type are
    // representable at the command-line / stdout boundary. Struct and
    // List are deliberately excluded ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â they need a serialization implementation.
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

        // 1. corvid_init() ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â registers atexit handler for leak counters.
        let init_ref = module.declare_func_in_func(runtime.entry_init, builder.func);
        builder.ins().call(init_ref, &[]);

        // If the program uses the async runtime, build
        // the tokio + corvid runtime globals NOW, eagerly. Shutdown is
        // registered via `atexit` so worker threads join cleanly at
        // exit. Shutdown runs BEFORE the leak-counter atexit (atexit
        // is LIFO), so any refcount activity from the runtime settles
        // before the counter prints ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â that's the intended ordering.
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

        let entry_ref = module.declare_func_in_func(entry_func_id, builder.func);
        let bench_enabled_ref =
            module.declare_func_in_func(runtime.bench_server_enabled, builder.func);
        let bench_next_ref =
            module.declare_func_in_func(runtime.bench_next_trial, builder.func);
        let bench_finish_ref =
            module.declare_func_in_func(runtime.bench_finish_trial, builder.func);

        let bench_mode = builder.ins().call(bench_enabled_ref, &[]);
        let bench_enabled = builder.inst_results(bench_mode)[0];
        let bench_check_b = builder.create_block();
        let bench_body_b = builder.create_block();
        builder.append_block_param(bench_body_b, I64);
        let bench_done_b = builder.create_block();
        let normal_call_b = builder.create_block();
        let bench_cmp = builder.ins().icmp_imm(IntCC::NotEqual, bench_enabled, 0);
        builder
            .ins()
            .brif(bench_cmp, bench_check_b, &[], normal_call_b, &[]);

        builder.switch_to_block(bench_check_b);
        let next_trial_call = builder.ins().call(bench_next_ref, &[]);
        let trial_idx = builder.inst_results(next_trial_call)[0];
        let has_trial = builder.ins().icmp_imm(IntCC::NotEqual, trial_idx, 0);
        builder
            .ins()
            .brif(has_trial, bench_body_b, &[trial_idx], bench_done_b, &[]);

        builder.switch_to_block(bench_body_b);
        let trial_idx = builder.block_params(bench_body_b)[0];
        let entry_call = builder.ins().call(entry_ref, &decoded_args);
        builder.ins().call(bench_finish_ref, &[trial_idx]);
        if let Some(result_val) = builder.inst_results(entry_call).first().copied() {
            emit_entry_result_print(
                &mut builder,
                module,
                runtime,
                &entry_agent.return_ty,
                result_val,
            );
        }
        builder.ins().jump(bench_check_b, &[]);
        builder.seal_block(bench_body_b);
        builder.seal_block(bench_check_b);

        builder.switch_to_block(normal_call_b);
        builder.seal_block(normal_call_b);
        let entry_call = builder.ins().call(entry_ref, &decoded_args);
        let result = builder.inst_results(entry_call).first().copied();

        if !runtime.dup_drop_enabled {
            for (v, is_ref) in decoded_args.iter().zip(decoded_refcounted.iter()) {
                if *is_ref {
                    emit_release(&mut builder, module, runtime, *v);
                }
            }
        }
        if let Some(result_val) = result {
            emit_entry_result_print(
                &mut builder,
                module,
                runtime,
                &entry_agent.return_ty,
                result_val,
            );
        }
        let zero = builder.ins().iconst(I32, 0);
        builder.ins().return_(&[zero]);

        builder.switch_to_block(bench_done_b);
        builder.seal_block(bench_done_b);
        let zero = builder.ins().iconst(I32, 0);
        builder.ins().return_(&[zero]);
        builder.finalize();
    }

    define_function_with_stack_maps(
        module,
        main_id,
        &mut ctx,
        runtime,
        entry_agent.span,
        "main",
    )?;
    Ok(())
}

/// Validate that a type is one of the four supported at the
/// command-line / stdout boundary. Struct and List need a
/// dedicated serialization layer; Nothing isn't a sensible
/// CLI value either.
fn check_entry_boundary_type(
    ty: &Type,
    span: Span,
    role: &str,
) -> Result<(), CodegenError> {
    match ty {
        Type::Int | Type::Bool | Type::Float | Type::String => Ok(()),
        Type::Struct(_) | Type::List(_) | Type::Nothing
        | Type::Result(_, _) | Type::Option(_) | Type::Weak(_, _) | Type::Stream(_) => {
            Err(CodegenError::not_supported(
                format!(
                    "entry agent {role} of type `{}` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â the native command-line boundary currently supports only `Int` / `Bool` / `Float` / `String`; structured types (including Result, Option, and Weak) need a dedicated serialization layer (use a wrapper agent that converts internally)",
                    ty.display_name()
                ),
                span,
            ))
        }
        Type::Grounded(inner) => check_entry_boundary_type(inner, span, role),
        Type::Function { .. } | Type::Unknown => Err(CodegenError::cranelift(
            format!("entry agent {role} has un-printable type `{}`", ty.display_name()),
            span,
        )),
    }
}

fn emit_entry_result_print(
    builder: &mut FunctionBuilder<'_>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    return_ty: &Type,
    result_val: ClValue,
) {
    match return_ty {
        Type::Int => {
            let r = module.declare_func_in_func(runtime.print_i64, builder.func);
            builder.ins().call(r, &[result_val]);
        }
        Type::Bool => {
            let widened = builder.ins().uextend(I64, result_val);
            let r = module.declare_func_in_func(runtime.print_bool, builder.func);
            builder.ins().call(r, &[widened]);
        }
        Type::Float => {
            let r = module.declare_func_in_func(runtime.print_f64, builder.func);
            builder.ins().call(r, &[result_val]);
        }
        Type::String => {
            builder.declare_value_needs_stack_map(result_val);
            let r = module.declare_func_in_func(runtime.print_string, builder.func);
            builder.ins().call(r, &[result_val]);
            emit_release(builder, module, runtime, result_val);
        }
        Type::Grounded(inner) => {
            emit_entry_result_print(builder, module, runtime, inner, result_val);
        }
        _ => unreachable!("boundary check rejected non-printable returns"),
    }
}

fn reject_unsupported_types(agent: &IrAgent) -> Result<(), CodegenError> {
    for p in &agent.params {
        cl_type_for(&p.ty, p.span).map_err(|_| {
            CodegenError::not_supported(
                format!(
                    "parameter `{}: {}` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â this native lowering path supports `Int`, `Bool`, and `Float` here; `String`, `Struct`, and `List` use later lowering paths",
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
                "agent `{}` returns `{}` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â this native lowering path supports `Int`, `Bool`, and `Float` returns here",
                agent.name,
                agent.return_ty.display_name()
            ),
            agent.span,
        )
    })?;
    Ok(())
}

/// Map a Corvid `Type` to the Cranelift IR type width we compile it to.
/// `Int` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢ `I64`, `Bool` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â‚¬Å¾Ã‚Â¢ `I8`. Everything else raises
/// `CodegenError::NotSupported` with a descriptive feature boundary.
fn cl_type_for(ty: &Type, span: Span) -> Result<clir::Type, CodegenError> {
    match ty {
        Type::Int => Ok(I64),
        Type::Bool => Ok(I8),
        Type::Float => Ok(F64),
        // String is a pointer to a 16-byte descriptor that lives behind
        // a 16-byte refcount header. Single I64 in registers/env keeps
        // the calling convention uniform with future Struct/List types.
        Type::String => Ok(I64),
        // Struct values are descriptor pointers (like String) ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â single
        // I64 in registers/env. The actual layout lives behind the
        // refcount header: `[header (16) | field0 (8) | ... | fieldN (8)]`.
        Type::Struct(_) => Ok(I64),
        // List values are descriptor pointers (like String, Struct) ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â
        // single I64 pointing at `[length (8) | elements...]` after the
        // 16-byte refcount header.
        Type::List(_) => Ok(I64),
        // Weak values are pointer-sized heap-managed slot boxes.
        Type::Weak(_, _) => Ok(I64),
        Type::Result(_, _) if is_native_result_type(ty) => Ok(I64),
        Type::Option(inner) if is_refcounted_type(inner) => Ok(I64),
        Type::Option(_) if is_native_wide_option_type(ty) => Ok(I64),
        Type::Grounded(inner) => cl_type_for(inner, span),
        Type::Stream(_) => Err(CodegenError::not_supported(
            "`Stream<T>` - Stream lowering is not yet implemented",
            span,
        )),
        Type::Nothing => Err(CodegenError::not_supported(
            "`Nothing` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â use a bare `return` instead",
            span,
        )),
        Type::Function { .. } => Err(CodegenError::not_supported(
            "function types as values ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â first-class callables are not implemented in native codegen yet",
            span,
        )),
        // Result<T,E> and Option<T> are accepted by the typechecker and
        // handled fully by the interpreter. Native tagged-union layout
        // and retry lowering are not implemented yet, so native
        // compilation reports a clean boundary here.
        Type::Result(_, _) => Err(CodegenError::not_supported(
            "`Result<T, E>` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â native tagged-union lowering is not implemented yet; use the interpreter tier (`corvid run --tier interp`) until then",
            span,
        )),
        Type::Option(_) => Err(CodegenError::not_supported(
            "`Option<T>` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â native codegen currently supports nullable-pointer `Option<T>` when `T` is refcounted plus wide scalar `Option<Int|Bool|Float>`; other payload shapes still need the interpreter tier",
            span,
        )),
        Type::Unknown => Err(CodegenError::cranelift(
            "encountered `Unknown` type at codegen ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â typecheck should have caught this",
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
        // Loop context stack ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â empty at function entry. `for` pushes
        // on entry, `break`/`continue` consult the top entry, `for`
        // pops on exit.
        let mut loop_stack: Vec<LoopCtx> = Vec::new();

        for (i, p) in agent.params.iter().enumerate() {
            let block_arg = builder.block_params(entry)[i];
            let var = Variable::from_u32(var_idx as u32);
            var_idx += 1;
            let ty = cl_type_for(&p.ty, p.span)?;
            builder.declare_var(var, ty);
            // Mark the refcounted parameter Value so
            // Cranelift spills it before safepoints and records its
            // SP-relative offset in the function's stack map. The
            // `declare_value_needs_stack_map` API is per-Value (not
            // per-Variable); the safepoint liveness pass handles
            // SSA-phi flow for values that travel through this
            // Variable across blocks.
            //
            // The cycle collector mark walk uses these recorded
            // offsets at each safepoint PC to find on-stack GC roots.
            if is_refcounted_type(&p.ty) {
                builder.declare_value_needs_stack_map(block_arg);
            }
            builder.def_var(var, block_arg);
            env.insert(p.local_id, (var, ty));
            // Borrow ABI driven by `agent.borrow_sig`:
            //   * `ParamBorrow::Owned` (pre-17b default, and what a
            //      consuming body needs): callee retains on entry,
            //      tracks in function-root scope for symmetric
            //      scope-exit release.
            //   * `ParamBorrow::Borrowed`: caller keeps its +1 and
            //      the callee does NOT retain nor release ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â ownership
            //      doesn't cross the ABI. Saves one retain + one
            //      release per call site for read-only parameters.
            //
            // `borrow_sig = None` means the ownership pass didn't
            // run on this agent ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â fall back to Owned (pre-17b
            // semantics). Keeps parity for code paths that bypass
            // `ownership::analyze`.
            if is_refcounted_type(&p.ty) {
                let is_borrowed = agent
                    .borrow_sig
                    .as_ref()
                    .and_then(|v| v.get(i).copied())
                    .map(|b| matches!(b, corvid_ir::ParamBorrow::Borrowed))
                    .unwrap_or(false);
                if !is_borrowed {
                    if !runtime.dup_drop_enabled {
                        emit_retain(&mut builder, module, runtime, block_arg);
                    }
                    // scope_stack tracking is still needed even under
                    // dup_drop_enabled for the fallback path to work
                    // (CORVID_DUP_DROP_PASS=0). The scope walk at
                    // return/break/continue guards its emit_release
                    // calls on the flag, so carrying the entry is
                    // free when the flag is on.
                    scope_stack[0].push((p.local_id, var));
                }
                // Borrowed: no retain, no scope tracking. Caller's
                // +1 stays with the caller; the callee just reads.
            }
        }

        let lowered = lower_block(
            &mut builder,
            &agent.body,
            &agent.return_ty,
            &mut env,
            &mut var_idx,
            &mut scope_stack,
            &mut loop_stack,
            func_ids_by_def,
            module,
            runtime,
        )?;

        // Defensive fallthrough terminator. `lower_block` returns
        // `BlockOutcome::Terminated` after emitting a `return`, so we
        // only reach here for agents whose type checker somehow allowed
        // a missing return (shouldn't happen for `Int` agents). Terminate
        // with a trap so the verifier doesn't reject the function.
        match lowered {
            BlockOutcome::Normal => {
                if builder.current_block().is_some() {
                    let cur = builder.current_block().unwrap();
                    if !builder.func.layout.is_block_inserted(cur) {
                        // Should not happen ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â a block was switched to but
                        // not inserted. Terminator still needed.
                    }
                    // If Cranelift considers the block unterminated,
                    // trap ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â an untyped fallthrough on an `Int` return
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
            BlockOutcome::Terminated => {}
        }

        builder.finalize();
    }

    define_function_with_stack_maps(
        module,
        func_id,
        &mut ctx,
        runtime,
        agent.span,
        &format!("agent `{}`", agent.name),
    )?;
    Ok(())
}

fn lower_try_propagate_option(
    builder: &mut FunctionBuilder,
    expr: &IrExpr,
    inner: &IrExpr,
    current_return_ty: &Type,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    scope_stack: &Vec<Vec<(LocalId, Variable)>>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<ClValue, CodegenError> {
    let (option_payload_ty, wrapped_option) = match &inner.ty {
        Type::Option(payload) if option_uses_wrapper(&inner.ty) => (&**payload, true),
        Type::Option(payload) if is_refcounted_type(payload) => (&**payload, false),
        Type::Option(_) => {
            return Err(CodegenError::not_supported(
                "postfix `?` on `Option<T>` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â native lowering currently supports nullable-pointer `Option<T>` with refcounted payloads plus wide scalar `Option<Int|Bool|Float>`",
                expr.span,
            ))
        }
        Type::Result(_, _) => {
            return Err(CodegenError::not_supported(
                "postfix `?` on `Result<T, E>` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â native tagged-union lowering is not implemented yet; use the interpreter tier until then",
                expr.span,
            ))
        }
        _ => {
            return Err(CodegenError::cranelift(
                format!(
                    "postfix `?` saw non-Option inner type `{}` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â typecheck should have caught this",
                    inner.ty.display_name()
                ),
                expr.span,
            ))
        }
    };

    match current_return_ty {
        Type::Option(ret_payload) if is_refcounted_type(ret_payload) => {}
        Type::Option(_) if is_native_wide_option_type(current_return_ty) => {}
        _ => {
            return Err(CodegenError::not_supported(
                "postfix `?` on `Option<T>` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â native lowering currently supports only functions returning native `Option<T>` shapes",
                expr.span,
            ))
        }
    }

    let option_val = lower_expr(
        builder,
        inner,
        current_return_ty,
        env,
        scope_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;
    let result_cl_ty = cl_type_for(option_payload_ty, expr.span)?;
    let some_block = builder.create_block();
    let none_block = builder.create_block();
    let merge_block = builder.create_block();
    let result = builder.append_block_param(merge_block, result_cl_ty);

    builder
        .ins()
        .brif(option_val, some_block, &[], none_block, &[]);

    builder.switch_to_block(none_block);
    builder.seal_block(none_block);
    let none_val = builder.ins().iconst(I64, 0);
    emit_function_return(builder, none_val, scope_stack, module, runtime);

    builder.switch_to_block(some_block);
    builder.seal_block(some_block);
    if wrapped_option {
        let payload = builder.ins().load(
            result_cl_ty,
            cranelift_codegen::ir::MemFlags::trusted(),
            option_val,
            OPTION_PAYLOAD_OFFSET,
        );
        if is_refcounted_type(option_payload_ty) {
            emit_retain(builder, module, runtime, payload);
        }
        emit_release(builder, module, runtime, option_val);
        builder.ins().jump(merge_block, &[payload.into()]);
    } else {
        builder.ins().jump(merge_block, &[option_val.into()]);
    }

    builder.switch_to_block(merge_block);
    builder.seal_block(merge_block);
    Ok(result)
}

fn lower_try_propagate_result(
    builder: &mut FunctionBuilder,
    expr: &IrExpr,
    inner: &IrExpr,
    current_return_ty: &Type,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    scope_stack: &Vec<Vec<(LocalId, Variable)>>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<ClValue, CodegenError> {
    let (ok_ty, err_ty) = match &inner.ty {
        Type::Result(ok, err) if is_native_result_type(&inner.ty) => (&**ok, &**err),
        Type::Result(_, _) => {
            return Err(CodegenError::not_supported(
                "postfix `?` on `Result<T, E>` ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â this concrete shape is outside the current native subset",
                expr.span,
            ))
        }
        _ => {
            return Err(CodegenError::cranelift(
                format!(
                    "postfix `?` saw non-Result inner type `{}` ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â typecheck should have caught this",
                    inner.ty.display_name()
                ),
                expr.span,
            ))
        }
    };

    if current_return_ty != &inner.ty {
        if let Type::Result(_, outer_err) = current_return_ty {
            if is_native_result_type(current_return_ty) && &**outer_err == err_ty {
                let result_ptr = lower_expr(
                    builder,
                    inner,
                    current_return_ty,
                    env,
                    scope_stack,
                    func_ids_by_def,
                    module,
                    runtime,
                )?;
                let tag = builder.ins().load(
                    I64,
                    cranelift_codegen::ir::MemFlags::trusted(),
                    result_ptr,
                    RESULT_TAG_OFFSET,
                );
                let ok_cl_ty = cl_type_for(ok_ty, expr.span)?;
                let ok_block = builder.create_block();
                let err_block = builder.create_block();
                let merge_block = builder.create_block();
                let unwrapped = builder.append_block_param(merge_block, ok_cl_ty);
                let is_ok = builder.ins().icmp_imm(IntCC::Equal, tag, RESULT_TAG_OK);
                builder.ins().brif(is_ok, ok_block, &[], err_block, &[]);

                builder.switch_to_block(err_block);
                builder.seal_block(err_block);
                let err_cl_ty = cl_type_for(err_ty, expr.span)?;
                let err_payload = builder.ins().load(
                    err_cl_ty,
                    cranelift_codegen::ir::MemFlags::trusted(),
                    result_ptr,
                    RESULT_PAYLOAD_OFFSET,
                );
                if is_refcounted_type(err_ty) {
                    emit_retain(builder, module, runtime, err_payload);
                }
                let outer_err_result = emit_result_wrapper_value(
                    builder,
                    module,
                    runtime,
                    current_return_ty,
                    err_payload,
                    err_ty,
                    RESULT_TAG_ERR,
                    expr.span,
                )?;
                emit_release(builder, module, runtime, result_ptr);
                emit_function_return(builder, outer_err_result, scope_stack, module, runtime);

                builder.switch_to_block(ok_block);
                builder.seal_block(ok_block);
                let payload = builder.ins().load(
                    ok_cl_ty,
                    cranelift_codegen::ir::MemFlags::trusted(),
                    result_ptr,
                    RESULT_PAYLOAD_OFFSET,
                );
                if is_refcounted_type(ok_ty) {
                    emit_retain(builder, module, runtime, payload);
                }
                emit_release(builder, module, runtime, result_ptr);
                builder.ins().jump(merge_block, &[payload.into()]);

                builder.switch_to_block(merge_block);
                builder.seal_block(merge_block);
                return Ok(unwrapped);
            }
        }
        return Err(CodegenError::not_supported(
            "postfix `?` on `Result<T, E>` ÃƒÆ’Ã†â€™Ãƒâ€ Ã¢â‚¬â„¢ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€¦Ã‚Â¡ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã¢â‚¬Å¡Ãƒâ€šÃ‚Â the current native subset supports propagation only when the enclosing function returns the same concrete `Result<T, E>` shape",
            expr.span,
        ));
    }

    let result_ptr = lower_expr(
        builder,
        inner,
        current_return_ty,
        env,
        scope_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;
    let tag = builder.ins().load(
        I64,
        cranelift_codegen::ir::MemFlags::trusted(),
        result_ptr,
        RESULT_TAG_OFFSET,
    );
    let ok_cl_ty = cl_type_for(ok_ty, expr.span)?;
    let ok_block = builder.create_block();
    let err_block = builder.create_block();
    let merge_block = builder.create_block();
    let unwrapped = builder.append_block_param(merge_block, ok_cl_ty);
    let is_ok = builder.ins().icmp_imm(IntCC::Equal, tag, RESULT_TAG_OK);
    builder.ins().brif(is_ok, ok_block, &[], err_block, &[]);

    builder.switch_to_block(err_block);
    builder.seal_block(err_block);
    emit_function_return(builder, result_ptr, scope_stack, module, runtime);

    builder.switch_to_block(ok_block);
    builder.seal_block(ok_block);
    let payload = builder.ins().load(
        ok_cl_ty,
        cranelift_codegen::ir::MemFlags::trusted(),
        result_ptr,
        RESULT_PAYLOAD_OFFSET,
    );
    if is_refcounted_type(ok_ty) {
        emit_retain(builder, module, runtime, payload);
    }
    emit_release(builder, module, runtime, result_ptr);
    builder.ins().jump(merge_block, &[payload.into()]);

    builder.switch_to_block(merge_block);
    builder.seal_block(merge_block);
    Ok(unwrapped)
}

fn emit_retry_delay_ms(
    builder: &mut FunctionBuilder,
    retry_index: ClValue,
    backoff: Backoff,
) -> ClValue {
    let cap = builder.ins().iconst(I64, i64::MAX);
    match backoff {
        Backoff::Linear(base_ms) => {
            if base_ms == 0 {
                return builder.ins().iconst(I64, 0);
            }
            let base = (base_ms.min(i64::MAX as u64)) as i64;
            let ordinal = builder.ins().iadd_imm(retry_index, 1);
            let max_multiplier = i64::MAX / base;
            let overflow =
                builder
                    .ins()
                    .icmp_imm(IntCC::SignedGreaterThan, ordinal, max_multiplier);
            let raw = builder.ins().imul_imm(ordinal, base);
            builder.ins().select(overflow, cap, raw)
        }
        Backoff::Exponential(base_ms) => {
            if base_ms == 0 {
                return builder.ins().iconst(I64, 0);
            }
            let base = (base_ms.min(i64::MAX as u64)) as i64;
            let max_shift = ((i64::MAX / base) as u64).ilog2() as i64;
            let overflow =
                builder
                    .ins()
                    .icmp_imm(IntCC::SignedGreaterThan, retry_index, max_shift);
            let base_val = builder.ins().iconst(I64, base);
            let raw = builder.ins().ishl(base_val, retry_index);
            builder.ins().select(overflow, cap, raw)
        }
    }
}

fn lower_try_retry_result(
    builder: &mut FunctionBuilder,
    expr: &IrExpr,
    body: &IrExpr,
    attempts: u64,
    backoff: Backoff,
    current_return_ty: &Type,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    scope_stack: &Vec<Vec<(LocalId, Variable)>>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<ClValue, CodegenError> {
    if !is_native_result_type(&body.ty) {
        return Err(CodegenError::not_supported(
            "`try ... on error retry ...` in native code currently supports only the native one-word `Result<T, E>` subset",
            expr.span,
        ));
    }
    if expr.ty != body.ty {
        return Err(CodegenError::cranelift(
            format!(
                "retry expression type `{}` did not match body type `{}` ÃƒÆ’Ã†â€™Ãƒâ€šÃ‚Â¢ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â€šÂ¬Ã…Â¡Ãƒâ€šÃ‚Â¬ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬Ãƒâ€šÃ‚Â typecheck should have caught this",
                expr.ty.display_name(),
                body.ty.display_name()
            ),
            expr.span,
        ));
    }

    let total_attempts = attempts.max(1).min(i64::MAX as u64) as i64;
    let total_val = builder.ins().iconst(I64, total_attempts);
    let zero = builder.ins().iconst(I64, 0);

    let attempt_block = builder.create_block();
    let done_block = builder.create_block();
    let result_cl_ty = cl_type_for(&expr.ty, expr.span)?;
    let done_result = builder.append_block_param(done_block, result_cl_ty);
    let attempt_retry_index = builder.append_block_param(attempt_block, I64);
    let attempt_remaining = builder.append_block_param(attempt_block, I64);

    builder.ins().jump(attempt_block, &[zero.into(), total_val.into()]);

    builder.switch_to_block(attempt_block);
    let retry_index_val = attempt_retry_index;
    let remaining_val = attempt_remaining;
    let result_ptr = lower_expr(
        builder,
        body,
        current_return_ty,
        env,
        scope_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;
    let tag = builder.ins().load(
        I64,
        cranelift_codegen::ir::MemFlags::trusted(),
        result_ptr,
        RESULT_TAG_OFFSET,
    );
    let is_ok = builder.ins().icmp_imm(IntCC::Equal, tag, RESULT_TAG_OK);
    let ok_block = builder.create_block();
    let err_block = builder.create_block();
    builder.ins().brif(is_ok, ok_block, &[], err_block, &[]);

    builder.switch_to_block(ok_block);
    builder.seal_block(ok_block);
    builder.ins().jump(done_block, &[result_ptr.into()]);

    builder.switch_to_block(err_block);
    builder.seal_block(err_block);
    let should_retry = builder.ins().icmp_imm(IntCC::SignedGreaterThan, remaining_val, 1);
    let retry_block = builder.create_block();
    let finish_err_block = builder.create_block();
    builder.append_block_param(retry_block, result_cl_ty);
    builder.append_block_param(retry_block, I64);
    builder.append_block_param(retry_block, I64);
    builder.append_block_param(finish_err_block, result_cl_ty);
    builder.ins().brif(
        should_retry,
        retry_block,
        &[result_ptr.into(), retry_index_val.into(), remaining_val.into()],
        finish_err_block,
        &[result_ptr.into()],
    );

    builder.switch_to_block(finish_err_block);
    builder.seal_block(finish_err_block);
    let final_result = builder.block_params(finish_err_block)[0];
    builder.ins().jump(done_block, &[final_result.into()]);

    builder.switch_to_block(retry_block);
    builder.seal_block(retry_block);
    let retry_result = builder.block_params(retry_block)[0];
    let retry_index = builder.block_params(retry_block)[1];
    let retry_remaining = builder.block_params(retry_block)[2];
    emit_release(builder, module, runtime, retry_result);

    let delay_val = emit_retry_delay_ms(builder, retry_index, backoff);
    let zero_delay = builder.ins().iconst(I64, 0);
    let has_delay = builder.ins().icmp(IntCC::SignedGreaterThan, delay_val, zero_delay);
    let sleep_block = builder.create_block();
    let no_sleep_block = builder.create_block();
    let continue_block = builder.create_block();
    builder.append_block_param(sleep_block, I64);
    builder
        .ins()
        .brif(has_delay, sleep_block, &[delay_val.into()], no_sleep_block, &[]);

    builder.switch_to_block(sleep_block);
    builder.seal_block(sleep_block);
    let sleep_delay = builder.block_params(sleep_block)[0];
    let sleep_ref = module.declare_func_in_func(runtime.sleep_ms, builder.func);
    builder.ins().call(sleep_ref, &[sleep_delay]);
    builder.ins().jump(continue_block, &[]);

    builder.switch_to_block(no_sleep_block);
    builder.seal_block(no_sleep_block);
    builder.ins().jump(continue_block, &[]);

    builder.switch_to_block(continue_block);
    builder.seal_block(continue_block);

    let next_retry_index = builder.ins().iadd_imm(retry_index, 1);
    let next_remaining = builder.ins().iadd_imm(retry_remaining, -1);
    builder
        .ins()
        .jump(attempt_block, &[next_retry_index.into(), next_remaining.into()]);
    builder.seal_block(attempt_block);

    builder.switch_to_block(done_block);
    builder.seal_block(done_block);
    Ok(done_result)
}

fn lower_try_retry_option(
    builder: &mut FunctionBuilder,
    expr: &IrExpr,
    body: &IrExpr,
    attempts: u64,
    backoff: Backoff,
    current_return_ty: &Type,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    scope_stack: &Vec<Vec<(LocalId, Variable)>>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<ClValue, CodegenError> {
    if !is_native_option_expr_type(&body.ty) {
        return Err(CodegenError::not_supported(
            "`try ... on error retry ...` in native code currently supports only native `Result<T, E>` and `Option<T>` bodies",
            expr.span,
        ));
    }
    if expr.ty != body.ty {
        return Err(CodegenError::cranelift(
            format!(
                "retry expression type `{}` did not match body type `{}` - typecheck should have caught this",
                expr.ty.display_name(),
                body.ty.display_name()
            ),
            expr.span,
        ));
    }

    let total_attempts = attempts.max(1).min(i64::MAX as u64) as i64;
    let total_val = builder.ins().iconst(I64, total_attempts);
    let zero = builder.ins().iconst(I64, 0);

    let attempt_block = builder.create_block();
    let done_block = builder.create_block();
    let result_cl_ty = cl_type_for(&expr.ty, expr.span)?;
    let done_result = builder.append_block_param(done_block, result_cl_ty);
    let attempt_retry_index = builder.append_block_param(attempt_block, I64);
    let attempt_remaining = builder.append_block_param(attempt_block, I64);

    builder.ins().jump(attempt_block, &[zero.into(), total_val.into()]);

    builder.switch_to_block(attempt_block);
    let retry_index_val = attempt_retry_index;
    let remaining_val = attempt_remaining;
    let option_val = lower_expr(
        builder,
        body,
        current_return_ty,
        env,
        scope_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;

    let is_some = builder.ins().icmp_imm(IntCC::NotEqual, option_val, 0);
    let some_block = builder.create_block();
    let none_block = builder.create_block();
    builder.ins().brif(is_some, some_block, &[], none_block, &[]);

    builder.switch_to_block(some_block);
    builder.seal_block(some_block);
    builder.ins().jump(done_block, &[option_val.into()]);

    builder.switch_to_block(none_block);
    builder.seal_block(none_block);
    let should_retry = builder.ins().icmp_imm(IntCC::SignedGreaterThan, remaining_val, 1);
    let retry_block = builder.create_block();
    let finish_none_block = builder.create_block();
    builder.append_block_param(retry_block, result_cl_ty);
    builder.append_block_param(retry_block, I64);
    builder.append_block_param(retry_block, I64);
    builder.append_block_param(finish_none_block, result_cl_ty);
    builder.ins().brif(
        should_retry,
        retry_block,
        &[option_val.into(), retry_index_val.into(), remaining_val.into()],
        finish_none_block,
        &[option_val.into()],
    );

    builder.switch_to_block(finish_none_block);
    builder.seal_block(finish_none_block);
    let final_result = builder.block_params(finish_none_block)[0];
    builder.ins().jump(done_block, &[final_result.into()]);

    builder.switch_to_block(retry_block);
    builder.seal_block(retry_block);
    let _retry_result = builder.block_params(retry_block)[0];
    let retry_index = builder.block_params(retry_block)[1];
    let retry_remaining = builder.block_params(retry_block)[2];

    let delay_val = emit_retry_delay_ms(builder, retry_index, backoff);
    let zero_delay = builder.ins().iconst(I64, 0);
    let has_delay = builder.ins().icmp(IntCC::SignedGreaterThan, delay_val, zero_delay);
    let sleep_block = builder.create_block();
    let no_sleep_block = builder.create_block();
    let continue_block = builder.create_block();
    builder.append_block_param(sleep_block, I64);
    builder
        .ins()
        .brif(has_delay, sleep_block, &[delay_val.into()], no_sleep_block, &[]);

    builder.switch_to_block(sleep_block);
    builder.seal_block(sleep_block);
    let sleep_delay = builder.block_params(sleep_block)[0];
    let sleep_ref = module.declare_func_in_func(runtime.sleep_ms, builder.func);
    builder.ins().call(sleep_ref, &[sleep_delay]);
    builder.ins().jump(continue_block, &[]);

    builder.switch_to_block(no_sleep_block);
    builder.seal_block(no_sleep_block);
    builder.ins().jump(continue_block, &[]);

    builder.switch_to_block(continue_block);
    builder.seal_block(continue_block);

    let next_retry_index = builder.ins().iadd_imm(retry_index, 1);
    let next_remaining = builder.ins().iadd_imm(retry_remaining, -1);
    builder
        .ins()
        .jump(attempt_block, &[next_retry_index.into(), next_remaining.into()]);
    builder.seal_block(attempt_block);

    builder.switch_to_block(done_block);
    builder.seal_block(done_block);
    Ok(done_result)
}

fn tool_wrapper_symbol(tool_name: &str) -> String {
    let mangled: String = tool_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("__corvid_tool_{mangled}")
}

// Silence warnings for fields we expect to use soon.
#[allow(dead_code)]
fn _force_use(_: MemFlags, _: Signature) {}

