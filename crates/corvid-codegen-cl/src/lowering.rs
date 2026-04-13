//! IR → Cranelift IR lowering.
//!
//! Slice 12a scope: `Int` parameters, `Int` return, `Int` literals,
//! `Int` arithmetic with overflow trap, agent-to-agent calls,
//! parameter loads, `return`. Everything else raises
//! `CodegenError::NotSupported` with a slice pointer so the boundary
//! is auditable.

use crate::errors::CodegenError;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::types::{I64, I8};
use cranelift_codegen::ir::{
    self as clir, AbiParam, Function, InstBuilder, MemFlags, Signature, UserFuncName,
    Value as ClValue,
};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{FuncId, Linkage, Module};
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
        define_agent(module, agent, func_id, &func_ids_by_def, overflow_func_id)?;
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
                    "parameter `{}: {}` — slice 12b supports only `Int` and `Bool` parameters (Float/String/Struct/List land in slice 12d)",
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
                "agent `{}` returns `{}` — slice 12b supports only `Int` and `Bool` returns",
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
        Type::Float => Err(CodegenError::not_supported(
            "`Float` — slice 12d adds floating-point",
            span,
        )),
        Type::String => Err(CodegenError::not_supported(
            "`String` — slice 12d adds strings",
            span,
        )),
        Type::Struct(_) => Err(CodegenError::not_supported(
            "`Struct` — slice 12d adds struct layout",
            span,
        )),
        Type::List(_) => Err(CodegenError::not_supported(
            "`List` — slice 12d adds lists",
            span,
        )),
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
    overflow_func_id: FuncId,
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

        let mut env: HashMap<LocalId, Variable> = HashMap::new();
        let mut var_idx: usize = 0;

        for (i, p) in agent.params.iter().enumerate() {
            let block_arg = builder.block_params(entry)[i];
            let var = Variable::from_u32(var_idx as u32);
            var_idx += 1;
            let ty = cl_type_for(&p.ty, p.span)?;
            builder.declare_var(var, ty);
            builder.def_var(var, block_arg);
            env.insert(p.local_id, var);
        }

        let lowered = lower_block(
            &mut builder,
            &agent.body,
            &mut env,
            &mut var_idx,
            func_ids_by_def,
            module,
            overflow_func_id,
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
    env: &mut HashMap<LocalId, Variable>,
    _var_idx: &mut usize,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    overflow_func_id: FuncId,
) -> Result<BlockOutcome, CodegenError> {
    for stmt in &block.stmts {
        match lower_stmt(builder, stmt, env, _var_idx, func_ids_by_def, module, overflow_func_id)? {
            BlockOutcome::Terminated => return Ok(BlockOutcome::Terminated),
            BlockOutcome::Normal => {}
        }
    }
    Ok(BlockOutcome::Normal)
}

fn lower_stmt(
    builder: &mut FunctionBuilder,
    stmt: &IrStmt,
    env: &mut HashMap<LocalId, Variable>,
    var_idx: &mut usize,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    overflow_func_id: FuncId,
) -> Result<BlockOutcome, CodegenError> {
    match stmt {
        IrStmt::Return { value, span } => {
            let v = match value {
                Some(e) => lower_expr(builder, e, env, func_ids_by_def, module, overflow_func_id)?,
                None => {
                    return Err(CodegenError::not_supported(
                        "bare `return` (Nothing type not supported in slice 12a)",
                        *span,
                    ));
                }
            };
            builder.ins().return_(&[v]);
            Ok(BlockOutcome::Terminated)
        }
        IrStmt::Expr { expr, .. } => {
            let _ = lower_expr(builder, expr, env, func_ids_by_def, module, overflow_func_id)?;
            Ok(BlockOutcome::Normal)
        }
        IrStmt::Let { span, .. } => Err(CodegenError::not_supported(
            "`let` bindings — slice 12c adds them",
            *span,
        )),
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
            func_ids_by_def,
            module,
            overflow_func_id,
        ),
        IrStmt::For { span, .. } => Err(CodegenError::not_supported(
            "`for` loops — slice 12c adds them",
            *span,
        )),
        IrStmt::Approve { span, .. } => Err(CodegenError::not_supported(
            "`approve` in compiled code — Phase 14 adds it alongside the tool registry",
            *span,
        )),
        IrStmt::Break { span } | IrStmt::Continue { span } | IrStmt::Pass { span } => {
            Err(CodegenError::not_supported(
                "loop control flow — slice 12c adds it",
                *span,
            ))
        }
    }
}

