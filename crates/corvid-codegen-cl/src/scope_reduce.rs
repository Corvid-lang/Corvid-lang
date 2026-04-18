//! Effect-typed scope reduction.
//!
//! This pass runs after `insert_dup_drop` and after same-block pair
//! elimination. It shortens RC-alive windows by moving `IrStmt::Drop`
//! earlier when the interval between a binding's defining `Let` and
//! its current `Drop` contains only effect-free statements after the
//! value's last use.
//!
//! Current scope is intentionally conservative:
//!   - only same-block relocation
//!   - no movement across nested control-flow boundaries
//!   - every non-trivial call kind is treated as an effect barrier
//!   - `Approve`, `If`, `For`, `Return`, `Break`, `Continue`, `Dup`,
//!     and `Drop` are effect barriers
//!   - only literal-producing / local-read / arithmetic / unary pure
//!     expressions count as effect-free
//!
//! The point of this pass is not maximum movement. It is to make the
//! ownership pass aware of effect-free windows without reopening the
//! active dataflow files.

use std::collections::BTreeSet;

use corvid_ir::{IrAgent, IrBlock, IrExpr, IrExprKind, IrStmt};
use corvid_resolve::LocalId;

use crate::dataflow::{IrNavStep, IrPath};

#[derive(Debug, Clone, Default)]
pub struct EffectInfo {
    barriers: BTreeSet<IrPath>,
}

impl EffectInfo {
    pub fn is_barrier(&self, path: &IrPath) -> bool {
        self.barriers.contains(path)
    }

    fn insert_barrier(&mut self, path: IrPath) {
        self.barriers.insert(path);
    }
}

pub fn analyze_effects(agent: &IrAgent) -> EffectInfo {
    let mut info = EffectInfo::default();
    collect_effects_in_block(&agent.body, &mut Vec::new(), &mut info);
    info
}

pub fn reduce_scope(mut agent: IrAgent, effect_info: &EffectInfo) -> IrAgent {
    reduce_block(&mut agent.body, &mut Vec::new(), effect_info);
    agent
}

fn collect_effects_in_block(block: &IrBlock, parent: &mut IrPath, info: &mut EffectInfo) {
    for (idx, stmt) in block.stmts.iter().enumerate() {
        parent.push(IrNavStep::Stmt(idx));
        let path = parent.clone();
        if stmt_is_effect_barrier(stmt) {
            info.insert_barrier(path.clone());
        }
        match stmt {
            IrStmt::If {
                then_block,
                else_block,
                ..
            } => {
                parent.push(IrNavStep::IfThen);
                collect_effects_in_block(then_block, parent, info);
                parent.pop();
                if let Some(else_block) = else_block {
                    parent.push(IrNavStep::IfElse);
                    collect_effects_in_block(else_block, parent, info);
                    parent.pop();
                }
            }
            IrStmt::For { body, .. } => {
                parent.push(IrNavStep::ForBody);
                collect_effects_in_block(body, parent, info);
                parent.pop();
            }
            _ => {}
        }
        parent.pop();
    }
}

fn reduce_block(block: &mut IrBlock, parent: &mut IrPath, effect_info: &EffectInfo) {
    let mut barrier_flags: Vec<bool> = (0..block.stmts.len())
        .map(|idx| effect_info.is_barrier(&stmt_path(parent, idx)))
        .collect();

    for (idx, stmt) in block.stmts.iter_mut().enumerate() {
        parent.push(IrNavStep::Stmt(idx));
        match stmt {
            IrStmt::If {
                then_block,
                else_block,
                ..
            } => {
                parent.push(IrNavStep::IfThen);
                reduce_block(then_block, parent, effect_info);
                parent.pop();
                if let Some(else_block) = else_block {
                    parent.push(IrNavStep::IfElse);
                    reduce_block(else_block, parent, effect_info);
                    parent.pop();
                }
            }
            IrStmt::For { body, .. } => {
                parent.push(IrNavStep::ForBody);
                reduce_block(body, parent, effect_info);
                parent.pop();
            }
            _ => {}
        }
        parent.pop();
    }

    let mut idx = 0usize;
    while idx < block.stmts.len() {
        let local_id = match block.stmts.get(idx) {
            Some(IrStmt::Drop { local_id, .. }) => *local_id,
            _ => {
                idx += 1;
                continue;
            }
        };

        let Some(def_idx) = find_def_in_same_block(&block.stmts, idx, local_id) else {
            idx += 1;
            continue;
        };

        let mut move_after = def_idx;
        for scan_idx in def_idx + 1..idx {
            if stmt_mentions_local(&block.stmts[scan_idx], local_id)
                || barrier_flags[scan_idx]
            {
                move_after = scan_idx;
            }
        }

        if move_after + 1 < idx {
            let drop_stmt = block.stmts.remove(idx);
            let drop_barrier = barrier_flags.remove(idx);
            block.stmts.insert(move_after + 1, drop_stmt);
            barrier_flags.insert(move_after + 1, drop_barrier);
            idx = move_after + 2;
        } else {
            idx += 1;
        }
    }
}

fn stmt_path(parent: &IrPath, stmt_idx: usize) -> IrPath {
    let mut out = parent.clone();
    out.push(IrNavStep::Stmt(stmt_idx));
    out
}

