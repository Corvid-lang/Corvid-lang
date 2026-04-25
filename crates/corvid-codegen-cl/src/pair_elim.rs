//! Same-block retain/release pair elimination.
//!
//! This pass consumes the IR produced by the unified ownership path
//! (`insert_dup_drop`) and removes `IrStmt::Dup` / `IrStmt::Drop`
//! pairs that are provably redundant in a single straight-line basic
//! block.
//!
//! Scope of this pass:
//!   - only pass-inserted `Dup`/`Drop` are expected to exist in the IR
//!   - only same-block, straight-line pairs are considered
//!   - cross-branch / cross-loop / cross-function elimination is
//!     intentionally out of scope
//!
//! Assumption to document explicitly: today the only producer of
//! `IrStmt::Dup` / `IrStmt::Drop` is the ownership pass. If a future
//! feature introduces hand-authored Dup/Drop or another pass starts
//! emitting them for a different purpose, this pass's safety contract
//! must be re-reviewed.
//!
//! Safepoint note:
//! Cranelift safepoints sit at call instructions, but removing a
//! redundant Dup/Drop pair around a safepoint does NOT change the GC
//! visible live-set. The local's value is still present in the stack
//! map at the same program points; only the transient refcount bump
//! and matching release disappear.

use corvid_ir::{IrAgent, IrBlock, IrCallKind, IrExpr, IrExprKind, IrStmt};
use corvid_resolve::LocalId;

/// Remove same-block redundant `Dup` / `Drop` pairs from `agent`.
///
/// The pass is intentionally conservative:
///   - `Dup(local)` must be followed immediately by a single safe use
///     statement that mentions `local` exactly once.
///   - The matching `Drop(local)` must occur later in the SAME block.
///   - No intervening statement may touch `local`, redefine it, or
///     pass it to code we do not control.
pub fn eliminate_pairs(mut agent: IrAgent) -> IrAgent {
    eliminate_pairs_in_block(&mut agent.body);
    agent
}

fn eliminate_pairs_in_block(block: &mut IrBlock) {
    for stmt in &mut block.stmts {
        match stmt {
            IrStmt::If {
                then_block,
                else_block,
                ..
            } => {
                eliminate_pairs_in_block(then_block);
                if let Some(else_block) = else_block {
                    eliminate_pairs_in_block(else_block);
                }
            }
            IrStmt::For { body, .. } => eliminate_pairs_in_block(body),
            _ => {}
        }
    }

    let mut remove = vec![false; block.stmts.len()];
    let mut idx = 0usize;
    while idx < block.stmts.len() {
        let local_id = match block.stmts.get(idx) {
            Some(IrStmt::Dup { local_id, .. }) => *local_id,
            _ => {
                idx += 1;
                continue;
            }
        };

        let use_idx = idx + 1;
        if use_idx >= block.stmts.len()
            || !stmt_is_pairable_use(&block.stmts[use_idx], local_id)
        {
            idx += 1;
            continue;
        }

        let mut drop_idx = None;
        let mut scan_idx = use_idx + 1;
        while scan_idx < block.stmts.len() {
            let stmt = &block.stmts[scan_idx];
            match stmt {
                IrStmt::Drop {
                    local_id: drop_local,
                    ..
                } if *drop_local == local_id => {
                    drop_idx = Some(scan_idx);
                    break;
                }
                _ if stmt_blocks_pair_search(stmt, local_id) => break,
                _ => scan_idx += 1,
            }
        }

        if let Some(drop_idx) = drop_idx {
            remove[idx] = true;
            remove[drop_idx] = true;
            idx = drop_idx + 1;
        } else {
            idx += 1;
        }
    }

    if remove.iter().any(|flag| *flag) {
        let mut idx = 0usize;
        block.stmts.retain(|_| {
            let keep = !remove[idx];
            idx += 1;
            keep
        });
    }
}

