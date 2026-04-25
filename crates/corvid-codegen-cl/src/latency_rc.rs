//! Latency-aware RC across prompt / LLM boundaries.
//!
//! Current scope is intentionally narrow:
//!   - prompt / LLM boundaries only
//!   - tool-only boundaries are left alone because the default-on
//!     unified ownership path already flattened borrowed-local tool
//!     args close to zero boundary RC traffic
//!   - no runtime deferred-RC queue
//!   - no verifier ledger changes
//!   - no pinning of internal prompt-bridge temps (concat
//!     accumulator, stringify temps, prompt-name/signature/model
//!     literals still keep their real ownership)
//!
//! The pass identifies bare-Local String arguments at prompt call
//! sites that are already classified as Borrowed by the ownership
//! analysis. Codegen can then treat those values as boundary-pinned:
//! they are read by the prompt boundary but must not be released by
//! the prompt-template concat path.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use corvid_ast::Span;
use corvid_ir::{IrAgent, IrBlock, IrCallKind, IrExpr, IrExprKind, IrStmt};
use corvid_resolve::LocalId;
use corvid_types::Type;

use crate::dataflow::{analyze_agent_full, CfgStmt, ProgramPoint, ReadKind};

#[derive(Debug, Clone, Default)]
pub struct PromptPinInfo {
    by_span: HashMap<Span, BTreeSet<LocalId>>,
}

impl PromptPinInfo {
    pub fn pinned_locals(&self, span: Span) -> Option<&BTreeSet<LocalId>> {
        self.by_span.get(&span)
    }

    pub fn pinned_by_span(&self) -> &HashMap<Span, BTreeSet<LocalId>> {
        &self.by_span
    }
}

pub fn analyze_prompt_pins(agent: &IrAgent) -> PromptPinInfo {
    let (cfg, _liveness, plan) = analyze_agent_full(agent);
    let mut point_by_path: BTreeMap<_, _> = BTreeMap::new();
    for (pp, path) in &plan.ir_paths {
        point_by_path.insert(path.clone(), *pp);
    }

    let mut out = PromptPinInfo::default();
    collect_prompt_pins_in_block(&agent.body, &mut Vec::new(), &cfg.blocks, &point_by_path, &mut out);
    out
}

fn collect_prompt_pins_in_block(
    block: &IrBlock,
    parent: &mut Vec<crate::dataflow::IrNavStep>,
    cfg_blocks: &[crate::dataflow::CfgBlock],
    point_by_path: &BTreeMap<crate::dataflow::IrPath, ProgramPoint>,
    out: &mut PromptPinInfo,
) {
    for (idx, stmt) in block.stmts.iter().enumerate() {
        parent.push(crate::dataflow::IrNavStep::Stmt(idx));
        if let Some(pp) = point_by_path.get(parent) {
            let borrowed_reads = borrowed_reads_for_stmt(&cfg_blocks[pp.0].stmts[pp.1]);
            collect_prompt_pins_in_stmt(stmt, &borrowed_reads, out);
        }

        match stmt {
            IrStmt::If {
                then_block,
                else_block,
                ..
            } => {
                parent.push(crate::dataflow::IrNavStep::IfThen);
                collect_prompt_pins_in_block(then_block, parent, cfg_blocks, point_by_path, out);
                parent.pop();
                if let Some(else_block) = else_block {
                    parent.push(crate::dataflow::IrNavStep::IfElse);
                    collect_prompt_pins_in_block(else_block, parent, cfg_blocks, point_by_path, out);
                    parent.pop();
                }
            }
            IrStmt::For { body, .. } => {
                parent.push(crate::dataflow::IrNavStep::ForBody);
                collect_prompt_pins_in_block(body, parent, cfg_blocks, point_by_path, out);
                parent.pop();
            }
            _ => {}
        }

        parent.pop();
    }
}

fn collect_prompt_pins_in_stmt(
    stmt: &IrStmt,
    borrowed_reads: &BTreeSet<LocalId>,
    out: &mut PromptPinInfo,
) {
    match stmt {
        IrStmt::Let { value, .. } => collect_prompt_pins_in_expr(value, borrowed_reads, out),
        IrStmt::Return { value, .. } => {
            if let Some(value) = value {
                collect_prompt_pins_in_expr(value, borrowed_reads, out);
            }
        }
        IrStmt::Yield { value, .. } => collect_prompt_pins_in_expr(value, borrowed_reads, out),
        IrStmt::Expr { expr, .. } => collect_prompt_pins_in_expr(expr, borrowed_reads, out),
        IrStmt::If { cond, .. } => collect_prompt_pins_in_expr(cond, borrowed_reads, out),
        IrStmt::For { iter, .. } => collect_prompt_pins_in_expr(iter, borrowed_reads, out),
        IrStmt::Approve { args, .. } => {
            for arg in args {
                collect_prompt_pins_in_expr(arg, borrowed_reads, out);
            }
        }
        IrStmt::Break { .. }
        | IrStmt::Continue { .. }
        | IrStmt::Pass { .. }
        | IrStmt::Dup { .. }
        | IrStmt::Drop { .. } => {}
    }
}

