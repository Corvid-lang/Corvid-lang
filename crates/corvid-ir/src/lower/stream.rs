use super::Lowerer;
use crate::types::{IrExpr, IrExprKind, IrLiteral, StreamMergePolicy};
use corvid_ast::{Expr, Ident, Literal, Span};
use corvid_resolve::{Binding, BuiltIn};
use corvid_types::Type;
use std::collections::HashMap;

pub(super) fn lower_merge_call(lowerer: &Lowerer<'_>, span: Span, args: &[Expr]) -> IrExprKind {
    let groups = args
        .first()
        .map(|arg| lowerer.lower_expr(arg))
        .unwrap_or_else(|| IrExpr {
            kind: IrExprKind::Literal(IrLiteral::Nothing),
            ty: Type::Unknown,
            span,
        });
    IrExprKind::StreamMerge {
        groups: Box::new(groups),
        policy: StreamMergePolicy::Fifo,
    }
}

pub(super) fn try_stream_builtin_call(
    lowerer: &Lowerer<'_>,
    target: &Expr,
    field: &Ident,
    args: &[Expr],
) -> Option<IrExprKind> {
    match field.name.as_str() {
        "split_by" if args.len() == 1 => {
            let key = string_literal(&args[0]).unwrap_or_default().to_string();
            Some(IrExprKind::StreamSplitBy {
                stream: Box::new(lowerer.lower_expr(target)),
                key,
            })
        }
        "ordered_by" if args.len() == 1 => {
            let policy = string_literal(&args[0])
                .and_then(StreamMergePolicy::from_name)
                .unwrap_or(StreamMergePolicy::Fifo);
            if let Expr::Call {
                callee: merge_callee,
                args: merge_args,
                ..
            } = target
            {
                if is_builtin_ident(merge_callee, BuiltIn::StreamMerge, lowerer.bindings)
                    && merge_args.len() == 1
                {
                    return Some(IrExprKind::StreamMerge {
                        groups: Box::new(lowerer.lower_expr(&merge_args[0])),
                        policy,
                    });
                }
            }
            Some(IrExprKind::StreamOrderedBy {
                stream: Box::new(lowerer.lower_expr(target)),
                policy,
            })
        }
        _ => None,
    }
}

fn string_literal(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Literal {
            value: Literal::String(value),
            ..
        } => Some(value),
        _ => None,
    }
}

fn is_builtin_ident(
    expr: &Expr,
    expected: BuiltIn,
    bindings: &HashMap<Span, Binding>,
) -> bool {
    match expr {
        Expr::Ident { name, .. } => {
            matches!(bindings.get(&name.span), Some(Binding::BuiltIn(actual)) if *actual == expected)
        }
        _ => false,
    }
}