fn lower_expr(
    builder: &mut FunctionBuilder,
    expr: &IrExpr,
    env: &HashMap<LocalId, Variable>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    overflow_func_id: FuncId,
) -> Result<ClValue, CodegenError> {
    match &expr.kind {
        IrExprKind::Literal(IrLiteral::Int(n)) => Ok(builder.ins().iconst(I64, *n)),
        IrExprKind::Literal(IrLiteral::Bool(b)) => {
            Ok(builder.ins().iconst(I8, if *b { 1 } else { 0 }))
        }
        IrExprKind::Literal(IrLiteral::Float(_)) => Err(CodegenError::not_supported(
            "`Float` literals — slice 12d adds floating-point",
            expr.span,
        )),
        IrExprKind::Literal(IrLiteral::String(_)) => Err(CodegenError::not_supported(
            "`String` literals — slice 12d adds strings",
            expr.span,
        )),
        IrExprKind::Literal(IrLiteral::Nothing) => Err(CodegenError::not_supported(
            "`nothing` literal — slice 12d adds it alongside the `Nothing` type",
            expr.span,
        )),
        IrExprKind::Local { local_id, name } => {
            let var = env.get(local_id).ok_or_else(|| {
                CodegenError::cranelift(
                    format!("no variable for local `{name}` — compiler bug"),
                    expr.span,
                )
            })?;
            Ok(builder.use_var(*var))
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
                    overflow_func_id,
                );
            }
            let l = lower_expr(builder, left, env, func_ids_by_def, module, overflow_func_id)?;
            let r = lower_expr(builder, right, env, func_ids_by_def, module, overflow_func_id)?;
            lower_binop_strict(builder, *op, l, r, expr.span, module, overflow_func_id)
        }
        IrExprKind::UnOp { op, operand } => {
            let v = lower_expr(builder, operand, env, func_ids_by_def, module, overflow_func_id)?;
            lower_unop(builder, *op, v, expr.span, module, overflow_func_id)
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
                let mut arg_vals = Vec::with_capacity(args.len());
                for a in args {
                    arg_vals.push(lower_expr(
                        builder,
                        a,
                        env,
                        func_ids_by_def,
                        module,
                        overflow_func_id,
                    )?);
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
                Ok(results[0])
            }
            IrCallKind::Tool { .. } | IrCallKind::Prompt { .. } => Err(CodegenError::not_supported(
                "tool / prompt calls in compiled code — Phase 14 adds them",
                expr.span,
            )),
            IrCallKind::Unknown => Err(CodegenError::cranelift(
                format!("call to `{callee_name}` did not resolve — typecheck should have caught this"),
                expr.span,
            )),
        },
        IrExprKind::FieldAccess { .. } => Err(CodegenError::not_supported(
            "field access — slice 12d adds structs",
            expr.span,
        )),
        IrExprKind::Index { .. } => Err(CodegenError::not_supported(
            "list indexing — slice 12d adds lists",
            expr.span,
        )),
        IrExprKind::List { .. } => Err(CodegenError::not_supported(
            "list literals — slice 12d adds lists",
            expr.span,
        )),
    }
}

