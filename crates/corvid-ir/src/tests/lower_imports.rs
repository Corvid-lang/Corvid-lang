use super::*;

    #[test]
    fn refund_bot_lowers_to_expected_ir_shape() {
        let src = r#"
import python "anthropic" as anthropic effects: network

type Ticket:
    order_id: String

type Order:
    id: String
    amount: Float

type Decision:
    should_refund: Bool

type Receipt:
    refund_id: String

tool get_order(id: String) -> Order
tool issue_refund(id: String, amount: Float) -> Receipt dangerous

prompt decide_refund(ticket: Ticket, order: Order) -> Decision:
    """Decide."""

agent refund_bot(ticket: Ticket) -> Decision:
    order = get_order(ticket.order_id)
    decision = decide_refund(ticket, order)

    if decision.should_refund:
        approve IssueRefund(order.id, order.amount)
        issue_refund(order.id, order.amount)

    return decision
"#;
        let ir = lower_src(src);
        assert_eq!(ir.imports.len(), 1);
        assert_eq!(ir.types.len(), 4);
        assert_eq!(ir.tools.len(), 2);
        assert_eq!(ir.prompts.len(), 1);
        assert_eq!(ir.agents.len(), 1);

        // Exactly one tool is dangerous.
        let dangerous_tools = ir
            .tools
            .iter()
            .filter(|t| matches!(t.effect, Effect::Dangerous))
            .count();
        assert_eq!(dangerous_tools, 1);

        // Agent body: let, let, if, return.
        let agent = &ir.agents[0];
        assert_eq!(agent.body.stmts.len(), 4);
        assert!(matches!(agent.body.stmts[0], IrStmt::Let { .. }));
        assert!(matches!(agent.body.stmts[1], IrStmt::Let { .. }));
        assert!(matches!(agent.body.stmts[2], IrStmt::If { .. }));
        assert!(matches!(agent.body.stmts[3], IrStmt::Return { .. }));

        // Inside the If: Approve then tool call Expr.
        if let IrStmt::If { then_block, .. } = &agent.body.stmts[2] {
            assert_eq!(then_block.stmts.len(), 2);
            match &then_block.stmts[0] {
                IrStmt::Approve { label, args, .. } => {
                    assert_eq!(label, "IssueRefund");
                    assert_eq!(args.len(), 2);
                }
                other => panic!("expected Approve, got {other:?}"),
            }
            assert!(matches!(then_block.stmts[1], IrStmt::Expr { .. }));
        }
    }

    #[test]
    fn corvid_import_hash_pin_survives_lowering() {
        let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let src = format!("import \"./policy\" hash:sha256:{digest} as p\n");
        let ir = lower_src(&src);
        assert_eq!(ir.imports.len(), 1);
        let pin = ir.imports[0].content_hash.as_ref().expect("hash pin");
        assert_eq!(pin.algorithm, "sha256");
        assert_eq!(pin.hex, digest);
    }