fn stmt_is_pairable_use(stmt: &IrStmt, local_id: LocalId) -> bool {
    if stmt_local_mentions(stmt, local_id) != 1 {
        return false;
    }
    if stmt_observes_refcount(stmt, local_id) {
        return false;
    }

    match stmt {
        IrStmt::Let { local_id: defined, .. } => *defined != local_id,
        IrStmt::Expr { .. } => true,
        _ => false,
    }
}

fn stmt_blocks_pair_search(stmt: &IrStmt, local_id: LocalId) -> bool {
    match stmt {
        IrStmt::If { .. }
        | IrStmt::For { .. }
        | IrStmt::Return { .. }
        | IrStmt::Yield { .. }
        | IrStmt::Break { .. }
        | IrStmt::Continue { .. } => true,
        IrStmt::Approve { args, .. } => args.iter().any(|expr| expr_mentions_local(expr, local_id)),
        IrStmt::Let {
            local_id: defined,
            value,
            ..
        } => *defined == local_id || expr_mentions_local(value, local_id),
        IrStmt::Expr { expr, .. } => expr_mentions_local(expr, local_id),
        IrStmt::Dup {
            local_id: dup_local, ..
        }
        | IrStmt::Drop {
            local_id: dup_local, ..
        } => *dup_local == local_id,
        IrStmt::Pass { .. } => false,
    }
}

fn stmt_local_mentions(stmt: &IrStmt, local_id: LocalId) -> usize {
    match stmt {
        IrStmt::Let { value, .. } => count_local_mentions_expr(value, local_id),
        IrStmt::Return { value, .. } => value
            .as_ref()
            .map(|expr| count_local_mentions_expr(expr, local_id))
            .unwrap_or(0),
        IrStmt::Yield { value, .. } => count_local_mentions_expr(value, local_id),
        IrStmt::If { cond, .. } => count_local_mentions_expr(cond, local_id),
        IrStmt::For { iter, .. } => count_local_mentions_expr(iter, local_id),
        IrStmt::Approve { args, .. } => args
            .iter()
            .map(|expr| count_local_mentions_expr(expr, local_id))
            .sum(),
        IrStmt::Expr { expr, .. } => count_local_mentions_expr(expr, local_id),
        IrStmt::Break { .. }
        | IrStmt::Continue { .. }
        | IrStmt::Pass { .. }
        | IrStmt::Dup { .. }
        | IrStmt::Drop { .. } => 0,
    }
}

fn stmt_observes_refcount(stmt: &IrStmt, local_id: LocalId) -> bool {
    match stmt {
        IrStmt::Let { value, .. }
        | IrStmt::Expr { expr: value, .. }
        | IrStmt::Yield { value, .. } => {
            expr_observes_refcount(value, local_id)
        }
        IrStmt::Approve { args, .. } => args.iter().any(|expr| expr_mentions_local(expr, local_id)),
        _ => true,
    }
}

fn expr_mentions_local(expr: &IrExpr, local_id: LocalId) -> bool {
    count_local_mentions_expr(expr, local_id) > 0
}

fn count_local_mentions_expr(expr: &IrExpr, local_id: LocalId) -> usize {
    match &expr.kind {
        IrExprKind::Literal(_) | IrExprKind::Decl { .. } | IrExprKind::OptionNone => 0,
        IrExprKind::Local { local_id: current, .. } => usize::from(*current == local_id),
        IrExprKind::Call { args, .. } | IrExprKind::List { items: args } => args
            .iter()
            .map(|arg| count_local_mentions_expr(arg, local_id))
            .sum(),
        IrExprKind::FieldAccess { target, .. }
        | IrExprKind::UnwrapGrounded { value: target }
        | IrExprKind::WeakNew { strong: target }
        | IrExprKind::WeakUpgrade { weak: target }
        | IrExprKind::ResultOk { inner: target }
        | IrExprKind::ResultErr { inner: target }
        | IrExprKind::OptionSome { inner: target }
        | IrExprKind::TryPropagate { inner: target }
        | IrExprKind::TryRetry { body: target, .. }
        | IrExprKind::UnOp { operand: target, .. } => count_local_mentions_expr(target, local_id),
        IrExprKind::Index { target, index } | IrExprKind::BinOp { left: target, right: index, .. } => {
            count_local_mentions_expr(target, local_id)
                + count_local_mentions_expr(index, local_id)
        }
        IrExprKind::Replay { trace, arms, else_body } => {
            let mut total = count_local_mentions_expr(trace, local_id);
            for arm in arms {
                total += count_local_mentions_expr(&arm.body, local_id);
            }
            total + count_local_mentions_expr(else_body, local_id)
        }
    }
}

