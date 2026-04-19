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
mod agent;
use agent::*;

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

// Silence warnings for fields we expect to use soon.
#[allow(dead_code)]
fn _force_use(_: MemFlags, _: Signature) {}

