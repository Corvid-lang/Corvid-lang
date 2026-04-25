//! Intermediate representation for the Corvid compiler.
//!
//! Post-typecheck, pre-codegen. Desugared, normalized, and carrying
//! resolved references plus attached types.
//!
//! See `ARCHITECTURE.md` §4.

#![allow(dead_code)]

mod imports;
pub mod lower;
pub mod types;

pub use lower::{lower, lower_with_modules};
pub use types::*;

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_ast::{Backoff, BackpressurePolicy, Effect};
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
        // The typechecker gives loop var `x` a real type
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

    #[test]
    fn lowers_grounded_type_refs_to_ir_grounded_types() {
        let src = "\
effect retrieval:
    data: grounded

tool grounded_echo(name: String) -> Grounded<String> uses retrieval

pub extern \"c\"
agent grounded_lookup(name: String) -> Grounded<String>:
    return grounded_echo(name)
";
        let ir = lower_src(src);
        assert!(matches!(
            &ir.tools[0].return_ty,
            corvid_types::Type::Grounded(inner) if matches!(&**inner, corvid_types::Type::String)
        ));
        assert!(matches!(
            &ir.agents[0].return_ty,
            corvid_types::Type::Grounded(inner) if matches!(&**inner, corvid_types::Type::String)
        ));
    }

    #[test]
    fn lowers_result_option_constructors_and_none() {
        let src = "\
agent build(flag: Bool) -> Result<Option<String>, String>:
    if flag:
        return Ok(Some(\"hi\"))
    return Ok(None)
";
        let ir = lower_src(src);
        let agent = &ir.agents[0];
        match &agent.body.stmts[0] {
            IrStmt::If {
                then_block,
                else_block: None,
                ..
            } => match &then_block.stmts[0] {
                IrStmt::Return { value: Some(expr), .. } => match &expr.kind {
                    IrExprKind::ResultOk { inner } => match &inner.kind {
                        IrExprKind::OptionSome { .. } => {}
                        other => panic!("expected OptionSome, got {other:?}"),
                    },
                    other => panic!("expected ResultOk, got {other:?}"),
                },
                other => panic!("expected Return, got {other:?}"),
            },
            other => panic!("expected If, got {other:?}"),
        }
        match &agent.body.stmts[1] {
            IrStmt::Return { value: Some(expr), .. } => match &expr.kind {
                IrExprKind::ResultOk { inner } => {
                    assert!(matches!(inner.kind, IrExprKind::OptionNone));
                }
                other => panic!("expected ResultOk, got {other:?}"),
            },
            other => panic!("expected Return, got {other:?}"),
        }
    }

    #[test]
    fn lowers_try_propagate_and_retry() {
        let src = "\
tool fetch(id: String) -> Result<String, String>
tool lookup(id: String) -> Result<String, String>

agent load(id: String) -> Result<String, String>:
    value = fetch(id)?
    stable = try lookup(id) on error retry 3 times backoff exponential 40
    joined = stable?
    return Ok(value + joined)
";
        let ir = lower_src(src);
        let agent = &ir.agents[0];
        match &agent.body.stmts[0] {
            IrStmt::Let { value, .. } => {
                assert!(matches!(value.kind, IrExprKind::TryPropagate { .. }));
            }
            other => panic!("expected Let, got {other:?}"),
        }
        match &agent.body.stmts[1] {
            IrStmt::Let { value, .. } => match &value.kind {
                IrExprKind::TryRetry {
                    attempts,
                    backoff,
                    ..
                } => {
                    assert_eq!(*attempts, 3);
                    assert_eq!(*backoff, Backoff::Exponential(40));
                }
                other => panic!("expected TryRetry, got {other:?}"),
            },
            other => panic!("expected Let, got {other:?}"),
        }
        match &agent.body.stmts[2] {
            IrStmt::Let { value, .. } => {
                assert!(matches!(value.kind, IrExprKind::TryPropagate { .. }));
            }
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn lowers_evals_into_ir_nodes() {
        let src = "\
tool get_order(id: String) -> String
tool issue_refund(id: String) -> String dangerous

eval refund_process:
    order = get_order(\"ord_42\")
    assert called get_order before issue_refund
    assert approved IssueRefund
    assert cost < $0.50
    assert order == order with confidence 0.95 over 50 runs
";
        let ir = lower_src(src);
        assert_eq!(ir.evals.len(), 1);
        let eval = &ir.evals[0];
        assert_eq!(eval.name, "refund_process");
        assert_eq!(eval.body.stmts.len(), 1);
        assert_eq!(eval.assertions.len(), 4);
        assert!(matches!(eval.assertions[0], IrEvalAssert::Ordering { .. }));
        assert!(matches!(eval.assertions[1], IrEvalAssert::Approved { .. }));
        assert!(matches!(eval.assertions[2], IrEvalAssert::Cost { .. }));
        match &eval.assertions[3] {
            IrEvalAssert::Value {
                confidence, runs, ..
            } => {
                assert_eq!(*confidence, Some(0.95));
                assert_eq!(*runs, Some(50));
            }
            other => panic!("expected value assertion, got {other:?}"),
        }
    }

    #[test]
    fn lowers_yield_to_dedicated_ir_stmt() {
        let src = "\
agent chunks(text: String) -> Stream<String>:
    yield text
";
        let ir = lower_src(src);
        let agent = &ir.agents[0];
        match &agent.body.stmts[0] {
            IrStmt::Yield { value, .. } => {
                assert_eq!(value.ty.display_name(), "String");
            }
            other => panic!("expected Yield, got {other:?}"),
        }
    }

    #[test]
    fn lowers_prompt_stream_metadata() {
        let src = "\
model expert:
    capability: expert

prompt generate(ctx: String) -> Stream<String>:
    with min_confidence 0.80
    with max_tokens 5000
    with backpressure bounded(100)
    with escalate_to expert
    \"Generate {ctx}\"
";
        let ir = lower_src(src);
        let prompt = &ir.prompts[0];
        assert_eq!(prompt.min_confidence, Some(0.80));
        assert_eq!(prompt.max_tokens, Some(5000));
        assert_eq!(
            prompt.backpressure,
            Some(BackpressurePolicy::Bounded(100))
        );
        assert_eq!(prompt.escalate_to.as_deref(), Some("expert"));
    }

    #[test]
    fn lowers_stream_partial_prompt_return_type() {
        let src = "\
type Plan:
    title: String
    body: String

prompt plan(topic: String) -> Stream<Partial<Plan>>:
    \"Plan {topic}\"
";
        let ir = lower_src(src);
        let prompt = &ir.prompts[0];
        match &prompt.return_ty {
            corvid_types::Type::Stream(inner) => match &**inner {
                corvid_types::Type::Partial(partial_inner) => {
                    assert!(matches!(&**partial_inner, corvid_types::Type::Struct(_)));
                }
                other => panic!("expected Partial<T>, got {other:?}"),
            },
            other => panic!("expected Stream<T>, got {other:?}"),
        }
    }

    #[test]
    fn lowers_calibrated_prompt_modifier() {
        let src = "\
prompt classify(ctx: String) -> String:
    calibrated
    \"Classify {ctx}.\"
";
        let ir = lower_src(src);
        assert!(ir.prompts[0].calibrated);
    }

    #[test]
    fn lowers_prompt_cites_strictly_param_index() {
        let src = "\
prompt answer(question: String, ctx: Grounded<String>) -> Grounded<String>:
    cites ctx strictly
    \"Answer from {ctx}\"
";
        let ir = lower_src(src);
        let prompt = &ir.prompts[0];
        assert_eq!(prompt.cites_strictly_param, Some(1));
    }

    #[test]
    fn grounded_unwrap_lowers_to_explicit_ir_node() {
        let src = "\
effect retrieval:
    data: grounded

tool fetch_doc(id: String) -> Grounded<String> uses retrieval

agent load(id: String) -> String:
    doc = fetch_doc(id)
    return doc.unwrap_discarding_sources()
";
        let ir = lower_src(src);
        let agent = ir.agents.iter().find(|a| a.name == "load").unwrap();
        let ret = agent
            .body
            .stmts
            .iter()
            .find_map(|stmt| match stmt {
                IrStmt::Return { value: Some(value), .. } => Some(value),
                _ => None,
            })
            .expect("return value");
        assert!(
            matches!(ret.kind, IrExprKind::UnwrapGrounded { .. }),
            "expected UnwrapGrounded, got {:?}",
            ret.kind
        );
        assert_eq!(ret.ty, corvid_types::Type::String);
    }

    #[test]
    fn wrapping_agent_lowers_integer_arithmetic_to_explicit_nodes() {
        let src = "\
@wrapping
agent hash_step(n: Int) -> Int:
    return -(n * 1099511628211)
";
        let ir = lower_src(src);
        let agent = ir.agents.iter().find(|a| a.name == "hash_step").unwrap();
        assert!(agent.wrapping_arithmetic);
        let ret = agent
            .body
            .stmts
            .iter()
            .find_map(|stmt| match stmt {
                IrStmt::Return { value: Some(value), .. } => Some(value),
                _ => None,
            })
            .expect("return value");
        match &ret.kind {
            IrExprKind::WrappingUnOp { operand, .. } => {
                assert!(matches!(operand.kind, IrExprKind::WrappingBinOp { .. }));
            }
            other => panic!("expected WrappingUnOp, got {other:?}"),
        }
    }

    // ============================================================
    // Replay IR lowering (21-inv-E-4)
    // ============================================================

    /// Reach into the agent's body to find the `IrExprKind::Replay`
    /// the tests below construct.
    fn find_replay<'a>(ir: &'a IrFile) -> &'a IrExprKind {
        for agent in &ir.agents {
            if let Some(kind) = find_replay_in_block(&agent.body) {
                return kind;
            }
        }
        panic!("no IrExprKind::Replay found in IR");
    }

    fn find_replay_in_block<'a>(block: &'a IrBlock) -> Option<&'a IrExprKind> {
        for stmt in &block.stmts {
            if let Some(kind) = find_replay_in_stmt(stmt) {
                return Some(kind);
            }
        }
        None
    }

    fn find_replay_in_stmt<'a>(stmt: &'a IrStmt) -> Option<&'a IrExprKind> {
        match stmt {
            IrStmt::Let { value, .. }
            | IrStmt::Expr { expr: value, .. }
            | IrStmt::Yield { value, .. } => find_replay_in_expr(value),
            IrStmt::Approve { args, .. } => args.iter().find_map(find_replay_in_expr),
            IrStmt::Return { value: Some(value), .. } => find_replay_in_expr(value),
            IrStmt::Return { value: None, .. } => None,
            IrStmt::If { cond, then_block, else_block, .. } => {
                find_replay_in_expr(cond)
                    .or_else(|| find_replay_in_block(then_block))
                    .or_else(|| else_block.as_ref().and_then(find_replay_in_block))
            }
            IrStmt::For { iter, body, .. } => {
                find_replay_in_expr(iter).or_else(|| find_replay_in_block(body))
            }
            IrStmt::Break { .. }
            | IrStmt::Continue { .. }
            | IrStmt::Pass { .. }
            | IrStmt::Dup { .. }
            | IrStmt::Drop { .. } => None,
        }
    }

    fn find_replay_in_expr<'a>(expr: &'a IrExpr) -> Option<&'a IrExprKind> {
        if matches!(expr.kind, IrExprKind::Replay { .. }) {
            return Some(&expr.kind);
        }
        match &expr.kind {
            IrExprKind::Call { args, .. } => {
                args.iter().find_map(find_replay_in_expr)
            }
            IrExprKind::FieldAccess { target, .. }
            | IrExprKind::UnwrapGrounded { value: target }
            | IrExprKind::WeakNew { strong: target }
            | IrExprKind::WeakUpgrade { weak: target }
            | IrExprKind::ResultOk { inner: target }
            | IrExprKind::ResultErr { inner: target }
            | IrExprKind::OptionSome { inner: target }
            | IrExprKind::TryPropagate { inner: target }
            | IrExprKind::TryRetry { body: target, .. }
            | IrExprKind::UnOp { operand: target, .. }
            | IrExprKind::WrappingUnOp { operand: target, .. } => find_replay_in_expr(target),
            IrExprKind::Index { target, index }
            | IrExprKind::BinOp { left: target, right: index, .. }
            | IrExprKind::WrappingBinOp { left: target, right: index, .. } => {
                find_replay_in_expr(target).or_else(|| find_replay_in_expr(index))
            }
            IrExprKind::List { items } => items.iter().find_map(find_replay_in_expr),
            _ => None,
        }
    }

    const REPLAY_PRELUDE: &str = r#"
