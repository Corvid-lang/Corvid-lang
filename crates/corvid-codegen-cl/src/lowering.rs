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
mod entry;
use entry::*;

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
// Silence warnings for fields we expect to use soon.
#[allow(dead_code)]
fn _force_use(_: MemFlags, _: Signature) {}