/// Int arithmetic (overflow-trapping) and comparison (Int or Bool).
/// Short-circuit `and`/`or` are handled in `lower_short_circuit`, not
/// here — by the time this helper runs, both sides have been evaluated.
fn lower_binop_strict(
    builder: &mut FunctionBuilder,
    op: BinaryOp,
    l: ClValue,
    r: ClValue,
    span: Span,
    module: &mut ObjectModule,
    overflow_func_id: FuncId,
) -> Result<ClValue, CodegenError> {
    match op {
        BinaryOp::Add => with_overflow_trap(builder, l, r, module, overflow_func_id, |b| {
            b.ins().sadd_overflow(l, r)
        }),
        BinaryOp::Sub => with_overflow_trap(builder, l, r, module, overflow_func_id, |b| {
            b.ins().ssub_overflow(l, r)
        }),
        BinaryOp::Mul => with_overflow_trap(builder, l, r, module, overflow_func_id, |b| {
            b.ins().smul_overflow(l, r)
        }),
        BinaryOp::Div => {
            trap_on_zero(builder, r, module, overflow_func_id);
            Ok(builder.ins().sdiv(l, r))
        }
        BinaryOp::Mod => {
            trap_on_zero(builder, r, module, overflow_func_id);
            Ok(builder.ins().srem(l, r))
        }
        BinaryOp::Eq => Ok(builder.ins().icmp(IntCC::Equal, l, r)),
        BinaryOp::NotEq => Ok(builder.ins().icmp(IntCC::NotEqual, l, r)),
        BinaryOp::Lt => Ok(builder.ins().icmp(IntCC::SignedLessThan, l, r)),
        BinaryOp::LtEq => Ok(builder.ins().icmp(IntCC::SignedLessThanOrEqual, l, r)),
        BinaryOp::Gt => Ok(builder.ins().icmp(IntCC::SignedGreaterThan, l, r)),
        BinaryOp::GtEq => Ok(builder.ins().icmp(IntCC::SignedGreaterThanOrEqual, l, r)),
        BinaryOp::And | BinaryOp::Or => {
            let _ = span;
            unreachable!("and/or is short-circuited upstream and never reaches lower_binop_strict")
        }
    }
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
    _span: Span,
    module: &mut ObjectModule,
    overflow_func_id: FuncId,
) -> Result<ClValue, CodegenError> {
    match op {
        UnaryOp::Not => {
            let zero = builder.ins().iconst(I8, 0);
            Ok(builder.ins().icmp(IntCC::Equal, v, zero))
        }
        UnaryOp::Neg => {
            // `-x` ≡ `0 - x`, trap on overflow (only at i64::MIN).
            let zero = builder.ins().iconst(I64, 0);
            with_overflow_trap(builder, zero, v, module, overflow_func_id, |b| {
                b.ins().ssub_overflow(zero, v)
            })
        }
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
    env: &HashMap<LocalId, Variable>,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    overflow_func_id: FuncId,
) -> Result<ClValue, CodegenError> {
    let l = lower_expr(builder, left, env, func_ids_by_def, module, overflow_func_id)?;

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
    let r = lower_expr(builder, right, env, func_ids_by_def, module, overflow_func_id)?;
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
    env: &mut HashMap<LocalId, Variable>,
    var_idx: &mut usize,
    func_ids_by_def: &HashMap<DefId, FuncId>,
    module: &mut ObjectModule,
    overflow_func_id: FuncId,
) -> Result<BlockOutcome, CodegenError> {
    let cond_val = lower_expr(builder, cond, env, func_ids_by_def, module, overflow_func_id)?;

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

    // Then branch.
    builder.switch_to_block(then_b);
    builder.seal_block(then_b);
    let then_outcome = lower_block(
        builder,
        then_block_ir,
        env,
        var_idx,
        func_ids_by_def,
        module,
        overflow_func_id,
    )?;
    if matches!(then_outcome, BlockOutcome::Normal) {
        builder.ins().jump(merge_b, &[]);
        any_fell_through = true;
    }

    // Else branch (if present).
    if let (Some(else_b), Some(else_body)) = (else_b, else_block_ir) {
        builder.switch_to_block(else_b);
        builder.seal_block(else_b);
        let else_outcome = lower_block(
            builder,
            else_body,
            env,
            var_idx,
            func_ids_by_def,
            module,
            overflow_func_id,
        )?;
        if matches!(else_outcome, BlockOutcome::Normal) {
            builder.ins().jump(merge_b, &[]);
            any_fell_through = true;
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
    overflow_func_id: FuncId,
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
    let callee_ref = module.declare_func_in_func(overflow_func_id, builder.func);
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
    overflow_func_id: FuncId,
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
    let callee_ref = module.declare_func_in_func(overflow_func_id, builder.func);
    builder.ins().call(callee_ref, &[]);
    builder.ins().trap(cranelift_codegen::ir::TrapCode::INTEGER_OVERFLOW);
    builder.switch_to_block(cont_block);
    builder.seal_block(cont_block);
}

// Silence warnings for fields we'll start using in slice 12b.
#[allow(dead_code)]
fn _force_use(_: MemFlags, _: Signature) {}