type Decision:
    label: String

type Order:
    id: String

prompt classify(x: String) -> Decision:
    """Classify."""

tool get_order(id: String) -> Order

tool issue_refund(id: String, amount: Float) -> Order dangerous
"#;

    fn lower_replay(body: &str) -> IrFile {
        let src = format!("{REPLAY_PRELUDE}\n{body}");
        lower_src(&src)
    }

    #[test]
    fn replay_lowers_to_structured_ir_node_not_just_else_body() {
        let body = r#"
agent run(x: String) -> Decision:
    return replay "t.jsonl":
        when llm("classify") -> Decision("fixture")
        else Decision("unknown")
"#;
        let ir = lower_replay(body);
        let kind = find_replay(&ir);
        let (arms_len, has_trace) = match kind {
            IrExprKind::Replay { trace, arms, .. } => {
                let has_trace = matches!(
                    trace.kind,
                    IrExprKind::Literal(IrLiteral::String(_))
                );
                (arms.len(), has_trace)
            }
            other => panic!("expected Replay, got {other:?}"),
        };
        assert_eq!(arms_len, 1, "expected exactly one `when` arm");
        assert!(has_trace, "trace subexpression should lower to a String literal");
    }

    #[test]
    fn replay_arm_pattern_lowers_with_resolved_name() {
        let body = r#"
agent run(x: String) -> Decision:
    return replay "t.jsonl":
        when llm("classify") -> Decision("fixture")
        else Decision("unknown")
"#;
        let ir = lower_replay(body);
        let IrExprKind::Replay { arms, .. } = find_replay(&ir) else {
            unreachable!();
        };
        match &arms[0].pattern {
            IrReplayPattern::Llm { prompt, .. } => {
                assert_eq!(prompt, "classify");
            }
            other => panic!("expected Llm pattern, got {other:?}"),
        }
    }

    #[test]
    fn replay_arm_tool_pattern_lowers_wildcard_arg() {
        let body = r#"
agent run(x: String) -> Order:
    return replay "t.jsonl":
        when tool("get_order", _) -> Order("fixture")
        else Order("unknown")
"#;
        let ir = lower_replay(body);
        let IrExprKind::Replay { arms, .. } = find_replay(&ir) else {
            unreachable!();
        };
        match &arms[0].pattern {
            IrReplayPattern::Tool { tool, arg, .. } => {
                assert_eq!(tool, "get_order");
                assert!(
                    matches!(arg, IrReplayToolArgPattern::Wildcard),
                    "expected Wildcard, got {arg:?}"
                );
            }
            other => panic!("expected Tool pattern, got {other:?}"),
        }
    }

    #[test]
    fn replay_arm_tool_pattern_lowers_string_literal_arg() {
        let body = r#"
agent run(x: String) -> Order:
    return replay "t.jsonl":
        when tool("get_order", "ticket-42") -> Order("fixture")
        else Order("unknown")
"#;
        let ir = lower_replay(body);
        let IrExprKind::Replay { arms, .. } = find_replay(&ir) else {
            unreachable!();
        };
        match &arms[0].pattern {
            IrReplayPattern::Tool { arg, .. } => match arg {
                IrReplayToolArgPattern::StringLit(value) => {
                    assert_eq!(value, "ticket-42");
                }
                other => panic!("expected StringLit arg, got {other:?}"),
            },
            other => panic!("expected Tool pattern, got {other:?}"),
        }
    }

    #[test]
    fn replay_arm_tool_pattern_lowers_identifier_capture_with_local_id() {
        let body = r#"
agent run(x: String) -> Order:
    return replay "t.jsonl":
        when tool("get_order", ticket_id) -> get_order(ticket_id)
        else get_order(x)
"#;
        let ir = lower_replay(body);
        let IrExprKind::Replay { arms, .. } = find_replay(&ir) else {
            unreachable!();
        };
        let arg_capture = match &arms[0].pattern {
            IrReplayPattern::Tool { arg, .. } => match arg {
                IrReplayToolArgPattern::Capture(c) => c,
                other => panic!("expected Capture arg, got {other:?}"),
            },
            other => panic!("expected Tool pattern, got {other:?}"),
        };
        assert_eq!(arg_capture.name, "ticket_id");
        // LocalId should not be the unresolved-name sentinel.
        assert_ne!(arg_capture.local_id.0, u32::MAX);

        // And the arm body should reference that same LocalId.
        let body_mentions_capture = expr_mentions_local_id(&arms[0].body, arg_capture.local_id);
        assert!(
            body_mentions_capture,
            "arm body should read the captured LocalId",
        );
    }

    fn expr_mentions_local_id(expr: &IrExpr, target: corvid_resolve::LocalId) -> bool {
        match &expr.kind {
            IrExprKind::Local { local_id, .. } => *local_id == target,
            IrExprKind::Call { args, .. } => args.iter().any(|a| expr_mentions_local_id(a, target)),
            IrExprKind::FieldAccess { target: t, .. } => expr_mentions_local_id(t, target),
            IrExprKind::UnwrapGrounded { value } => expr_mentions_local_id(value, target),
            IrExprKind::Index { target: t, index } => {
                expr_mentions_local_id(t, target) || expr_mentions_local_id(index, target)
            }
            IrExprKind::BinOp { left, right, .. } => {
                expr_mentions_local_id(left, target) || expr_mentions_local_id(right, target)
            }
            IrExprKind::WrappingBinOp { left, right, .. } => {
                expr_mentions_local_id(left, target) || expr_mentions_local_id(right, target)
            }
            IrExprKind::UnOp { operand, .. }
            | IrExprKind::WrappingUnOp { operand, .. } => expr_mentions_local_id(operand, target),
            IrExprKind::List { items } => items.iter().any(|i| expr_mentions_local_id(i, target)),
            IrExprKind::WeakNew { strong }
            | IrExprKind::WeakUpgrade { weak: strong } => expr_mentions_local_id(strong, target),
            IrExprKind::ResultOk { inner }
            | IrExprKind::ResultErr { inner }
            | IrExprKind::OptionSome { inner }
            | IrExprKind::TryPropagate { inner } => expr_mentions_local_id(inner, target),
            IrExprKind::TryRetry { body, .. } => expr_mentions_local_id(body, target),
            IrExprKind::Replay { trace, arms, else_body } => {
                expr_mentions_local_id(trace, target)
                    || arms.iter().any(|a| expr_mentions_local_id(&a.body, target))
                    || expr_mentions_local_id(else_body, target)
            }
            _ => false,
        }
    }

    #[test]
    fn replay_arm_whole_event_as_capture_lowers_with_local_id() {
        let body = r#"
agent run(x: String) -> Decision:
    return replay "t.jsonl":
        when llm("classify") as recorded -> recorded
        else Decision("unknown")
"#;
        let ir = lower_replay(body);
        let IrExprKind::Replay { arms, .. } = find_replay(&ir) else {
            unreachable!();
        };
        let capture = arms[0]
            .capture
            .as_ref()
            .expect("expected whole-event `as recorded` capture");
        assert_eq!(capture.name, "recorded");
        assert_ne!(capture.local_id.0, u32::MAX);

        // Arm body references the capture.
        let body_refs = expr_mentions_local_id(&arms[0].body, capture.local_id);
        assert!(body_refs, "arm body should read the capture LocalId");
    }

    #[test]
    fn replay_approve_pattern_lowers_with_label() {
        let body = r#"
agent run(id: String, amount: Float) -> Order:
    approve IssueRefund(id, amount)
    return replay "t.jsonl":
        when approve("IssueRefund") -> get_order(id)
        else get_order(id)
"#;
        let ir = lower_replay(body);
        let IrExprKind::Replay { arms, .. } = find_replay(&ir) else {
            unreachable!();
        };
        match &arms[0].pattern {
            IrReplayPattern::Approve { label, .. } => {
                assert_eq!(label, "IssueRefund");
            }
            other => panic!("expected Approve pattern, got {other:?}"),
        }
    }

    #[test]
    fn replay_preserves_arm_order_in_ir() {
        let body = r#"
agent run(x: String) -> Decision:
    return replay "t.jsonl":
        when llm("classify") -> Decision("a")
        when llm("classify") -> Decision("b")
        when llm("classify") -> Decision("c")
        else Decision("d")
"#;
        let ir = lower_replay(body);
        let IrExprKind::Replay { arms, else_body, .. } = find_replay(&ir) else {
            unreachable!();
        };
        assert_eq!(arms.len(), 3);
        // Else body is present and non-empty.
        assert!(!matches!(
            else_body.kind,
            IrExprKind::Literal(IrLiteral::Nothing)
        ));
    }

    #[test]
    fn replay_else_body_is_separate_from_arms() {
        // Regression: the pre-E-4 stub lowered the whole replay to
        // just else_body. Now arms must be distinct from else_body.
        let body = r#"
agent run(x: String) -> Decision:
    return replay "t.jsonl":
        when llm("classify") -> Decision("arm")
        else Decision("else")
"#;
        let ir = lower_replay(body);
        let IrExprKind::Replay { arms, else_body, .. } = find_replay(&ir) else {
            unreachable!();
        };
        assert_eq!(arms.len(), 1);
        // The else body should literally be `Decision("else")` — a Call node,
        // distinct from the arm body which is `Decision("arm")`.
        assert!(matches!(
            else_body.kind,
            IrExprKind::Call { .. }
        ));
    }
}
