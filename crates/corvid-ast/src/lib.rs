//! Corvid AST types.
//!
//! This crate defines the shape of a parsed `.cor` file. The parser
//! produces values of these types; the type checker, IR lowering, and
//! code generator consume them.
//!
//! See `ARCHITECTURE.md` §5 for the high-level design.

#![allow(dead_code)]

pub mod decl;
pub mod effect;
pub mod expr;
pub mod span;
pub mod stmt;
pub mod ty;

pub use decl::*;
pub use effect::*;
pub use expr::*;
pub use span::*;
pub use stmt::*;
pub use ty::*;

#[cfg(test)]
mod tests {
    use super::*;

    /// Convenience: a dummy span for tests that don't care about locations.
    fn sp() -> Span {
        Span::new(0, 0)
    }

    fn id(name: &str) -> Ident {
        Ident::new(name, sp())
    }

    fn ty_named(name: &str) -> TypeRef {
        TypeRef::Named {
            name: id(name),
            span: sp(),
        }
    }

    /// Build the AST for `examples/refund_bot.cor` by hand. This proves the
    /// v0.1 AST can represent the entire canonical example.
    #[test]
    fn can_build_refund_bot_ast() {
        // tool get_order(id: String) -> Order
        let get_order = ToolDecl {
            name: id("get_order"),
            params: vec![Param {
                name: id("id"),
                ty: ty_named("String"),
                span: sp(),
            }],
            return_ty: ty_named("Order"),
            effect: Effect::Safe,
            effect_row: EffectRow::default(),
            span: sp(),
        };

        // tool issue_refund(id: String, amount: Float) -> Receipt dangerous
        let issue_refund = ToolDecl {
            name: id("issue_refund"),
            params: vec![
                Param {
                    name: id("id"),
                    ty: ty_named("String"),
                    span: sp(),
                },
                Param {
                    name: id("amount"),
                    ty: ty_named("Float"),
                    span: sp(),
                },
            ],
            return_ty: ty_named("Receipt"),
            effect: Effect::Dangerous,
            effect_row: EffectRow::default(),
            span: sp(),
        };

        // agent refund_bot(ticket: Ticket) -> Decision:
        //     order = get_order(ticket.order_id)
        //     decision = decide_refund(ticket, order)
        //     if decision.should_refund:
        //         approve IssueRefund(order.id, order.amount)
        //         issue_refund(order.id, order.amount)
        //     return decision

        let order_let = Stmt::Let {
            name: id("order"),
            ty: None,
            value: Expr::Call {
                callee: Box::new(Expr::Ident {
                    name: id("get_order"),
                    span: sp(),
                }),
                args: vec![Expr::FieldAccess {
                    target: Box::new(Expr::Ident {
                        name: id("ticket"),
                        span: sp(),
                    }),
                    field: id("order_id"),
                    span: sp(),
                }],
                span: sp(),
            },
            span: sp(),
        };

        let approve_stmt = Stmt::Approve {
            action: Expr::Call {
                callee: Box::new(Expr::Ident {
                    name: id("IssueRefund"),
                    span: sp(),
                }),
                args: vec![
                    Expr::FieldAccess {
                        target: Box::new(Expr::Ident {
                            name: id("order"),
                            span: sp(),
                        }),
                        field: id("id"),
                        span: sp(),
                    },
                    Expr::FieldAccess {
                        target: Box::new(Expr::Ident {
                            name: id("order"),
                            span: sp(),
                        }),
                        field: id("amount"),
                        span: sp(),
                    },
                ],
                span: sp(),
            },
            span: sp(),
        };

        let refund_bot = AgentDecl {
            name: id("refund_bot"),
            params: vec![Param {
                name: id("ticket"),
                ty: ty_named("Ticket"),
                span: sp(),
            }],
            return_ty: ty_named("Decision"),
            body: Block {
                stmts: vec![
                    order_let,
                    Stmt::If {
                        cond: Expr::FieldAccess {
                            target: Box::new(Expr::Ident {
                                name: id("decision"),
                                span: sp(),
                            }),
                            field: id("should_refund"),
                            span: sp(),
                        },
                        then_block: Block {
                            stmts: vec![approve_stmt],
                            span: sp(),
                        },
                        else_block: None,
                        span: sp(),
                    },
                ],
                span: sp(),
            },
            effect_row: EffectRow::default(),
            constraints: Vec::new(),
            attributes: Vec::new(),
            span: sp(),
        };

        let file = File {
            decls: vec![
                Decl::Tool(get_order),
                Decl::Tool(issue_refund),
                Decl::Agent(refund_bot),
            ],
            span: sp(),
        };

        assert_eq!(file.decls.len(), 3);
    }

    #[test]
    fn effect_variants_exist() {
        let _ = Effect::Safe;
        let _ = Effect::Dangerous;
    }

    #[test]
    fn binary_ops_roundtrip_through_serde() {
        let op = BinaryOp::And;
        let s = serde_json::to_string(&op).unwrap();
        let decoded: BinaryOp = serde_json::from_str(&s).unwrap();
        assert_eq!(op, decoded);
    }
}
