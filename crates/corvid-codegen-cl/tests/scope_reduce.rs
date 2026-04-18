use corvid_ast::{BinaryOp, Effect, Span};
use corvid_codegen_cl::scope_reduce::{analyze_effects, reduce_scope};
use corvid_ir::{IrAgent, IrBlock, IrCallKind, IrExpr, IrExprKind, IrLiteral, IrParam, IrStmt};
use corvid_resolve::{DefId, LocalId};
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

fn test_agent(body: Vec<IrStmt>) -> IrAgent {
    IrAgent {
        id: DefId(0),
        name: "f".into(),
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

#[test]
fn drop_moves_across_effect_free_arithmetic() {
    let agent = test_agent(vec![
        IrStmt::Let {
            local_id: LocalId(1),
            name: "x".into(),
            ty: Type::String,
            value: local_expr(0, Type::String),
            span: span(),
        },
        IrStmt::Let {
            local_id: LocalId(2),
            name: "n".into(),
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
        IrStmt::Expr {
            expr: int_lit(0),
            span: span(),
        },
        IrStmt::Drop {
            local_id: LocalId(1),
            span: span(),
        },
    ]);

    let info = analyze_effects(&agent);
    let out = reduce_scope(agent, &info);
    assert!(matches!(out.body.stmts[1], IrStmt::Drop { local_id: LocalId(1), .. }));
}

#[test]
fn drop_does_not_move_across_tool_call() {
    let agent = test_agent(vec![
        IrStmt::Let {
            local_id: LocalId(1),
            name: "x".into(),
            ty: Type::String,
            value: local_expr(0, Type::String),
            span: span(),
        },
        IrStmt::Expr {
            expr: IrExpr {
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
            },
            span: span(),
        },
        IrStmt::Drop {
            local_id: LocalId(1),
            span: span(),
        },
    ]);

    let info = analyze_effects(&agent);
    let out = reduce_scope(agent, &info);
    assert!(matches!(out.body.stmts[2], IrStmt::Drop { local_id: LocalId(1), .. }));
}

#[test]
fn drop_does_not_move_across_approve() {
    let agent = test_agent(vec![
        IrStmt::Let {
            local_id: LocalId(1),
            name: "x".into(),
            ty: Type::String,
            value: local_expr(0, Type::String),
            span: span(),
        },
        IrStmt::Approve {
            label: "Refund".into(),
            args: vec![int_lit(1)],
            span: span(),
        },
        IrStmt::Drop {
            local_id: LocalId(1),
            span: span(),
        },
    ]);

    let info = analyze_effects(&agent);
    let out = reduce_scope(agent, &info);
    assert!(matches!(out.body.stmts[2], IrStmt::Drop { local_id: LocalId(1), .. }));
}
