//! Intermediate representation for the Corvid compiler.
//!
//! Post-typecheck, pre-codegen. Desugared, normalized, and carrying
//! resolved references plus attached types.
//!
//! See `ARCHITECTURE.md` §4.

#![allow(dead_code)]

pub mod lower;
pub mod types;

pub use lower::lower;
pub use types::*;

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_ast::Effect;
    use corvid_resolve::resolve;
    use corvid_syntax::{lex, parse_file};
    use corvid_types::typecheck;

    fn lower_src(src: &str) -> IrFile {
        let tokens = lex(src).expect("lex");
        let (file, perr) = parse_file(&tokens);
        assert!(perr.is_empty(), "parse: {perr:?}");
        let resolved = resolve(&file);
        assert!(resolved.errors.is_empty(), "resolve: {:?}", resolved.errors);
        let checked = typecheck(&file, &resolved);
        assert!(checked.errors.is_empty(), "typecheck: {:?}", checked.errors);
        lower(&file, &resolved, &checked)
    }

    #[test]
    fn lowers_simple_agent() {
        let src = "\
tool greet(name: String) -> String

agent hello(name: String) -> String:
    message = greet(name)
    return message
";
        let ir = lower_src(src);
        assert_eq!(ir.tools.len(), 1);
        assert_eq!(ir.agents.len(), 1);
        assert_eq!(ir.agents[0].body.stmts.len(), 2);
        assert!(matches!(ir.agents[0].body.stmts[0], IrStmt::Let { .. }));
        assert!(matches!(ir.agents[0].body.stmts[1], IrStmt::Return { .. }));
    }

    #[test]
    fn tool_effect_is_preserved_on_ir_tool() {
        let src = "\
tool send_email(to: String, body: String) -> Nothing dangerous

agent do_it(to: String) -> Nothing:
    approve SendEmail(to, to)
    return send_email(to, to)
";
        let ir = lower_src(src);
        assert_eq!(ir.tools.len(), 1);
        assert!(matches!(ir.tools[0].effect, Effect::Dangerous));
    }

    #[test]
    fn approve_is_structured_in_ir() {
        let src = "\
tool send_email(to: String, body: String) -> Nothing dangerous

agent do_it(to: String) -> Nothing:
    approve SendEmail(to, to)
    return send_email(to, to)
";
        let ir = lower_src(src);
        let agent = &ir.agents[0];
        match &agent.body.stmts[0] {
            IrStmt::Approve { label, args, .. } => {
                assert_eq!(label, "SendEmail");
                assert_eq!(args.len(), 2);
            }
            other => panic!("expected Approve, got {other:?}"),
        }
    }

    #[test]
    fn break_continue_pass_lower_to_dedicated_variants() {
        // Slice 12h typechecker: the loop var `x` gets a real type
        // (String, for String iteration), so `if x:` is now a Bool
        // mismatch. Use an explicit comparison.
        let src = "\
agent loop_stuff(xs: String) -> String:
    for x in xs:
        if x == \"a\":
            break
        continue
    pass
    return xs
";
        let ir = lower_src(src);
        let agent = &ir.agents[0];
        // First stmt: a For loop containing Break and Continue.
        match &agent.body.stmts[0] {
            IrStmt::For { body, .. } => {
                // body has: if-stmt (containing Break), continue
                match &body.stmts[0] {
                    IrStmt::If { then_block, .. } => {
                        assert!(matches!(then_block.stmts[0], IrStmt::Break { .. }));
                    }
                    other => panic!("expected If, got {other:?}"),
                }
                assert!(matches!(body.stmts[1], IrStmt::Continue { .. }));
            }
            other => panic!("expected For, got {other:?}"),
        }
        // Second stmt at top level: Pass.
        assert!(matches!(agent.body.stmts[1], IrStmt::Pass { .. }));
    }

    #[test]
    fn tool_call_ir_identifies_the_tool() {
        let src = "\
tool get_order(id: String) -> Order
type Order:
    id: String

agent a(id: String) -> Order:
    return get_order(id)
";
        let ir = lower_src(src);
        let agent = &ir.agents[0];
        match &agent.body.stmts[0] {
            IrStmt::Return { value: Some(e), .. } => match &e.kind {
                IrExprKind::Call { kind, callee_name, .. } => {
                    assert_eq!(callee_name, "get_order");
                    assert!(matches!(kind, IrCallKind::Tool { .. }));
                }
                other => panic!("expected Call, got {other:?}"),
            },
            other => panic!("expected Return, got {other:?}"),
        }
    }

    #[test]
    fn refund_bot_lowers_to_expected_ir_shape() {
        let src = r#"
import python "anthropic" as anthropic

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
}
