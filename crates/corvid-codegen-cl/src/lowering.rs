//! IR → Cranelift IR lowering.
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

#[inline(always)]
fn lowered_outcome_placeholder() -> BlockOutcome {
    BlockOutcome::Normal
}

/// Mangle a user agent's name into a link-safe symbol. Prevents
/// collisions with C runtime symbols (`main`, `printf`, `malloc`, ...).
///
/// Include the agent's `DefId` in the symbol because methods
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
mod runtime;
use runtime::*;

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
    //      when rc→0. Only for structs with refcounted fields.
    //   2. Struct trace fns (new in 17a): walk refcounted fields for
    //      the collector's mark walk. Emitted for every refcounted struct —
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
    // (every struct, empty body if no refcounted fields — linker
    // folds duplicates), typeinfos (every struct, uniform allocation
    // path). The pre-17a "primitive-only structs skip typeinfo"
    // short-circuit is gone — uniformity means 17d doesn't need a
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
        // but those don't need the async bridge — runtime is a C ABI
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
    // List are deliberately excluded — they need a serialization implementation.
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

        // If the program uses the async runtime, build
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
                    "entry agent {role} of type `{}` — the native command-line boundary currently supports only `Int` / `Bool` / `Float` / `String`; structured types (including Result, Option, and Weak) need a dedicated serialization layer (use a wrapper agent that converts internally)",
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
                    "parameter `{}: {}` — this native lowering path supports `Int`, `Bool`, and `Float` here; `String`, `Struct`, and `List` use later lowering paths",
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
                "agent `{}` returns `{}` — this native lowering path supports `Int`, `Bool`, and `Float` returns here",
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
        // Struct values are descriptor pointers (like String) — single
        // I64 in registers/env. The actual layout lives behind the
        // refcount header: `[header (16) | field0 (8) | ... | fieldN (8)]`.
        Type::Struct(_) => Ok(I64),
        // List values are descriptor pointers (like String, Struct) —
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
            "`Nothing` — use a bare `return` instead",
            span,
        )),
        Type::Function { .. } => Err(CodegenError::not_supported(
            "function types as values — first-class callables are not implemented in native codegen yet",
            span,
        )),
        // Result<T,E> and Option<T> are accepted by the typechecker and
        // handled fully by the interpreter. Native tagged-union layout
        // and retry lowering are not implemented yet, so native
        // compilation reports a clean boundary here.
        Type::Result(_, _) => Err(CodegenError::not_supported(
            "`Result<T, E>` — native tagged-union lowering is not implemented yet; use the interpreter tier (`corvid run --tier interp`) until then",
            span,
        )),
        Type::Option(_) => Err(CodegenError::not_supported(
            "`Option<T>` — native codegen currently supports nullable-pointer `Option<T>` when `T` is refcounted plus wide scalar `Option<Int|Bool|Float>`; other payload shapes still need the interpreter tier",
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
            //      the callee does NOT retain nor release — ownership
            //      doesn't cross the ABI. Saves one retain + one
            //      release per call site for read-only parameters.
            //
            // `borrow_sig = None` means the ownership pass didn't
            // run on this agent — fall back to Owned (pre-17b
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

#[derive(Clone, Copy)]
enum BlockOutcome {
    Normal,
    Terminated,
}

fn emit_function_return(
    builder: &mut FunctionBuilder,
    value: ClValue,
    scope_stack: &[Vec<(LocalId, Variable)>],
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) {
    if !runtime.dup_drop_enabled {
        for scope in scope_stack.iter().rev() {
            for (_, var) in scope.iter().rev() {
                let v_local = builder.use_var(*var);
                emit_release(builder, module, runtime, v_local);
            }
        }
    }
    builder.ins().return_(&[value]);
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
                "postfix `?` on `Option<T>` — native lowering currently supports nullable-pointer `Option<T>` with refcounted payloads plus wide scalar `Option<Int|Bool|Float>`",
                expr.span,
            ))
        }
        Type::Result(_, _) => {
            return Err(CodegenError::not_supported(
                "postfix `?` on `Result<T, E>` — native tagged-union lowering is not implemented yet; use the interpreter tier until then",
                expr.span,
            ))
        }
        _ => {
            return Err(CodegenError::cranelift(
                format!(
                    "postfix `?` saw non-Option inner type `{}` — typecheck should have caught this",
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
                "postfix `?` on `Option<T>` — native lowering currently supports only functions returning native `Option<T>` shapes",
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
                "postfix `?` on `Result<T, E>` â€” this concrete shape is outside the current native subset",
                expr.span,
            ))
        }
        _ => {
            return Err(CodegenError::cranelift(
                format!(
                    "postfix `?` saw non-Result inner type `{}` â€” typecheck should have caught this",
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
            "postfix `?` on `Result<T, E>` â€” the current native subset supports propagation only when the enclosing function returns the same concrete `Result<T, E>` shape",
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
                "retry expression type `{}` did not match body type `{}` — typecheck should have caught this",
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

fn lower_block(
    builder: &mut FunctionBuilder,
    block: &IrBlock,
    current_return_ty: &Type,
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
            current_return_ty,
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
    current_return_ty: &Type,
    env: &mut HashMap<LocalId, (Variable, clir::Type)>,
    var_idx: &mut usize,
    scope_stack: &mut Vec<Vec<(LocalId, Variable)>>,
    loop_stack: &mut Vec<LoopCtx>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<BlockOutcome, CodegenError> {
    match stmt {
        IrStmt::Yield { span, .. } => {
            Err(CodegenError::not_supported(
                "Stream lowering not yet implemented",
                *span,
            ))
        }
        IrStmt::Return { value, span } => {
            let v = match value {
                Some(e) => lower_expr(
                    builder,
                    e,
                    current_return_ty,
                    env,
                    scope_stack,
                    func_ids_by_def,
                    module,
                    runtime,
                )?,
                None => {
                    return Err(CodegenError::not_supported(
                        "bare `return` (Nothing type not supported yet)",
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
            emit_function_return(builder, v, scope_stack, module, runtime);
            Ok(BlockOutcome::Terminated)
        }
        IrStmt::Expr { expr, .. } => {
            let v = lower_expr(
                builder,
                expr,
                current_return_ty,
                env,
                scope_stack,
                func_ids_by_def,
                module,
                runtime,
            )?;
            // Discarded statement-expression:
            //   - If the expression is a BARE Local read, the value
            //     belongs to the Local binding; the .6d pass handles
            //     its lifetime via block-exit drops or last-use kills.
            //     Under flag-off the pre-.6d path retained at read
            //     time, so we release to match.
            //   - If the expression is anything else (call, composite,
            //     literal), it produced a fresh Owned temp with no
            //     owner — always release, regardless of flag, since
            //     the pass has no Local to drop.
            if is_refcounted_type(&expr.ty) {
                let is_bare_local = matches!(&expr.kind, IrExprKind::Local { .. });
                if !is_bare_local || !runtime.dup_drop_enabled {
                    emit_release(builder, module, runtime, v);
                }
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
            // Reassignment: the old value of the Local is being
            // replaced; its +1 must drop. The .6d pass doesn't yet
            // model reassignment-kill (the analysis treats a rebind
            // as a def but doesn't schedule a Drop for the previous
            // value — this is a forward-compatibility gap tracked for
            // a future analysis extension). Always release the old
            // value at codegen time so the unified pass stays
            // refcount-correct on reassigned refcounted locals.
            if refcounted && is_reassignment {
                let old = builder.use_var(var);
                emit_release(builder, module, runtime, old);
            }
            let v = lower_expr(
                builder,
                value,
                current_return_ty,
                env,
                scope_stack,
                func_ids_by_def,
                module,
                runtime,
            )?;
            // The Value flowing into a refcounted
            // binding must be stack-map-declared so Cranelift spills
            // it across safepoints. Cranelift's safepoint pass
            // tracks liveness through SSA phis for Values that
            // travel between blocks via the Variable facade, but
            // only for Values originally declared here.
            if refcounted {
                builder.declare_value_needs_stack_map(v);
            }
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
            current_return_ty,
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
            current_return_ty,
            env,
            var_idx,
            scope_stack,
            loop_stack,
            func_ids_by_def,
            module,
            runtime,
        ),
        IrStmt::Approve { label, args, span } => {
            // Keep approve-arg side effects intact, then route the
            // approval label through the runtime so native runs emit
            // real approval trace events too.
            for a in args {
                let v = lower_expr(
                    builder,
                    a,
                    current_return_ty,
                    env,
                    scope_stack,
                    func_ids_by_def,
                    module,
                    runtime,
                )?;
                if !runtime.dup_drop_enabled && is_refcounted_type(&a.ty) {
                    emit_release(builder, module, runtime, v);
                }
            }

            let label_val = emit_string_const(builder, module, runtime, label, *span)?;
            let approve_fref = module.declare_func_in_func(runtime.approve_sync, builder.func);
            let call = builder.ins().call(approve_fref, &[label_val]);
            let results: Vec<ClValue> = builder.inst_results(call).iter().copied().collect();
            emit_release(builder, module, runtime, label_val);

            let denied = builder.ins().icmp_imm(IntCC::Equal, results[0], 0);
            let deny_block = builder.create_block();
            let continue_block = builder.create_block();
            builder
                .ins()
                .brif(denied, deny_block, &[], continue_block, &[]);

            builder.switch_to_block(deny_block);
            builder.seal_block(deny_block);
            builder
                .ins()
                .trap(cranelift_codegen::ir::TrapCode::unwrap_user(1));

            builder.switch_to_block(continue_block);
            builder.seal_block(continue_block);
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
        // Dup/Drop as first-class IR operations. In the fallback
        // path the ownership analysis pass hasn't been
        // written yet, so these variants never appear in the IR
        // codegen receives (only `None` borrow_sigs and no
        // Dup/Drop statements — all ownership is still handled by
        // the scattered `emit_retain`/`emit_release` calls in the
        // expression lowerings). 17b-1b replaces that with the
        // analysis pass that actually emits these. For forward
        // compatibility the handlers are present and correct today:
        // each lowers to a single runtime call on the variable's
        // current value, with a type-based no-op for non-refcounted
        // locals.
        IrStmt::Dup { local_id, span } => {
            let (var, cl_ty) = *env.get(local_id).ok_or_else(|| {
                CodegenError::cranelift(
                    format!("Dup references unknown local {:?}", local_id),
                    *span,
                )
            })?;
            // Non-refcounted locals use I64/F64/I8 for primitives;
            // only I64 values that point at refcounted payloads need
            // retain. The analysis pass is responsible for only
            // emitting Dup on refcounted locals — if a non-I64
            // slipped through, that's a bug in the analysis, not
            // a silent no-op here.
            if cl_ty != I64 {
                return Err(CodegenError::cranelift(
                    format!(
                        "Dup on non-I64 local (cl_ty={cl_ty:?}) — analysis \
                         should have filtered this out"
                    ),
                    *span,
                ));
            }
            let v = builder.use_var(var);
            emit_retain(builder, module, runtime, v);
            Ok(BlockOutcome::Normal)
        }
        IrStmt::Drop { local_id, span } => {
            let (var, cl_ty) = *env.get(local_id).ok_or_else(|| {
                CodegenError::cranelift(
                    format!("Drop references unknown local {:?}", local_id),
                    *span,
                )
            })?;
            if cl_ty != I64 {
                return Err(CodegenError::cranelift(
                    format!(
                        "Drop on non-I64 local (cl_ty={cl_ty:?}) — analysis \
                         should have filtered this out"
                    ),
                    *span,
                ));
            }
            let v = builder.use_var(var);
            emit_release(builder, module, runtime, v);
            Ok(BlockOutcome::Normal)
        }
    }
}

fn lower_expr(
    builder: &mut FunctionBuilder,
    expr: &IrExpr,
    current_return_ty: &Type,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    scope_stack: &Vec<Vec<(LocalId, Variable)>>,
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
            "`nothing` literal is not supported yet",
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
            //
            // Under the .6d unified pass, we rely on the pass's Dup
            // insertion at non-last consuming uses instead of
            // retaining on every read — the last use of a local
            // consumes the existing +1 directly.
            if !runtime.dup_drop_enabled && is_refcounted_type(&expr.ty) {
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
                    current_return_ty,
                    env,
                    scope_stack,
                    func_ids_by_def,
                    module,
                    runtime,
                );
            }
            // String operands route through the String runtime helpers
            // and have their own ownership semantics (release inputs
            // after the helper call). Dispatch here so we have access
            // to the IR type information.
            //
            // Borrow-at-use-site peephole for
            // consuming string BinOps. If an operand is a bare
            // `IrExprKind::Local` of a refcounted String, we lower
            // the Local WITHOUT emitting the ownership-conversion
            // retain, and correspondingly skip the post-op release.
            // The comparison/concat runtime helpers only read their
            // inputs (they don't mutate the operand's refcount or
            // store the pointer), so reading the Variable directly
            // is semantically equivalent to retain-then-release with
            // zero observable refcount net effect. The Local's
            // binding remains Live in its scope, governed by the
            // scope-exit release that's already in place.
            //
            // Non-bare-Local operands (literals, nested expressions,
            // call results) still produce an Owned +1 and are
            // released by the helper as before — the original
            // ownership contract.
            if matches!(&left.ty, Type::String) && matches!(&right.ty, Type::String) {
                let (l, l_borrowed) =
                    lower_string_operand_maybe_borrowed(builder, left, current_return_ty, env, scope_stack, func_ids_by_def, module, runtime)?;
                let (r, r_borrowed) =
                    lower_string_operand_maybe_borrowed(builder, right, current_return_ty, env, scope_stack, func_ids_by_def, module, runtime)?;
                return lower_string_binop_with_ownership(
                    builder, *op, l, r, l_borrowed, r_borrowed, expr.span, module, runtime,
                );
            }
            let l = lower_expr(builder, left, current_return_ty, env, scope_stack, func_ids_by_def, module, runtime)?;
            let r = lower_expr(builder, right, current_return_ty, env, scope_stack, func_ids_by_def, module, runtime)?;
            let result = lower_binop_strict(builder, *op, l, r, expr.span, module, runtime)?;
            if is_refcounted_type(&left.ty) {
                emit_release(builder, module, runtime, l);
            }
            if is_refcounted_type(&right.ty) {
                emit_release(builder, module, runtime, r);
            }
            Ok(result)
        }
        IrExprKind::UnOp { op, operand } => {
            let v = lower_expr(builder, operand, current_return_ty, env, scope_stack, func_ids_by_def, module, runtime)?;
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
                // Caller-side borrow peephole. For
                // each refcounted arg whose callee slot is Borrowed
                // (per the callee's `borrow_sig` populated by the
                // ownership pass) AND whose argument expression is a
                // bare `IrExprKind::Local`, we lower the Local
                // directly from its Variable without a retain, and
                // skip the paired post-call release. The callee
                // already skips its entry-retain + scope-exit release
                // for Borrowed params (17b-1b.1), so the whole
                // retain/release pair collapses on both sides.
                //
                // For Owned callee slots OR non-bare-Local args, the
                // original +0 ABI applies: lower_expr produces a +1
                // Owned, callee retains on entry (17b-1b.1 keeps this
                // for Owned), caller releases its +1 after the call.
                let callee_sig = runtime.agent_borrow_sigs.get(def_id);
                let mut arg_vals = Vec::with_capacity(args.len());
                // `needs_post_release[i]` = true iff the caller
                // produced a +1 that must be released post-call
                // (false when borrow peephole applies).
                let mut needs_post_release: Vec<bool> = Vec::with_capacity(args.len());
                for (i, a) in args.iter().enumerate() {
                    let is_ref = is_refcounted_type(&a.ty);
                    let callee_borrowed = callee_sig
                        .and_then(|s| s.get(i).copied())
                        .map(|b| matches!(b, corvid_ir::ParamBorrow::Borrowed))
                        .unwrap_or(false);
                    let arg_is_bare_local =
                        matches!(&a.kind, IrExprKind::Local { .. });
                    if is_ref && callee_borrowed && arg_is_bare_local {
                        // Borrow path: read the Variable directly, no retain.
                        if let IrExprKind::Local { local_id, name } = &a.kind {
                            let (var, _ty) = env.get(local_id).ok_or_else(|| {
                                CodegenError::cranelift(
                                    format!("no variable for local `{name}` — compiler bug"),
                                    a.span,
                                )
                            })?;
                            let v = builder.use_var(*var);
                            arg_vals.push(v);
                            needs_post_release.push(false);
                            continue;
                        }
                    }
                    // Normal path: +1 Owned, caller releases after call.
                    let v = lower_expr(
                        builder,
                        a,
                        current_return_ty,
                        env,
                        scope_stack,
                        func_ids_by_def,
                        module,
                        runtime,
                    )?;
                    arg_vals.push(v);
                    needs_post_release.push(is_ref);
                }
                let call = builder.ins().call(callee_ref, &arg_vals);
                let results = builder.inst_results(call);
                if results.len() != 1 {
                    return Err(CodegenError::cranelift(
                        format!(
                            "agent `{callee_name}` returned {} values; native lowering expects exactly 1",
                            results.len()
                        ),
                        expr.span,
                    ));
                }
                let result = results[0];
                if !runtime.dup_drop_enabled {
                    for (v, needs) in arg_vals.iter().zip(needs_post_release.iter()) {
                        if *needs {
                            emit_release(builder, module, runtime, *v);
                        }
                    }
                }
                Ok(result)
            }
            IrCallKind::Tool { def_id, .. } => {
                // Emit a direct typed call to the tool's
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

                // Tool-call ABI: refcount lifecycle matches
                // the agent-call convention — caller
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
                        current_return_ty,
                        env,
                        scope_stack,
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
                if !runtime.dup_drop_enabled {
                    for (v, is_ref) in arg_vals.iter().zip(arg_refcounted.iter()) {
                        if *is_ref {
                            emit_release(builder, module, runtime, *v);
                        }
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
                    current_return_ty,
                    env,
                    scope_stack,
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
                    current_return_ty,
                    env,
                    scope_stack,
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

            // Borrow-at-use-site peephole: if the target is a bare
            // `IrExprKind::Local`, read the Variable directly with
            // no ownership-conversion retain, and skip the symmetric
            // post-extract release of the struct pointer. The load
            // of the field only reads the struct's memory; we never
            // mutate the struct's refcount. The Local's binding
            // stays Live — its scope-exit release handles cleanup.
            let (struct_ptr, struct_borrowed) = lower_container_maybe_borrowed(
                builder, target, current_return_ty, env, scope_stack, func_ids_by_def, module, runtime,
            )?;
            let field_val = builder.ins().load(
                field_cl_ty,
                cranelift_codegen::ir::MemFlags::trusted(),
                struct_ptr,
                offset,
            );
            // Retain refcounted field so caller gets an Owned ref.
            // This retain is NOT redundant pass traffic — it's the
            // ownership conversion from "field slot inside a parent"
            // to "standalone result value the caller owns." The .6d
            // pass cannot replace this because the extracted value
            // never gets an IR Local; it's an intermediate temp
            // known only to codegen. Pairs with the struct_ptr
            // release below when struct_ptr was a fresh temp.
            if is_refcounted_type(&field_meta.ty) {
                emit_retain(builder, module, runtime, field_val);
            }
            // Release the temp +1 on the struct pointer only if we
            // created one. `struct_borrowed = false` means the target
            // was a fresh temp (call result, constructor, nested field
            // access) not a bare Local — the .6d pass doesn't see
            // those, so codegen must drop unconditionally. Borrowed
            // reads have no +1 to release.
            if !struct_borrowed {
                emit_release(builder, module, runtime, struct_ptr);
            }
            Ok(field_val)
        }
        IrExprKind::Index { target, index } => {
            // Element type from the Index expression's annotated type
            // (the type checker attaches the element type).
            let elem_ty = expr.ty.clone();
            let elem_cl_ty = cl_type_for(&elem_ty, expr.span)?;
            let elem_refcounted = is_refcounted_type(&elem_ty);

            // Same borrow-at-use-site trick
            // as FieldAccess — if the list target is a bare Local,
            // borrow it (no retain) and skip the post-extract release.
            // The bounds-check + load only read the list's memory;
            // never mutate its refcount or escape the pointer.
            let (list_ptr, list_borrowed) = lower_container_maybe_borrowed(
                builder, target, current_return_ty, env, scope_stack, func_ids_by_def, module, runtime,
            )?;
            let idx_val = lower_expr(
                builder,
                index,
                current_return_ty,
                env,
                scope_stack,
                func_ids_by_def,
                module,
                runtime,
            )?;

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
            // Retain extracted element as ownership conversion from
            // list slot → standalone caller-owned value. The .6d
            // pass can't replace this because the extracted value
            // never gets an IR Local. Pairs with list_ptr release.
            if elem_refcounted {
                emit_retain(builder, module, runtime, elem_val);
            }
            // Release the temp +1 on the list pointer only if we
            // actually took one. `list_borrowed = false` means the
            // target was a fresh temp, not a bare Local — the .6d
            // pass doesn't see internal temps, so codegen drops
            // unconditionally. Borrowed reads have no +1 to drop.
            if !list_borrowed {
                emit_release(builder, module, runtime, list_ptr);
            }
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
            // Single typed allocator. The typeinfo block
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
                let item_val = lower_expr(
                    builder,
                    item,
                    current_return_ty,
                    env,
                    scope_stack,
                    func_ids_by_def,
                    module,
                    runtime,
                )?;
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
        IrExprKind::WeakNew { strong } => {
            let strong_val = lower_expr(
                builder,
                strong,
                current_return_ty,
                env,
                scope_stack,
                func_ids_by_def,
                module,
                runtime,
            )?;
            let weak_new_ref = module.declare_func_in_func(runtime.weak_new, builder.func);
            let call = builder.ins().call(weak_new_ref, &[strong_val]);
            let weak_ptr = builder.inst_results(call)[0];
            if !runtime.dup_drop_enabled && is_refcounted_type(&strong.ty) {
                emit_release(builder, module, runtime, strong_val);
            }
            Ok(weak_ptr)
        }
        IrExprKind::WeakUpgrade { weak } => {
            let weak_val = lower_expr(
                builder,
                weak,
                current_return_ty,
                env,
                scope_stack,
                func_ids_by_def,
                module,
                runtime,
            )?;
            let weak_upgrade_ref =
                module.declare_func_in_func(runtime.weak_upgrade, builder.func);
            let call = builder.ins().call(weak_upgrade_ref, &[weak_val]);
            let upgraded = builder.inst_results(call)[0];
            if !runtime.dup_drop_enabled && is_refcounted_type(&weak.ty) {
                emit_release(builder, module, runtime, weak_val);
            }
            Ok(upgraded)
        }
        IrExprKind::ResultOk { inner } if is_native_result_type(&expr.ty) => lower_result_constructor(
            builder,
            expr,
            inner,
            RESULT_TAG_OK,
            current_return_ty,
            env,
            scope_stack,
            func_ids_by_def,
            module,
            runtime,
        ),
        IrExprKind::ResultErr { inner } if is_native_result_type(&expr.ty) => lower_result_constructor(
            builder,
            expr,
            inner,
            RESULT_TAG_ERR,
            current_return_ty,
            env,
            scope_stack,
            func_ids_by_def,
            module,
            runtime,
        ),
        // Result/Option construction and `?` / `try-retry` control
        // flow are native only for the current principled subset:
        //   * nullable-pointer `Option<T>` when `T` is refcounted
        //   * wide scalar `Option<Int|Bool|Float>`
        //   * one-word `Result<T, E>`
        //   * `try ... retry` over the native `Result<T, E>` subset
        //
        // Any shape outside that subset still gets a clean,
        // actionable boundary here. The `cl_type_for` path above
        // will typically fire first for bindings, but these arms are
        // the load-bearing fallback for intermediate expressions and
        // other positions the typecheck doesn't intercept.
        IrExprKind::ResultOk { .. } | IrExprKind::ResultErr { .. } => {
            Err(CodegenError::not_supported(
                "`Result<T, E>` construction (`Ok(...)` / `Err(...)`) — native tagged-union lowering is not implemented yet; use the interpreter tier (`corvid run --tier interp`) until then",
                expr.span,
            ))
        }
        IrExprKind::OptionSome { inner } => {
            if option_uses_wrapper(&expr.ty) {
                let payload_ty = match &expr.ty {
                    Type::Option(inner_ty) => &**inner_ty,
                    _ => unreachable!("option wrapper gate requires Option<T> type"),
                };
                let payload = lower_expr(
                    builder,
                    inner,
                    current_return_ty,
                    env,
                    scope_stack,
                    func_ids_by_def,
                    module,
                    runtime,
                )?;
                emit_option_wrapper_value(builder, module, runtime, &expr.ty, payload, payload_ty, expr.span)
            } else if is_refcounted_type(&expr.ty) {
                lower_expr(
                    builder,
                    inner,
                    current_return_ty,
                    env,
                    scope_stack,
                    func_ids_by_def,
                    module,
                    runtime,
                )
            } else {
                Err(CodegenError::not_supported(
                    "`Some(...)` — native codegen currently supports nullable-pointer `Option<T>` when `T` is refcounted plus wide scalar `Option<Int|Bool|Float>`",
                    expr.span,
                ))
            }
        }
        IrExprKind::OptionNone => {
            Ok(builder.ins().iconst(I64, 0))
        }
        IrExprKind::TryPropagate { inner } => match &inner.ty {
            Type::Result(_, _) => lower_try_propagate_result(
                builder,
                expr,
                inner,
                current_return_ty,
                env,
                scope_stack,
                func_ids_by_def,
                module,
                runtime,
            ),
            _ => lower_try_propagate_option(
                builder,
                expr,
                inner,
                current_return_ty,
                env,
                scope_stack,
                func_ids_by_def,
                module,
                runtime,
            ),
        },
        IrExprKind::TryRetry {
            body,
            attempts,
            backoff,
        } => match &body.ty {
            Type::Result(_, _) => lower_try_retry_result(
                builder,
                expr,
                body,
                *attempts,
                *backoff,
                current_return_ty,
                env,
                scope_stack,
                func_ids_by_def,
                module,
                runtime,
            ),
            Type::Option(_) => lower_try_retry_option(
                builder,
                expr,
                body,
                *attempts,
                *backoff,
                current_return_ty,
                env,
                scope_stack,
                func_ids_by_def,
                module,
                runtime,
            ),
            _ => Err(CodegenError::not_supported(
                "`try ... on error retry ...` in native code currently supports only native `Result<T, E>` and `Option<T>` bodies",
                expr.span,
            )),
        },
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

/// Lower a container expression (struct target of a
/// `FieldAccess`, list target of an `Index`) in a borrow position.
/// Returns `(value, borrowed)` following the same convention as
/// `lower_string_operand_maybe_borrowed`:
///
///   * bare `IrExprKind::Local` → `(value, true)`, no retain — caller
///     must NOT release the value afterward.
///   * any other shape → normal `lower_expr` (+1 Owned), `false` —
///     caller releases as before.
///
/// Safe because the FieldAccess / Index code paths only READ the
/// container (load + optionally bounds-check) — they never mutate
/// the container's refcount or escape the pointer elsewhere. The
/// caller's Local binding keeps the container alive through its
/// scope-exit release.
fn lower_container_maybe_borrowed(
    builder: &mut FunctionBuilder,
    expr: &IrExpr,
    current_return_ty: &Type,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    scope_stack: &Vec<Vec<(LocalId, Variable)>>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<(ClValue, bool), CodegenError> {
    if let IrExprKind::Local { local_id, name } = &expr.kind {
        let (var, _ty) = env.get(local_id).ok_or_else(|| {
            CodegenError::cranelift(
                format!("no variable for local `{name}` — compiler bug"),
                expr.span,
            )
        })?;
        let v = builder.use_var(*var);
        return Ok((v, true));
    }
    let v = lower_expr(
        builder,
        expr,
        current_return_ty,
        env,
        scope_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;
    Ok((v, false))
}

/// Lower a string-typed operand in a borrow position
/// (e.g. an operand of a consuming String BinOp: concat, equality, or
/// ordering compare). Returns `(value, borrowed)`:
///
///   * `borrowed = true` iff the operand was a bare `IrExprKind::Local`
///     — in that case we skip the ownership-conversion retain that
///     `lower_expr` would normally emit, and the caller must NOT
///     release the value afterward. The returned `ClValue` is a
///     borrow of the binding's current refcount (caller's scope
///     still governs the Drop).
///   * `borrowed = false` for every other expression shape — the
///     value is a fresh Owned +1 produced by `lower_expr`, and the
///     caller is responsible for the corresponding release.
///
/// Safe because the String runtime helpers (`corvid_string_concat`,
/// `corvid_string_eq`, `corvid_string_cmp`) only read their inputs —
/// they never mutate the operand refcount nor store the pointer. So
/// passing a borrow vs. an Owned +1 is indistinguishable from the
/// helper's perspective; the only observable difference is the net
/// zero retain/release pair we eliminated.
fn lower_string_operand_maybe_borrowed(
    builder: &mut FunctionBuilder,
    expr: &IrExpr,
    current_return_ty: &Type,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    scope_stack: &Vec<Vec<(LocalId, Variable)>>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<(ClValue, bool), CodegenError> {
    // Under the .6d unified pass: take the non-peephole path for bare
    // Local operands. If we returned `borrowed=true`, BinOp would skip
    // the release and the caller's +1 would leak — the pass's
    // analysis treats BinOp operands as Owned-consumed and does NOT
    // schedule a Drop for an operand that's consumed (there's no
    // later use), so codegen MUST release to retire the +1.
    if !runtime.dup_drop_enabled {
        if let IrExprKind::Local { local_id, name } = &expr.kind {
            let (var, _ty) = env.get(local_id).ok_or_else(|| {
                CodegenError::cranelift(
                    format!("no variable for local `{name}` — compiler bug"),
                    expr.span,
                )
            })?;
            let v = builder.use_var(*var);
            return Ok((v, true));
        }
    }
    let v = lower_expr(
        builder,
        expr,
        current_return_ty,
        env,
        scope_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;
    Ok((v, false))
}

/// Lower a string BinOp whose operands may be borrowed
/// rather than Owned. Mirrors `lower_string_binop` but conditionally
/// skips the post-op release for any operand that was passed as a
/// borrow (no +1 was produced; nothing to release).
#[allow(clippy::too_many_arguments)]
fn lower_string_binop_with_ownership(
    builder: &mut FunctionBuilder,
    op: BinaryOp,
    l: ClValue,
    r: ClValue,
    l_borrowed: bool,
    r_borrowed: bool,
    span: Span,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<ClValue, CodegenError> {
    match op {
        BinaryOp::Add => {
            let callee = module.declare_func_in_func(runtime.string_concat, builder.func);
            let call = builder.ins().call(callee, &[l, r]);
            let result = builder.inst_results(call)[0];
            // BinOp consumes its refcounted operands by convention:
            // the pass's Dup-before-non-last-use supplies the extra
            // +1 for multi-read Locals; the single +1 flowing in as
            // each operand is retired here. Under both flag modes.
            if !l_borrowed {
                emit_release(builder, module, runtime, l);
            }
            if !r_borrowed {
                emit_release(builder, module, runtime, r);
            }
            Ok(result)
        }
        BinaryOp::Eq | BinaryOp::NotEq => {
            let callee = module.declare_func_in_func(runtime.string_eq, builder.func);
            let call = builder.ins().call(callee, &[l, r]);
            let eq_i64 = builder.inst_results(call)[0];
            let eq_i8 = builder.ins().ireduce(I8, eq_i64);
            let result = if matches!(op, BinaryOp::Eq) {
                eq_i8
            } else {
                let zero = builder.ins().iconst(I8, 0);
                builder.ins().icmp(IntCC::Equal, eq_i8, zero)
            };
            // BinOp consumes its refcounted operands by convention:
            // the pass's Dup-before-non-last-use supplies the extra
            // +1 for multi-read Locals; the single +1 flowing in as
            // each operand is retired here. Under both flag modes.
            if !l_borrowed {
                emit_release(builder, module, runtime, l);
            }
            if !r_borrowed {
                emit_release(builder, module, runtime, r);
            }
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
            // BinOp consumes its refcounted operands by convention:
            // the pass's Dup-before-non-last-use supplies the extra
            // +1 for multi-read Locals; the single +1 flowing in as
            // each operand is retired here. Under both flag modes.
            if !l_borrowed {
                emit_release(builder, module, runtime, l);
            }
            if !r_borrowed {
                emit_release(builder, module, runtime, r);
            }
            Ok(result)
        }
        BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => Err(
            CodegenError::not_supported(
                format!("`{op:?}` is not defined for `String` operands"),
                span,
            ),
        ),
        BinaryOp::And | BinaryOp::Or => {
            unreachable!("and/or is short-circuited upstream and never reaches string BinOp")
        }
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
    current_return_ty: &Type,
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
                "`for c in string` iteration in native code — not implemented yet (needs iterator \
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

    // Iter-as-bare-Local borrow peephole. Same
    // correctness argument as 17b-1b.3's FieldAccess/Index — the
    // loop's use of `list_ptr` (length load + per-element load +
    // bounds arithmetic) only READS the list's memory; never
    // mutates the list's refcount or escapes the pointer. If iter
    // is a bare Local of the enclosing scope, we can borrow it:
    // skip the ownership-conversion retain at lower_expr time AND
    // skip the paired release at loop exit. The Local's binding
    // stays Live in its enclosing scope, governed by the usual
    // scope-exit release.
    let (list_ptr, list_borrowed) = lower_container_maybe_borrowed(
        builder, iter, current_return_ty, env, scope_stack, func_ids_by_def, module, runtime,
    )?;

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
    // The loaded element is a refcounted pointer that
    // flows into the loop variable. Declare it so Cranelift spills
    // before any safepoint in the loop body.
    if elem_refcounted {
        builder.declare_value_needs_stack_map(elem_val);
    }
    // Rebind loop-var: release old value (null-safe), retain new if
    // refcounted, def_var. Under the .6d pass, the pass inserts a
    // Drop on the loop variable at each iteration's natural last-use
    // point and does not emit a fresh Dup for the new iteration
    // element (the load above produces it with no +1 attached; the
    // consumer patterns inside the body supply Dups as needed).
    if !runtime.dup_drop_enabled && elem_refcounted {
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
        current_return_ty,
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
            if !runtime.dup_drop_enabled {
                for (_, v) in body_scope.iter().rev() {
                    let x = builder.use_var(*v);
                    emit_release(builder, module, runtime, x);
                }
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

    // Release the list pointer we retained at the top — only if we
    // actually produced a +1 (non-borrowed path). When iter was a
    // bare Local we borrowed it and there's nothing to release.
    // `list_borrowed = false` means the iter was a fresh-owned temp
    // (list literal or call result), not a bare Local. The .6d pass
    // only schedules Drops for Locals in the IR; internal expression
    // temps are invisible to it, so the codegen must still drop the
    // list's +1 at loop end. `list_borrowed = true` means it was a
    // bare Local and the pass handles its lifetime.
    if is_refcounted_type(&iter.ty) && !list_borrowed {
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
    if !runtime.dup_drop_enabled {
        for depth in (ctx.scope_depth_at_entry..scope_stack.len()).rev() {
            let scope = &scope_stack[depth];
            for (_, v) in scope.iter().rev() {
                let x = builder.use_var(*v);
                emit_release(builder, module, runtime, x);
            }
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
    current_return_ty: &Type,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    scope_stack: &Vec<Vec<(LocalId, Variable)>>,
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
    // Structs with refcounted fields use the typeinfo-
    // driven allocator. Structs with no refcounted fields currently
    // skip typeinfo entirely (lower_file bypasses emission for them)
    // — they allocate with NULL destroy_fn via a runtime-owned
    // empty typeinfo? For 17a we keep the pre-typed behavior for
    // these: only emit typed allocation when the struct actually
    // has refcounted fields requiring dispatch. Non-refcounted
    // structs remain on the old path *only temporarily* — future
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
        let value = lower_expr(
            builder,
            arg,
            current_return_ty,
            env,
            scope_stack,
            func_ids_by_def,
            module,
            runtime,
        )?;
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

fn lower_result_constructor(
    builder: &mut FunctionBuilder,
    expr: &IrExpr,
    inner: &IrExpr,
    tag: i64,
    current_return_ty: &Type,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    scope_stack: &Vec<Vec<(LocalId, Variable)>>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<ClValue, CodegenError> {
    if !is_native_result_type(&expr.ty) {
        return Err(CodegenError::not_supported(
            "`Result<T, E>` construction outside the supported native subset â€” native lowering currently supports one-word payload shapes only",
            expr.span,
        ));
    }

    let payload_ty = match (&expr.ty, tag) {
        (Type::Result(ok, _), RESULT_TAG_OK) => &**ok,
        (Type::Result(_, err), RESULT_TAG_ERR) => &**err,
        (Type::Result(_, _), _) => {
            return Err(CodegenError::cranelift(
                format!("invalid native Result tag {tag}"),
                expr.span,
            ))
        }
        _ => {
            return Err(CodegenError::cranelift(
                format!(
                    "Result constructor expression has non-Result type `{}` â€” typecheck should have caught this",
                    expr.ty.display_name()
                ),
                expr.span,
            ))
        }
    };

    let payload = lower_expr(
        builder,
        inner,
        current_return_ty,
        env,
        scope_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;
    emit_result_wrapper_value(builder, module, runtime, &expr.ty, payload, payload_ty, tag, expr.span)
}

fn emit_result_wrapper_value(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    result_ty: &Type,
    payload: ClValue,
    payload_ty: &Type,
    tag: i64,
    span: Span,
) -> Result<ClValue, CodegenError> {
    let result_ti_id = runtime
        .result_typeinfos
        .get(result_ty)
        .copied()
        .ok_or_else(|| {
            CodegenError::cranelift(
                format!(
                    "no typeinfo pre-emitted for Result type `{}`",
                    result_ty.display_name()
                ),
                span,
            )
        })?;
    let size = builder.ins().iconst(I64, RESULT_PAYLOAD_BYTES);
    let ti_gv = module.declare_data_in_func(result_ti_id, builder.func);
    let ti_addr = builder.ins().symbol_value(I64, ti_gv);
    let alloc_ref = module.declare_func_in_func(runtime.alloc_typed, builder.func);
    let call = builder.ins().call(alloc_ref, &[size, ti_addr]);
    let result_ptr = builder.inst_results(call)[0];
    let tag_val = builder.ins().iconst(I64, tag);
    builder.ins().store(
        cranelift_codegen::ir::MemFlags::trusted(),
        tag_val,
        result_ptr,
        RESULT_TAG_OFFSET,
    );
    let payload_cl_ty = cl_type_for(payload_ty, span)?;
    builder.ins().store(
        cranelift_codegen::ir::MemFlags::trusted(),
        payload,
        result_ptr,
        RESULT_PAYLOAD_OFFSET,
    );
    // Keep `payload_cl_ty` used explicitly for the correctness check
    // above: mismatched payload widths should fail before store.
    let _ = payload_cl_ty;
    Ok(result_ptr)
}

fn emit_option_wrapper_value(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    option_ty: &Type,
    payload: ClValue,
    payload_ty: &Type,
    span: Span,
) -> Result<ClValue, CodegenError> {
    let option_ti_id = runtime
        .option_typeinfos
        .get(option_ty)
        .copied()
        .ok_or_else(|| {
            CodegenError::cranelift(
                format!(
                    "no typeinfo pre-emitted for wide Option type `{}`",
                    option_ty.display_name()
                ),
                span,
            )
        })?;
    let size = builder.ins().iconst(I64, OPTION_PAYLOAD_BYTES);
    let ti_gv = module.declare_data_in_func(option_ti_id, builder.func);
    let ti_addr = builder.ins().symbol_value(I64, ti_gv);
    let alloc_ref = module.declare_func_in_func(runtime.alloc_typed, builder.func);
    let call = builder.ins().call(alloc_ref, &[size, ti_addr]);
    let option_ptr = builder.inst_results(call)[0];
    let payload_cl_ty = cl_type_for(payload_ty, span)?;
    builder.ins().store(
        cranelift_codegen::ir::MemFlags::trusted(),
        payload,
        option_ptr,
        OPTION_PAYLOAD_OFFSET,
    );
    let _ = payload_cl_ty;
    Ok(option_ptr)
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
// Prompt call lowering.
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

fn prompt_constant_arg_text(expr: &IrExpr) -> Option<String> {
    match &expr.kind {
        IrExprKind::Literal(IrLiteral::String(s)) => Some(s.clone()),
        IrExprKind::Literal(IrLiteral::Int(n)) => Some(n.to_string()),
        IrExprKind::Literal(IrLiteral::Bool(b)) => Some(if *b {
            "true".to_string()
        } else {
            "false".to_string()
        }),
        _ => None,
    }
}

fn render_prompt_constant(
    segments: &[TemplateSegment<'_>],
    args: &[IrExpr],
) -> Option<String> {
    let mut rendered = String::new();
    for seg in segments {
        match seg {
            TemplateSegment::Literal(text) => rendered.push_str(text),
            TemplateSegment::Param(idx) => {
                rendered.push_str(&prompt_constant_arg_text(&args[*idx])?);
            }
        }
    }
    Some(rendered)
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
                "prompt argument type `{}` is not yet supported in template interpolation — the native prompt bridge currently supports only Int / Bool / Float / String; Struct / List interpolation is not implemented yet",
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
    parts: Vec<(ClValue, bool)>,
    span: Span,
) -> Result<(ClValue, bool), CodegenError> {
    if parts.is_empty() {
        // Empty rendered prompt — emit an empty literal.
        return emit_string_const(builder, module, runtime, "", span).map(|v| (v, false));
    }
    let mut parts = parts.into_iter();
    let (mut acc, mut acc_borrowed) = parts.next().expect("parts not empty");
    let concat_fid = module.declare_func_in_func(runtime.string_concat, builder.func);
    for (next, next_borrowed) in parts {
        let call = builder.ins().call(concat_fid, &[acc, next]);
        let results: Vec<ClValue> =
            builder.inst_results(call).iter().copied().collect();
        let new_acc = results[0];
        // Release the previous accumulator + the just-consumed next.
        // string_concat returns a fresh +1; the inputs are consumed
        // (concat keeps its own copies if it needs them, but the
        // result is independent — release-on-consume is safe).
        //
        // This helper is INTERNAL prompt-building — it operates on
        // freshly-allocated +1 String values produced within the same
        // function (from emit_string_const, string_from_*). Those
        // aren't driven by IrStmt-level ownership, so the .6d pass
        // doesn't touch them. Keep the release unconditional.
        if !acc_borrowed {
            emit_release(builder, module, runtime, acc);
        }
        if !next_borrowed {
            emit_release(builder, module, runtime, next);
        }
        acc = new_acc;
        acc_borrowed = false;
    }
    Ok((acc, acc_borrowed))
}

#[allow(clippy::too_many_arguments)]
fn lower_prompt_call(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    def_id: DefId,
    callee_name: &str,
    args: &[IrExpr],
    current_return_ty: &Type,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    scope_stack: &Vec<Vec<(LocalId, Variable)>>,
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

    let pinned_locals = runtime.prompt_pins.get(&span);

    // 1. Lower each arg expression. Under the unified ownership pass,
    //    bare-Local String args at prompt boundaries may already be
    //    flowing as the binding's existing +1 rather than a fresh temp.
    //    The prompt-pin analysis marks exactly those locals so the
    //    prompt concat path can treat them as borrowed boundary inputs
    //    instead of releasing them like owned temps.
    let mut arg_vals: Vec<ClValue> = Vec::with_capacity(args.len());
    let mut arg_pinned: Vec<bool> = Vec::with_capacity(args.len());
    for a in args {
        let v = lower_expr(
            builder,
            a,
            current_return_ty,
            env,
            scope_stack,
            func_ids_by_def,
            module,
            runtime,
        )?;
        let pinned = matches!(&a.ty, Type::String)
            && matches!(&a.kind, IrExprKind::Local { .. })
            && pinned_locals.is_some_and(|set| {
                matches!(
                    &a.kind,
                    IrExprKind::Local { local_id, .. } if set.contains(local_id)
                )
            });
        arg_vals.push(v);
        arg_pinned.push(pinned);
    }

    // 2. Parse the template into segments at codegen time.
    let segments = parse_prompt_template(&prompt.template, &prompt.params, span)?;

    // 3. Build the rendered prompt. If every interpolated arg is a
    //    compile-time scalar/string literal, fold the full template to
    //    one immortal literal and skip runtime stringify/concat work.
    let (rendered, rendered_borrowed) = if let Some(text) =
        render_prompt_constant(&segments, args)
    {
        (emit_string_const(builder, module, runtime, &text, span)?, false)
    } else {
        let mut parts: Vec<(ClValue, bool)> = Vec::with_capacity(segments.len());
        for seg in &segments {
            let part = match seg {
                TemplateSegment::Literal(text) => (
                    emit_string_const(builder, module, runtime, text, span)?,
                    false,
                ),
                TemplateSegment::Param(idx) => {
                    let av = arg_vals[*idx];
                    let aty = &args[*idx].ty;
                    (
                        emit_stringify_arg(builder, module, runtime, av, aty, span)?,
                        arg_pinned[*idx] && matches!(aty, Type::String),
                    )
                }
            };
            parts.push(part);
        }
        emit_concat_chain(builder, module, runtime, parts, span)?
    };

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
                    "prompt `{callee_name}` returns `{}` — the native prompt bridge currently supports only Int / Bool / Float / String returns; structured prompt returns are not implemented yet",
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
    if !rendered_borrowed {
        emit_release(builder, module, runtime, rendered);
    }
    emit_release(builder, module, runtime, model_val);

    // Release the original arg values (we passed their stringified
    // copies into the rendered prompt, but the originals still hold
    // the +1 they came back from `lower_expr` with). For String args:
    //   * pinned bare-locals were borrowed at the prompt boundary and
    //     intentionally skipped in the concat-chain releases
    //   * non-pinned String temps flow through stringify-as-identity
    //     and get consumed by `emit_concat_chain`
    // Non-String args are scalar and have nothing to release.
    for (v, a) in arg_vals.iter().zip(args.iter()) {
        if is_refcounted_type(&a.ty) {
            let _ = v;
        } else {
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
    current_return_ty: &Type,
    env: &HashMap<LocalId, (Variable, clir::Type)>,
    scope_stack: &Vec<Vec<(LocalId, Variable)>>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<ClValue, CodegenError> {
    let l = lower_expr(
        builder,
        left,
        current_return_ty,
        env,
        scope_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;

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
    let r = lower_expr(
        builder,
        right,
        current_return_ty,
        env,
        scope_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;
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
    current_return_ty: &Type,
    env: &mut HashMap<LocalId, (Variable, clir::Type)>,
    var_idx: &mut usize,
    scope_stack: &mut Vec<Vec<(LocalId, Variable)>>,
    loop_stack: &mut Vec<LoopCtx>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
) -> Result<BlockOutcome, CodegenError> {
    let cond_val = lower_expr(
        builder,
        cond,
        current_return_ty,
        env,
        scope_stack,
        func_ids_by_def,
        module,
        runtime,
    )?;

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
        current_return_ty,
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
        // Under the .6d pass, the analysis handles branch-scope drops
        // via block-exit drops on locals with different last-use
        // points across branches — skip the scattered emission.
        let scope = scope_stack.pop().unwrap_or_default();
        if !runtime.dup_drop_enabled {
            for (_, var) in scope.iter().rev() {
                let v = builder.use_var(*var);
                emit_release(builder, module, runtime, v);
            }
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
            current_return_ty,
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
            if !runtime.dup_drop_enabled {
                for (_, var) in scope.iter().rev() {
                    let v = builder.use_var(*var);
                    emit_release(builder, module, runtime, v);
                }
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

// Silence warnings for fields we expect to use soon.
#[allow(dead_code)]
fn _force_use(_: MemFlags, _: Signature) {}