fn collect_prompt_pins_in_expr(
    expr: &IrExpr,
    borrowed_reads: &BTreeSet<LocalId>,
    out: &mut PromptPinInfo,
) {
    match &expr.kind {
        IrExprKind::Call {
            kind: IrCallKind::Prompt { .. },
            args,
            ..
        } => {
            let mut pinned = BTreeSet::new();
            for arg in args {
                if let IrExprKind::Local { local_id, .. } = arg.kind {
                    if matches!(arg.ty, Type::String) && borrowed_reads.contains(&local_id) {
                        pinned.insert(local_id);
                    }
                }
            }
            if !pinned.is_empty() {
                out.by_span.insert(expr.span, pinned);
            }
            for arg in args {
                collect_prompt_pins_in_expr(arg, borrowed_reads, out);
            }
        }
        IrExprKind::Call { args, .. } | IrExprKind::List { items: args } => {
            for arg in args {
                collect_prompt_pins_in_expr(arg, borrowed_reads, out);
            }
        }
        IrExprKind::FieldAccess { target, .. }
        | IrExprKind::UnwrapGrounded { value: target }
        | IrExprKind::WeakNew { strong: target }
        | IrExprKind::WeakUpgrade { weak: target }
        | IrExprKind::StreamResumeToken { stream: target }
        | IrExprKind::ResumeStream { token: target, .. }
        | IrExprKind::ResultOk { inner: target }
        | IrExprKind::ResultErr { inner: target }
        | IrExprKind::OptionSome { inner: target }
        | IrExprKind::TryPropagate { inner: target }
        | IrExprKind::TryRetry { body: target, .. }
        | IrExprKind::UnOp { operand: target, .. }
        | IrExprKind::WrappingUnOp { operand: target, .. } => {
            collect_prompt_pins_in_expr(target, borrowed_reads, out)
        }
        IrExprKind::Index { target, index }
        | IrExprKind::BinOp {
            left: target,
            right: index,
            ..
        }
        | IrExprKind::WrappingBinOp {
            left: target,
            right: index,
            ..
        } => {
            collect_prompt_pins_in_expr(target, borrowed_reads, out);
            collect_prompt_pins_in_expr(index, borrowed_reads, out);
        }
        IrExprKind::Literal(_)
        | IrExprKind::Local { .. }
        | IrExprKind::Decl { .. }
        | IrExprKind::OptionNone => {}
        IrExprKind::Replay { trace, arms, else_body } => {
            // Replay expressions can't reach native codegen today
            // (cl_type_for rejects TraceId and routes to interpreter
            // tier), but we still walk children so if that boundary
            // is ever relaxed the prompt-pin analysis degrades to
            // zero-pins rather than silently skipping interior
            // prompt calls.
            collect_prompt_pins_in_expr(trace, borrowed_reads, out);
            for arm in arms {
                collect_prompt_pins_in_expr(&arm.body, borrowed_reads, out);
            }
            collect_prompt_pins_in_expr(else_body, borrowed_reads, out);
        }
    }
}

fn borrowed_reads_for_stmt(stmt: &CfgStmt) -> BTreeSet<LocalId> {
    let reads = match stmt {
        CfgStmt::Let { reads, .. }
        | CfgStmt::Expr { reads }
        | CfgStmt::Return { reads }
        | CfgStmt::Branch { reads }
        | CfgStmt::LoopHead { reads, .. }
        | CfgStmt::Other { reads } => reads,
    };
    reads.iter()
        .filter(|r| matches!(r.kind, ReadKind::Borrowed))
        .map(|r| r.local_id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_ast::Span;
    use corvid_ir::IrParam;
    use corvid_resolve::DefId;

    fn span() -> Span {
        Span { start: 0, end: 0 }
    }

    fn local_expr(id: u32, ty: Type) -> IrExpr {
        IrExpr {
            kind: IrExprKind::Local {
                local_id: LocalId(id),
                name: format!("l{id}"),
            },
            ty,
            span: span(),
        }
    }

    fn prompt_call(args: Vec<IrExpr>, span: Span) -> IrExpr {
        IrExpr {
            kind: IrExprKind::Call {
                kind: IrCallKind::Prompt { def_id: DefId(7) },
                callee_name: "classify".into(),
                args,
            },
            ty: Type::Int,
            span,
        }
    }

    fn test_agent(body: Vec<IrStmt>) -> IrAgent {
        IrAgent {
            id: DefId(0),
            name: "f".into(),
            extern_abi: None,
            params: vec![IrParam {
                name: "s".into(),
                local_id: LocalId(0),
                ty: Type::String,
                span: span(),
            }],
            return_ty: Type::Int,
            cost_budget: None,
            wrapping_arithmetic: false,
            body: IrBlock { stmts: body, span: span() },
            span: span(),
            borrow_sig: None,
        }
    }

    #[test]
    fn marks_bare_local_string_prompt_args_as_pinned() {
        let call_span = Span { start: 10, end: 20 };
        let agent = test_agent(vec![IrStmt::Return {
            value: Some(prompt_call(vec![local_expr(0, Type::String)], call_span)),
            span: span(),
        }]);
        let pins = analyze_prompt_pins(&agent);
        let set = pins.pinned_locals(call_span).expect("pin set");
        assert!(set.contains(&LocalId(0)));
    }

    #[test]
    fn ignores_non_local_prompt_args() {
        let call_span = Span { start: 10, end: 20 };
        let agent = test_agent(vec![IrStmt::Return {
            value: Some(prompt_call(
                vec![IrExpr {
                    kind: IrExprKind::Literal(corvid_ir::IrLiteral::String("hi".into())),
                    ty: Type::String,
                    span: span(),
                }],
                call_span,
            )),
            span: span(),
        }]);
        let pins = analyze_prompt_pins(&agent);
        assert!(pins.pinned_locals(call_span).is_none());
    }
}