fn find_def_in_same_block(stmts: &[IrStmt], drop_idx: usize, local_id: LocalId) -> Option<usize> {
    (0..drop_idx).rev().find(|idx| {
        matches!(
            &stmts[*idx],
            IrStmt::Let {
                local_id: defined,
                ..
            } if *defined == local_id
        )
    })
}

fn stmt_mentions_local(stmt: &IrStmt, local_id: LocalId) -> bool {
    match stmt {
        IrStmt::Let { value, .. } => expr_mentions_local(value, local_id),
        IrStmt::Return { value, .. } => value
            .as_ref()
            .map(|value| expr_mentions_local(value, local_id))
            .unwrap_or(false),
        IrStmt::Yield { value, .. } => expr_mentions_local(value, local_id),
        IrStmt::If { cond, .. } => expr_mentions_local(cond, local_id),
        IrStmt::For { iter, .. } => expr_mentions_local(iter, local_id),
        IrStmt::Approve { args, .. } => args.iter().any(|arg| expr_mentions_local(arg, local_id)),
        IrStmt::Expr { expr, .. } => expr_mentions_local(expr, local_id),
        IrStmt::Dup {
            local_id: current, ..
        }
        | IrStmt::Drop {
            local_id: current, ..
        } => *current == local_id,
        IrStmt::Break { .. } | IrStmt::Continue { .. } | IrStmt::Pass { .. } => false,
    }
}

fn expr_mentions_local(expr: &IrExpr, local_id: LocalId) -> bool {
    match &expr.kind {
        IrExprKind::Literal(_) | IrExprKind::Decl { .. } | IrExprKind::OptionNone => false,
        IrExprKind::Local {
            local_id: current, ..
        } => *current == local_id,
        IrExprKind::Call { args, .. } | IrExprKind::List { items: args } => {
            args.iter().any(|arg| expr_mentions_local(arg, local_id))
        }
        IrExprKind::FieldAccess { target, .. }
        | IrExprKind::WeakNew { strong: target }
        | IrExprKind::WeakUpgrade { weak: target }
        | IrExprKind::ResultOk { inner: target }
        | IrExprKind::ResultErr { inner: target }
        | IrExprKind::OptionSome { inner: target }
        | IrExprKind::TryPropagate { inner: target }
        | IrExprKind::TryRetry { body: target, .. }
        | IrExprKind::UnOp { operand: target, .. } => expr_mentions_local(target, local_id),
        IrExprKind::Index { target, index }
        | IrExprKind::BinOp {
            left: target,
            right: index,
            ..
        } => expr_mentions_local(target, local_id) || expr_mentions_local(index, local_id),
    }
}

fn stmt_is_effect_barrier(stmt: &IrStmt) -> bool {
    match stmt {
        IrStmt::Let { value, .. } => !expr_is_effect_free(value),
        IrStmt::Expr { expr, .. } => !expr_is_effect_free(expr),
        IrStmt::Yield { value, .. } => !expr_is_effect_free(value),
        IrStmt::Return { .. }
        | IrStmt::If { .. }
        | IrStmt::For { .. }
        | IrStmt::Approve { .. }
        | IrStmt::Break { .. }
        | IrStmt::Continue { .. }
        | IrStmt::Dup { .. }
        | IrStmt::Drop { .. } => true,
        IrStmt::Pass { .. } => false,
    }
}

fn expr_is_effect_free(expr: &IrExpr) -> bool {
    match &expr.kind {
        IrExprKind::Literal(_) | IrExprKind::Local { .. } | IrExprKind::Decl { .. } => true,
        IrExprKind::UnOp { operand, .. } => expr_is_effect_free(operand),
        IrExprKind::BinOp { left, right, .. } => {
            expr_is_effect_free(left) && expr_is_effect_free(right)
        }
        IrExprKind::Call { .. }
        | IrExprKind::FieldAccess { .. }
        | IrExprKind::Index { .. }
        | IrExprKind::List { .. }
        | IrExprKind::WeakNew { .. }
        | IrExprKind::WeakUpgrade { .. }
        | IrExprKind::ResultOk { .. }
        | IrExprKind::ResultErr { .. }
        | IrExprKind::OptionSome { .. }
        | IrExprKind::OptionNone
        | IrExprKind::TryPropagate { .. }
        | IrExprKind::TryRetry { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_ast::{BinaryOp, Effect, Span};
    use corvid_ir::{IrCallKind, IrLiteral, IrParam};
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

    #[test]
    fn effect_free_let_is_not_barrier() {
        let stmt = IrStmt::Let {
            local_id: LocalId(1),
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
        };
        assert!(!stmt_is_effect_barrier(&stmt));
    }

    #[test]
    fn prompt_call_is_barrier() {
        let expr = IrExpr {
            kind: IrExprKind::Call {
                kind: IrCallKind::Prompt { def_id: DefId(0) },
                callee_name: "p".into(),
                args: vec![local_expr(0, Type::String)],
            },
            ty: Type::String,
            span: span(),
        };
        assert!(stmt_is_effect_barrier(&IrStmt::Expr { expr, span: span() }));
    }

    #[test]
    fn tool_call_is_barrier() {
        let expr = IrExpr {
            kind: IrExprKind::Call {
                kind: IrCallKind::Tool {
                    def_id: DefId(1),
                    effect: Effect::Safe,
                },
                callee_name: "tool".into(),
                args: vec![int_lit(1)],
            },
            ty: Type::String,
            span: span(),
        };
        assert!(stmt_is_effect_barrier(&IrStmt::Expr { expr, span: span() }));
    }
}