fn expr_observes_refcount(expr: &IrExpr, local_id: LocalId) -> bool {
    match &expr.kind {
        IrExprKind::Literal(_) | IrExprKind::Decl { .. } | IrExprKind::OptionNone => false,
        IrExprKind::Local { .. } => false,
        IrExprKind::Call { kind, args, .. } => {
            let local_in_args = args.iter().any(|arg| expr_mentions_local(arg, local_id));
            let external_observer = matches!(
                kind,
                IrCallKind::Tool { .. }
                    | IrCallKind::Prompt { .. }
                    | IrCallKind::Agent { .. }
                    | IrCallKind::Unknown
            );
            (external_observer && local_in_args)
                || args.iter().any(|arg| expr_observes_refcount(arg, local_id))
        }
        IrExprKind::FieldAccess { target, .. }
        | IrExprKind::UnwrapGrounded { value: target }
        | IrExprKind::WeakUpgrade { weak: target }
        | IrExprKind::ResultOk { inner: target }
        | IrExprKind::ResultErr { inner: target }
        | IrExprKind::OptionSome { inner: target }
        | IrExprKind::TryPropagate { inner: target }
        | IrExprKind::TryRetry { body: target, .. }
        | IrExprKind::UnOp { operand: target, .. } => expr_observes_refcount(target, local_id),
        IrExprKind::WeakNew { strong } => {
            expr_mentions_local(strong, local_id) || expr_observes_refcount(strong, local_id)
        }
        IrExprKind::Index { target, index } | IrExprKind::BinOp { left: target, right: index, .. } => {
            expr_observes_refcount(target, local_id) || expr_observes_refcount(index, local_id)
        }
        IrExprKind::List { items } => items.iter().any(|item| expr_observes_refcount(item, local_id)),
        IrExprKind::Replay { trace, arms, else_body } => {
            expr_observes_refcount(trace, local_id)
                || arms.iter().any(|arm| expr_observes_refcount(&arm.body, local_id))
                || expr_observes_refcount(else_body, local_id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_ast::{BinaryOp, Span};
    use corvid_ir::{IrAgent, IrBlock, IrCallKind, IrExpr, IrExprKind, IrLiteral, IrParam, IrStmt};
    use corvid_resolve::DefId;
    use corvid_types::Type;

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

    fn int_lit(n: i64) -> IrExpr {
        IrExpr {
            kind: IrExprKind::Literal(IrLiteral::Int(n)),
            ty: Type::Int,
            span: span(),
        }
    }

    fn tool_call(arg: IrExpr) -> IrExpr {
        IrExpr {
            kind: IrExprKind::Call {
                kind: IrCallKind::Tool {
                    def_id: DefId(0),
                    effect: corvid_ast::Effect::Safe,
                },
                callee_name: "ship".into(),
                args: vec![arg],
            },
            ty: Type::String,
            span: span(),
        }
    }

    fn make_agent(body: Vec<IrStmt>) -> IrAgent {
        IrAgent {
            id: DefId(0),
            name: "test".into(),
            extern_abi: None,
            params: vec![IrParam {
                name: "s".into(),
                local_id: LocalId(0),
                ty: Type::String,
                span: span(),
            }],
            return_ty: Type::Int,
            cost_budget: None,
            body: IrBlock { stmts: body, span: span() },
            span: span(),
            borrow_sig: None,
        }
    }

    fn dup(local: u32) -> IrStmt {
        IrStmt::Dup {
            local_id: LocalId(local),
            span: span(),
        }
    }

    fn drop_stmt(local: u32) -> IrStmt {
        IrStmt::Drop {
            local_id: LocalId(local),
            span: span(),
        }
    }

    #[test]
    fn simple_dup_use_drop_pair_eliminated() {
        let agent = make_agent(vec![
            dup(0),
            IrStmt::Let {
                local_id: LocalId(1),
                name: "t".into(),
                ty: Type::String,
                value: local_expr(0, Type::String),
                span: span(),
            },
            drop_stmt(0),
        ]);

        let out = eliminate_pairs(agent);
        assert_eq!(out.body.stmts.len(), 1);
        assert!(matches!(out.body.stmts[0], IrStmt::Let { .. }));
    }

    #[test]
    fn pair_not_eliminated_when_tool_call_uses_local() {
        let agent = make_agent(vec![
            dup(0),
            IrStmt::Expr {
                expr: tool_call(local_expr(0, Type::String)),
                span: span(),
            },
            drop_stmt(0),
        ]);

        let out = eliminate_pairs(agent);
        assert_eq!(out.body.stmts.len(), 3);
        assert!(matches!(out.body.stmts[0], IrStmt::Dup { .. }));
        assert!(matches!(out.body.stmts[2], IrStmt::Drop { .. }));
    }

    #[test]
    fn pair_not_eliminated_across_asymmetric_if_merge() {
        let agent = make_agent(vec![
            dup(0),
            IrStmt::If {
                cond: IrExpr {
                    kind: IrExprKind::Literal(IrLiteral::Bool(true)),
                    ty: Type::Bool,
                    span: span(),
                },
                then_block: IrBlock {
                    stmts: vec![drop_stmt(0)],
                    span: span(),
                },
                else_block: Some(IrBlock {
                    stmts: vec![IrStmt::Pass { span: span() }],
                    span: span(),
                }),
                span: span(),
            },
        ]);

        let out = eliminate_pairs(agent);
        assert_eq!(out.body.stmts.len(), 2);
        assert!(matches!(out.body.stmts[0], IrStmt::Dup { .. }));
        if let IrStmt::If { then_block, .. } = &out.body.stmts[1] {
            assert!(matches!(then_block.stmts[0], IrStmt::Drop { .. }));
        } else {
            panic!("expected If");
        }
    }

    #[test]
    fn pair_eliminated_when_intervening_statements_do_not_touch_local() {
        let agent = make_agent(vec![
            dup(0),
            IrStmt::Let {
                local_id: LocalId(1),
                name: "t".into(),
                ty: Type::String,
                value: local_expr(0, Type::String),
                span: span(),
            },
            IrStmt::Let {
                local_id: LocalId(2),
                name: "x".into(),
                ty: Type::Int,
                value: IrExpr {
                    kind: IrExprKind::BinOp {
                        op: BinaryOp::Add,
                        left: Box::new(int_lit(1)),
                        right: Box::new(int_lit(2)),
                    },
                    ty: Type::Int,
                    span: span(),
                },
                span: span(),
            },
            IrStmt::Pass { span: span() },
            drop_stmt(0),
        ]);

        let out = eliminate_pairs(agent);
        assert_eq!(out.body.stmts.len(), 3);
        assert!(matches!(out.body.stmts[0], IrStmt::Let { local_id: LocalId(1), .. }));
        assert!(matches!(out.body.stmts[1], IrStmt::Let { local_id: LocalId(2), .. }));
        assert!(matches!(out.body.stmts[2], IrStmt::Pass { .. }));
    }
}
