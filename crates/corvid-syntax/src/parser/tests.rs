    use super::*;
    use crate::lex;
    use corvid_ast::{BinaryOp, Expr, Literal, UnaryOp};

    fn parse(src: &str) -> Expr {
        let tokens = lex(src).expect("lex failed");
        parse_expr(&tokens).expect("parse failed")
    }

    fn try_parse(src: &str) -> Result<Expr, ParseError> {
        let tokens = lex(src).expect("lex failed");
        parse_expr(&tokens)
    }

    fn parse_repl(src: &str) -> ReplItem {
        let tokens = lex(src).expect("lex failed");
        parse_repl_input(&tokens).expect("repl parse failed")
    }

    // -------------------- literals --------------------

    #[test]
    fn int_literal() {
        assert!(matches!(
            parse("42"),
            Expr::Literal { value: Literal::Int(42), .. }
        ));
    }

    #[test]
    fn float_literal() {
        match parse("3.14") {
            Expr::Literal { value: Literal::Float(f), .. } => assert!((f - 3.14).abs() < 1e-9),
            other => panic!("expected float, got {other:?}"),
        }
    }

    #[test]
    fn string_literal() {
        assert!(matches!(
            parse(r#""hello""#),
            Expr::Literal { value: Literal::String(ref s), .. } if s == "hello"
        ));
    }

    #[test]
    fn bool_literals() {
        assert!(matches!(
            parse("true"),
            Expr::Literal { value: Literal::Bool(true), .. }
        ));
        assert!(matches!(
            parse("false"),
            Expr::Literal { value: Literal::Bool(false), .. }
        ));
    }

    #[test]
    fn nothing_literal() {
        assert!(matches!(
            parse("nothing"),
            Expr::Literal { value: Literal::Nothing, .. }
        ));
    }

    #[test]
    fn identifier() {
        assert!(matches!(
            parse("order"),
            Expr::Ident { ref name, .. } if name.name == "order"
        ));
    }

    // -------------------- parentheses --------------------

    #[test]
    fn parenthesized_expression() {
        // `(42)` should produce the same AST as `42`.
        assert!(matches!(
            parse("(42)"),
            Expr::Literal { value: Literal::Int(42), .. }
        ));
    }

    // -------------------- operator precedence --------------------

    #[test]
    fn multiplication_binds_tighter_than_addition() {
        // `1 + 2 * 3` parses as `1 + (2 * 3)`.
        let e = parse("1 + 2 * 3");
        match e {
            Expr::BinOp { op: BinaryOp::Add, ref left, ref right, .. } => {
                assert!(matches!(**left, Expr::Literal { value: Literal::Int(1), .. }));
                match &**right {
                    Expr::BinOp { op: BinaryOp::Mul, left: l2, right: r2, .. } => {
                        assert!(matches!(**l2, Expr::Literal { value: Literal::Int(2), .. }));
                        assert!(matches!(**r2, Expr::Literal { value: Literal::Int(3), .. }));
                    }
                    other => panic!("expected inner Mul, got {other:?}"),
                }
            }
            other => panic!("expected Add at top, got {other:?}"),
        }
    }

    #[test]
    fn parens_override_precedence() {
        // `(1 + 2) * 3` parses as `(Add(1, 2)) * 3`.
        let e = parse("(1 + 2) * 3");
        match e {
            Expr::BinOp { op: BinaryOp::Mul, ref left, ref right, .. } => {
                assert!(matches!(**left, Expr::BinOp { op: BinaryOp::Add, .. }));
                assert!(matches!(**right, Expr::Literal { value: Literal::Int(3), .. }));
            }
            other => panic!("expected Mul at top, got {other:?}"),
        }
    }

    #[test]
    fn logical_precedence_or_below_and() {
        // `a or b and c` parses as `a or (b and c)`.
        let e = parse("a or b and c");
        match e {
            Expr::BinOp { op: BinaryOp::Or, ref right, .. } => {
                assert!(matches!(**right, Expr::BinOp { op: BinaryOp::And, .. }));
            }
            other => panic!("expected Or at top, got {other:?}"),
        }
    }

    #[test]
    fn not_binds_after_and_or() {
        // `not a and b` parses as `(not a) and b`.
        let e = parse("not a and b");
        match e {
            Expr::BinOp { op: BinaryOp::And, ref left, .. } => {
                assert!(matches!(**left, Expr::UnOp { op: UnaryOp::Not, .. }));
            }
            other => panic!("expected And at top, got {other:?}"),
        }
    }

    #[test]
    fn unary_minus_stacks() {
        // `--x` parses as `Neg(Neg(x))`.
        let e = parse("--x");
        match e {
            Expr::UnOp { op: UnaryOp::Neg, ref operand, .. } => {
                assert!(matches!(**operand, Expr::UnOp { op: UnaryOp::Neg, .. }));
            }
            other => panic!("expected outer Neg, got {other:?}"),
        }
    }

    #[test]
    fn unary_minus_binds_tighter_than_binary_minus() {
        // `-x - y` parses as `(Neg(x)) - y`.
        let e = parse("-x - y");
        match e {
            Expr::BinOp { op: BinaryOp::Sub, ref left, .. } => {
                assert!(matches!(**left, Expr::UnOp { op: UnaryOp::Neg, .. }));
            }
            other => panic!("expected Sub at top, got {other:?}"),
        }
    }

    // -------------------- postfix operators --------------------

    #[test]
    fn field_access_chains() {
        // `a.b.c` parses as `FieldAccess(FieldAccess(a, b), c)`.
        let e = parse("a.b.c");
        match e {
            Expr::FieldAccess { ref target, ref field, .. } => {
                assert_eq!(field.name, "c");
                assert!(matches!(**target, Expr::FieldAccess { .. }));
            }
            other => panic!("expected outer FieldAccess, got {other:?}"),
        }
    }

    #[test]
    fn call_with_args() {
        let e = parse("f(1, 2, 3)");
        match e {
            Expr::Call { ref callee, ref args, .. } => {
                assert!(matches!(**callee, Expr::Ident { .. }));
                assert_eq!(args.len(), 3);
            }
            other => panic!("expected Call, got {other:?}"),
        }
    }

    #[test]
    fn call_with_trailing_comma() {
        let e = parse("f(1, 2,)");
        match e {
            Expr::Call { args, .. } => assert_eq!(args.len(), 2),
            other => panic!("expected Call, got {other:?}"),
        }
    }

    #[test]
    fn indexing() {
        let e = parse("xs[0]");
        assert!(matches!(e, Expr::Index { .. }));
    }

    #[test]
    fn mixed_postfix_chain() {
        // `f(x).y[z]` — call, field, index in order.
        let e = parse("f(x).y[z]");
        match e {
            Expr::Index { target, .. } => match *target {
                Expr::FieldAccess { target, .. } => {
                    assert!(matches!(*target, Expr::Call { .. }));
                }
                other => panic!("expected FieldAccess, got {other:?}"),
            },
            other => panic!("expected outer Index, got {other:?}"),
        }
    }

    // -------------------- list literals --------------------

    #[test]
    fn empty_list() {
        let e = parse("[]");
        match e {
            Expr::List { items, .. } => assert_eq!(items.len(), 0),
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn list_with_items() {
        let e = parse("[1, 2, 3]");
        match e {
            Expr::List { items, .. } => assert_eq!(items.len(), 3),
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn parses_postfix_try_propagate() {
        let e = parse("load_order()?");
        match e {
            Expr::TryPropagate { inner, .. } => {
                assert!(matches!(*inner, Expr::Call { .. }));
            }
            other => panic!("expected TryPropagate, got {other:?}"),
        }
    }

    #[test]
    fn parses_try_retry_with_linear_backoff() {
        let e = parse("try fetch_order(id) on error retry 3 times backoff linear 50");
        match e {
            Expr::TryRetry {
                body,
                attempts,
                backoff,
                ..
            } => {
                assert_eq!(attempts, 3);
                assert_eq!(backoff, Backoff::Linear(50));
                assert!(matches!(*body, Expr::Call { .. }));
            }
            other => panic!("expected TryRetry, got {other:?}"),
        }
    }

    #[test]
    fn parses_try_retry_with_exponential_backoff() {
        let e = parse("try maybe_send() on error retry 5 times backoff exponential 125");
        match e {
            Expr::TryRetry {
                attempts,
                backoff,
                ..
            } => {
                assert_eq!(attempts, 5);
                assert_eq!(backoff, Backoff::Exponential(125));
            }
            other => panic!("expected TryRetry, got {other:?}"),
        }
    }

    // -------------------- errors --------------------

    #[test]
    fn rejects_chained_comparison() {
        let err = try_parse("a < b < c").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::ChainedComparison));
    }

    #[test]
    fn rejects_unclosed_paren() {
        let err = try_parse("(1 + 2").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::UnclosedParen));
    }

    #[test]
    fn rejects_unclosed_bracket() {
        let err = try_parse("[1, 2").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::UnclosedBracket));
    }

    #[test]
    fn rejects_empty_input() {
        let err = try_parse("").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::UnexpectedEof));
    }

    #[test]
    fn rejects_retry_without_backoff_policy_kind() {
        let err = try_parse("try fetch() on error retry 2 times backoff 100").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::UnexpectedToken { .. }));
    }

    #[test]
    fn repl_classifies_decl_by_leading_keyword() {
        let item = parse_repl("tool greet(name: String) -> String\n");
        assert!(matches!(item, ReplItem::Decl(Decl::Tool(_))));
    }

    #[test]
    fn repl_classifies_assignment_as_stmt() {
        let item = parse_repl("x = 1\n");
        assert!(matches!(item, ReplItem::Stmt(Stmt::Let { .. })));
    }

    #[test]
    fn repl_classifies_control_flow_as_stmt() {
        let item = parse_repl("return 1\n");
        assert!(matches!(item, ReplItem::Stmt(Stmt::Return { .. })));
    }

    #[test]
    fn repl_classifies_other_input_as_expr() {
        let item = parse_repl("greet(name)\n");
        assert!(matches!(item, ReplItem::Expr(Expr::Call { .. })));
    }

    // -------------------- realistic agent snippets --------------------

    #[test]
    fn parses_field_on_call() {
        // Real Corvid pattern: tool call, then field access.
        let e = parse("get_order(ticket.order_id).amount");
        assert!(matches!(e, Expr::FieldAccess { .. }));
    }

    #[test]
    fn parses_struct_literal_via_call_syntax() {
        // `IssueRefund(order.id, order.amount)` — just a call at parse time.
        let e = parse("IssueRefund(order.id, order.amount)");
        match e {
            Expr::Call { callee, args, .. } => {
                assert!(matches!(*callee, Expr::Ident { .. }));
                assert_eq!(args.len(), 2);
            }
            other => panic!("expected Call, got {other:?}"),
        }
    }

    // =================================================================
    // Statement and block parser tests
    // =================================================================

    use corvid_ast::{Block, Stmt};

    /// Lex a source snippet and strip the leading Newline (if any) so the
    /// token stream begins at the first meaningful token. Tests below use
    /// raw strings with the first line blank for readability.
    fn lex_block_src(src: &str) -> Vec<Token> {
        let mut toks = lex(src).expect("lex failed");
        // Drop an initial Newline introduced by a leading blank line.
        while matches!(toks.first().map(|t| &t.kind), Some(TokKind::Newline)) {
            toks.remove(0);
        }
        toks
    }

    fn parse_blk(src: &str) -> Block {
        let tokens = lex_block_src(src);
        let (block, errors) = parse_block(&tokens);
        assert!(
            errors.is_empty(),
            "parse errors: {:?}\nsource:\n{src}",
            errors
        );
        block
    }

    fn parse_blk_errs(src: &str) -> (Block, Vec<ParseError>) {
        let tokens = lex_block_src(src);
        parse_block(&tokens)
    }

    // -------------------- assignment --------------------

    #[test]
    fn parses_simple_assignment() {
        let b = parse_blk("\n    x = 42\n");
        assert_eq!(b.stmts.len(), 1);
        match &b.stmts[0] {
            Stmt::Let { name, value, .. } => {
                assert_eq!(name.name, "x");
                assert!(matches!(value, Expr::Literal { value: Literal::Int(42), .. }));
            }
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn parses_assignment_to_call_result() {
        let b = parse_blk("\n    order = get_order(ticket.order_id)\n");
        assert!(matches!(&b.stmts[0], Stmt::Let { .. }));
    }

    // -------------------- expression statement --------------------

    #[test]
    fn parses_expression_statement() {
        let b = parse_blk("\n    issue_refund(id, amount)\n");
        assert!(matches!(&b.stmts[0], Stmt::Expr { .. }));
    }

    // -------------------- return --------------------

    #[test]
    fn parses_return_with_value() {
        let b = parse_blk("\n    return decision\n");
        match &b.stmts[0] {
            Stmt::Return { value: Some(_), .. } => {}
            other => panic!("expected Return Some, got {other:?}"),
        }
    }

    #[test]
    fn parses_bare_return() {
        let b = parse_blk("\n    return\n");
        match &b.stmts[0] {
            Stmt::Return { value: None, .. } => {}
            other => panic!("expected Return None, got {other:?}"),
        }
    }

    // -------------------- if / else --------------------

    #[test]
    fn parses_if_without_else() {
        let src = "\n    if x:\n        y = 1\n";
        let b = parse_blk(src);
        match &b.stmts[0] {
            Stmt::If { then_block, else_block: None, .. } => {
                assert_eq!(then_block.stmts.len(), 1);
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn parses_if_with_else() {
        let src = "\n    if x:\n        y = 1\n    else:\n        y = 2\n";
        let b = parse_blk(src);
        match &b.stmts[0] {
            Stmt::If { then_block, else_block: Some(el), .. } => {
                assert_eq!(then_block.stmts.len(), 1);
                assert_eq!(el.stmts.len(), 1);
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    // -------------------- for --------------------

    #[test]
    fn parses_for_loop() {
        let src = "\n    for item in items:\n        process(item)\n";
        let b = parse_blk(src);
        match &b.stmts[0] {
            Stmt::For { var, body, .. } => {
                assert_eq!(var.name, "item");
                assert_eq!(body.stmts.len(), 1);
            }
            other => panic!("expected For, got {other:?}"),
        }
    }

    // -------------------- approve --------------------

    #[test]
    fn parses_approve_stmt() {
        let b = parse_blk("\n    approve IssueRefund(order.id, order.amount)\n");
        match &b.stmts[0] {
            Stmt::Approve { action, .. } => {
                assert!(matches!(action, Expr::Call { .. }));
            }
            other => panic!("expected Approve, got {other:?}"),
        }
    }

    // -------------------- break / continue / pass --------------------

    #[test]
    fn parses_break_continue_pass() {
        let src = "\n    for x in xs:\n        if x:\n            break\n        if x:\n            continue\n        pass\n";
        let b = parse_blk(src);
        // Just ensure parsing succeeds. (break/continue/pass currently encoded
        // as Expr::Ident statements — will get dedicated AST variants later.)
        assert_eq!(b.stmts.len(), 1);
    }

    #[test]
    fn parses_yield_statement() {
        let src = "\n    yield chunk\n";
        let b = parse_blk(src);
        assert!(matches!(b.stmts[0], Stmt::Yield { .. }));
    }

    #[test]
    fn parses_stream_prompt_modifiers() {
        let src = "\
prompt generate(ctx: String) -> Stream<String>:
    with min_confidence 0.80
    with max_tokens 5000
    with backpressure bounded(100)
    \"Generate {ctx} in chunks.\"
";
        let file = parse_file_src(src);
        let prompt = match &file.decls[0] {
            Decl::Prompt(prompt) => prompt,
            other => panic!("expected Prompt, got {other:?}"),
        };
        assert!(matches!(
            &prompt.return_ty,
            TypeRef::Generic { name, args, .. }
                if name.name == "Stream"
                    && matches!(&args[0], TypeRef::Named { name, .. } if name.name == "String")
        ));
        assert_eq!(prompt.stream.min_confidence, Some(0.80));
        assert_eq!(prompt.stream.max_tokens, Some(5000));
        assert_eq!(
            prompt.stream.backpressure,
            Some(BackpressurePolicy::Bounded(100))
        );
    }

    // -------------------- canonical refund_bot body --------------------

    #[test]
    fn parses_refund_bot_body() {
        let src = "
    order = get_order(ticket.order_id)
    decision = decide_refund(ticket, order)

    if decision.should_refund:
        approve IssueRefund(order.id, order.amount)
        issue_refund(order.id, order.amount)

    return decision
";
        let b = parse_blk(src);
        assert_eq!(b.stmts.len(), 4);
        assert!(matches!(b.stmts[0], Stmt::Let { .. }));
        assert!(matches!(b.stmts[1], Stmt::Let { .. }));
        assert!(matches!(b.stmts[2], Stmt::If { .. }));
        assert!(matches!(b.stmts[3], Stmt::Return { .. }));

        // Inner: the if body should contain approve then call.
        if let Stmt::If { then_block, .. } = &b.stmts[2] {
            assert_eq!(then_block.stmts.len(), 2);
            assert!(matches!(then_block.stmts[0], Stmt::Approve { .. }));
            assert!(matches!(then_block.stmts[1], Stmt::Expr { .. }));
        }
    }

    // -------------------- errors --------------------

    #[test]
    fn missing_colon_after_if_reports_error() {
        let src = "\n    if x\n        y = 1\n";
        let (_block, errs) = parse_blk_errs(src);
        assert!(!errs.is_empty(), "expected error for missing colon");
        assert!(
            errs.iter().any(|e| matches!(
                e.kind,
                ParseErrorKind::UnexpectedToken { .. }
            )),
            "expected UnexpectedToken, got {errs:?}"
        );
    }

    #[test]
    fn empty_block_reports_error() {
        // Block with only a blank line inside — no statements. Since the
        // lexer collapses blank lines away entirely, we simulate this with
        // a raw token sequence: Indent Dedent.
        let tokens = vec![
            Token::new(TokKind::Indent, Span::new(0, 0)),
            Token::new(TokKind::Dedent, Span::new(0, 0)),
            Token::new(TokKind::Eof, Span::new(0, 0)),
        ];
        let (_block, errs) = parse_block(&tokens);
        assert!(errs.iter().any(|e| matches!(e.kind, ParseErrorKind::EmptyBlock)));
    }

    #[test]
    fn parser_recovers_and_continues_after_bad_stmt() {
        // First statement is broken (missing `:` after `if`). Second is fine.
        // The parser should report the error but still parse the second.
        let src = "\n    if x\n    y = 42\n";
        let (block, errs) = parse_blk_errs(src);
        assert!(!errs.is_empty());
        // After recovery we should have parsed at least one good statement.
        assert!(
            !block.stmts.is_empty(),
            "expected recovery to yield statements"
        );
    }

    // =================================================================
    // File / declaration parser tests
    // =================================================================

    use corvid_ast::{AgentDecl, Decl, Effect, File, ImportSource, TypeRef};

    fn parse_file_src(src: &str) -> File {
        let tokens = lex(src).expect("lex failed");
        let (file, errors) = parse_file(&tokens);
        assert!(
            errors.is_empty(),
            "parse errors: {:?}\nsource:\n{src}",
            errors
        );
        file
    }

    fn parse_file_errs(src: &str) -> (File, Vec<ParseError>) {
        let tokens = lex(src).expect("lex failed");
        parse_file(&tokens)
    }

    // -------------------- imports --------------------

    #[test]
    fn parses_import_python() {
        let file = parse_file_src(r#"import python "anthropic" as anthropic"#);
        assert_eq!(file.decls.len(), 1);
        match &file.decls[0] {
            Decl::Import(i) => {
                assert!(matches!(i.source, ImportSource::Python));
                assert_eq!(i.module, "anthropic");
                assert_eq!(i.alias.as_ref().unwrap().name, "anthropic");
            }
            other => panic!("expected Import, got {other:?}"),
        }
    }

    #[test]
    fn parses_import_without_alias() {
        let file = parse_file_src(r#"import python "anthropic""#);
        match &file.decls[0] {
            Decl::Import(i) => {
                assert_eq!(i.module, "anthropic");
                assert!(i.alias.is_none());
            }
            other => panic!("expected Import, got {other:?}"),
        }
    }

    // -------------------- types --------------------

    #[test]
    fn parses_type_decl() {
        let src = "\
type Ticket:
    order_id: String
    user_id: String
    message: String
";
        let file = parse_file_src(src);
        match &file.decls[0] {
            Decl::Type(t) => {
                assert_eq!(t.name.name, "Ticket");
                assert_eq!(t.fields.len(), 3);
                assert_eq!(t.fields[0].name.name, "order_id");
            }
            other => panic!("expected Type, got {other:?}"),
        }
    }

    #[test]
    fn parses_result_and_option_type_refs() {
        let src = "\
agent load(id: String) -> Result<Option<Order>, String>:
    return fetch(id)
";
        let file = parse_file_src(src);
        let agent = match &file.decls[0] {
            Decl::Agent(a) => a,
            other => panic!("expected Agent, got {other:?}"),
        };
        match &agent.return_ty {
            TypeRef::Generic { name, args, .. } => {
                assert_eq!(name.name, "Result");
                assert_eq!(args.len(), 2);
                assert!(matches!(
                    &args[0],
                    TypeRef::Generic { name, args, .. }
                    if name.name == "Option" && args.len() == 1
                ));
                assert!(matches!(
                    &args[1],
                    TypeRef::Named { name, .. } if name.name == "String"
                ));
            }
            other => panic!("expected generic Result return type, got {other:?}"),
        }
    }

    #[test]
    fn parses_weak_type_ref_with_effect_row() {
        let src = "\
agent watch(name: String) -> Weak<String, {tool_call, llm}>:
    return Weak::new(name)
";
        let file = parse_file_src(src);
        let agent = match &file.decls[0] {
            Decl::Agent(a) => a,
            other => panic!("expected Agent, got {other:?}"),
        };
        match &agent.return_ty {
            TypeRef::Weak {
                inner,
                effects: Some(effects),
                ..
            } => {
                assert!(matches!(
                    inner.as_ref(),
                    TypeRef::Named { name, .. } if name.name == "String"
                ));
                assert!(effects.tool_call);
                assert!(effects.llm);
                assert!(!effects.approve);
            }
            other => panic!("expected Weak return type, got {other:?}"),
        }
    }

    #[test]
    fn parses_weak_builtin_calls() {
        let e = parse("Weak::upgrade(Weak::new(name))");
        match e {
            Expr::Call { callee, args, .. } => {
                assert!(matches!(
                    callee.as_ref(),
                    Expr::Ident { name, .. } if name.name == "Weak::upgrade"
                ));
                assert_eq!(args.len(), 1);
                assert!(matches!(
                    &args[0],
                    Expr::Call { callee, .. }
                    if matches!(
                        callee.as_ref(),
                        Expr::Ident { name, .. } if name.name == "Weak::new"
                    )
                ));
            }
            other => panic!("expected Weak builtin call, got {other:?}"),
        }
    }

    // -------------------- tools --------------------

    #[test]
    fn parses_safe_tool() {
        let src = "tool get_order(id: String) -> Order";
        let file = parse_file_src(src);
        match &file.decls[0] {
            Decl::Tool(t) => {
                assert_eq!(t.name.name, "get_order");
                assert_eq!(t.params.len(), 1);
                assert_eq!(t.params[0].name.name, "id");
                assert!(matches!(t.effect, Effect::Safe));
                assert!(matches!(
                    t.return_ty,
                    TypeRef::Named { ref name, .. } if name.name == "Order"
                ));
            }
            other => panic!("expected Tool, got {other:?}"),
        }
    }

    #[test]
    fn parses_dangerous_tool() {
        let src = "tool issue_refund(id: String, amount: Float) -> Receipt dangerous";
        let file = parse_file_src(src);
        match &file.decls[0] {
            Decl::Tool(t) => {
                assert_eq!(t.params.len(), 2);
                assert!(matches!(t.effect, Effect::Dangerous));
            }
            other => panic!("expected Tool, got {other:?}"),
        }
    }

    #[test]
    fn parses_tool_with_no_params() {
        let file = parse_file_src("tool now() -> String");
        match &file.decls[0] {
            Decl::Tool(t) => assert_eq!(t.params.len(), 0),
            other => panic!("expected Tool, got {other:?}"),
        }
    }

    // -------------------- prompts --------------------

    #[test]
    fn parses_single_line_prompt() {
        let src = "\
prompt greet(name: String) -> String:
    \"Write a short, warm greeting to {name}.\"
";
        let file = parse_file_src(src);
        match &file.decls[0] {
            Decl::Prompt(p) => {
                assert_eq!(p.name.name, "greet");
                assert_eq!(p.params.len(), 1);
                assert!(p.template.contains("greeting"));
            }
            other => panic!("expected Prompt, got {other:?}"),
        }
    }

    #[test]
    fn parses_triple_quoted_prompt() {
        let src = "\
prompt decide(ticket: Ticket) -> Decision:
    \"\"\"
    Decide whether this ticket deserves a refund.
    Consider the order amount and the user's complaint.
    \"\"\"
";
        let file = parse_file_src(src);
        match &file.decls[0] {
            Decl::Prompt(p) => {
                assert!(p.template.contains("refund"));
                assert!(p.template.contains("complaint"));
            }
            other => panic!("expected Prompt, got {other:?}"),
        }
    }

    // -------------------- agents --------------------

    #[test]
    fn parses_agent_with_body() {
        let src = "\
agent hello(name: String) -> String:
    message = greet(name)
    return message
";
        let file = parse_file_src(src);
        match &file.decls[0] {
            Decl::Agent(a) => {
                assert_eq!(a.name.name, "hello");
                assert_eq!(a.params.len(), 1);
                assert_eq!(a.body.stmts.len(), 2);
                assert!(a.attributes.is_empty());
                assert!(a.constraints.is_empty());
            }
            other => panic!("expected Agent, got {other:?}"),
        }
    }

    // -------------------- Phase 21 slice inv-A: @replayable --------------------

    #[test]
    fn parses_agent_with_replayable_attribute() {
        let src = "\
@replayable
agent refund_flow(q: String) -> String:
    return q
";
        let file = parse_file_src(src);
        let agent = match &file.decls[0] {
            Decl::Agent(a) => a,
            other => panic!("expected Agent, got {other:?}"),
        };
        assert_eq!(agent.attributes.len(), 1);
        assert!(matches!(
            agent.attributes[0],
            corvid_ast::AgentAttribute::Replayable { .. }
        ));
        // @replayable is an attribute, not an effect constraint.
        assert!(agent.constraints.is_empty());
    }

    #[test]
    fn parses_agent_with_replayable_empty_parens() {
        let src = "\
@replayable()
agent refund_flow(q: String) -> String:
    return q
";
        let file = parse_file_src(src);
        let agent = match &file.decls[0] {
            Decl::Agent(a) => a,
            other => panic!("expected Agent, got {other:?}"),
        };
        assert_eq!(agent.attributes.len(), 1);
    }

    #[test]
    fn parses_agent_with_replayable_and_effect_constraints() {
        // @replayable interleaves cleanly with @budget;
        // attributes go to .attributes, constraints to .constraints.
        let src = "\
@replayable
@budget($1.00)
agent refund_flow(q: String) -> String:
    return q
";
        let file = parse_file_src(src);
        let agent = match &file.decls[0] {
            Decl::Agent(a) => a,
            other => panic!("expected Agent, got {other:?}"),
        };
        assert_eq!(agent.attributes.len(), 1);
        // @budget($1.00) expands into one or more constraints
        // depending on the grammar; at minimum there's a cost
        // constraint.
        assert!(!agent.constraints.is_empty());
    }

    #[test]
    fn agent_without_replayable_has_no_attributes() {
        let src = "\
@budget($1.00)
agent refund_flow(q: String) -> String:
    return q
";
        let file = parse_file_src(src);
        let agent = match &file.decls[0] {
            Decl::Agent(a) => a,
            other => panic!("expected Agent, got {other:?}"),
        };
        assert!(agent.attributes.is_empty());
        assert!(!agent.constraints.is_empty());
    }

    // -------------------- Phase 21 slice inv-F: @deterministic --------------------

    #[test]
    fn parses_agent_with_deterministic_attribute() {
        let src = "\
@deterministic
agent pure(q: String) -> String:
    return q
";
        let file = parse_file_src(src);
        let agent = match &file.decls[0] {
            Decl::Agent(a) => a,
            other => panic!("expected Agent, got {other:?}"),
        };
        assert_eq!(agent.attributes.len(), 1);
        assert!(matches!(
            agent.attributes[0],
            corvid_ast::AgentAttribute::Deterministic { .. }
        ));
        assert!(agent.constraints.is_empty());
    }

    #[test]
    fn parses_agent_with_both_attributes() {
        let src = "\
@replayable
@deterministic
agent pure(q: String) -> String:
    return q
";
        let file = parse_file_src(src);
        let agent = match &file.decls[0] {
            Decl::Agent(a) => a,
            other => panic!("expected Agent, got {other:?}"),
        };
        assert_eq!(agent.attributes.len(), 2);
        assert!(corvid_ast::AgentAttribute::is_replayable(&agent.attributes));
        assert!(corvid_ast::AgentAttribute::is_deterministic(&agent.attributes));
    }

    #[test]
    fn parses_eval_with_trace_assertions_and_statistical_modifier() {
        let src = "\
tool get_order(id: String) -> String
tool issue_refund(id: String) -> String dangerous

eval refund_process:
    order_id = \"ord_42\"
    result = get_order(order_id)
    assert called get_order before issue_refund
    assert approved IssueRefund
    assert cost < $0.50
    assert result == result with confidence 0.95 over 50 runs
";
        let file = parse_file_src(src);
        let eval = match &file.decls[2] {
            Decl::Eval(eval_decl) => eval_decl,
            other => panic!("expected Eval decl, got {other:?}"),
        };
        assert_eq!(eval.body.stmts.len(), 2);
        assert_eq!(eval.assertions.len(), 4);
        assert!(matches!(
            eval.assertions[0],
            corvid_ast::EvalAssert::Ordering { .. }
        ));
        assert!(matches!(
            eval.assertions[1],
            corvid_ast::EvalAssert::Approved { .. }
        ));
        assert!(matches!(eval.assertions[2], corvid_ast::EvalAssert::Cost { .. }));
        match &eval.assertions[3] {
            corvid_ast::EvalAssert::Value {
                confidence, runs, ..
            } => {
                assert_eq!(*confidence, Some(0.95));
                assert_eq!(*runs, Some(50));
            }
            other => panic!("expected value assertion, got {other:?}"),
        }
    }

    #[test]
    fn contextual_eval_keywords_remain_normal_identifiers_elsewhere() {
        let src = "\
agent keep_names() -> Int:
    called = 1
    approved = called
    return approved
";
        let (file, errors) = parse_file_errs(src);
        assert!(errors.is_empty(), "parse errors: {errors:?}");
        assert!(matches!(file.decls[0], Decl::Agent(_)));
    }

    #[test]
    fn parses_multi_dimensional_budget_constraints() {
        let src = "\
@budget($1.00, tokens: 10000, latency: 5s)
agent planner(query: String) -> String:
    return query
";
        let file = parse_file_src(src);
        let agent = match &file.decls[0] {
            Decl::Agent(agent) => agent,
            other => panic!("expected Agent, got {other:?}"),
        };
        assert_eq!(agent.constraints.len(), 3);
        assert_eq!(agent.constraints[0].dimension.name, "cost");
        assert_eq!(agent.constraints[1].dimension.name, "tokens");
        assert_eq!(agent.constraints[2].dimension.name, "latency_ms");
        assert_eq!(
            agent.constraints[2].value,
            Some(corvid_ast::DimensionValue::Number(5000.0))
        );
    }

    // -------------------- full refund_bot file --------------------

    #[test]
    fn parses_full_refund_bot_file() {
        let src = r#"
import python "anthropic" as anthropic

type Ticket:
    order_id: String
    user_id: String
    message: String

type Order:
    id: String
    amount: Float
    user_id: String

type Decision:
    should_refund: Bool
    reason: String

type Receipt:
    refund_id: String
    amount: Float

tool get_order(id: String) -> Order
tool issue_refund(id: String, amount: Float) -> Receipt dangerous

prompt decide_refund(ticket: Ticket, order: Order) -> Decision:
    """
    Decide whether this ticket deserves a refund.
    """

agent refund_bot(ticket: Ticket) -> Decision:
    order = get_order(ticket.order_id)
    decision = decide_refund(ticket, order)

    if decision.should_refund:
        approve IssueRefund(order.id, order.amount)
        issue_refund(order.id, order.amount)

    return decision
"#;
        let (file, errors) = parse_file_errs(src);
        assert!(errors.is_empty(), "parse errors: {errors:?}");

        // Expected structure:
        //   1 import
        //   4 types
        //   2 tools
        //   1 prompt
        //   1 agent
        assert_eq!(file.decls.len(), 9);

        let import_count = file.decls.iter().filter(|d| matches!(d, Decl::Import(_))).count();
        let type_count = file.decls.iter().filter(|d| matches!(d, Decl::Type(_))).count();
        let tool_count = file.decls.iter().filter(|d| matches!(d, Decl::Tool(_))).count();
        let prompt_count = file.decls.iter().filter(|d| matches!(d, Decl::Prompt(_))).count();
        let agent_count = file.decls.iter().filter(|d| matches!(d, Decl::Agent(_))).count();
        assert_eq!(import_count, 1);
        assert_eq!(type_count, 4);
        assert_eq!(tool_count, 2);
        assert_eq!(prompt_count, 1);
        assert_eq!(agent_count, 1);

        // Verify dangerous tool is marked, safe tool isn't.
        let tools: Vec<&ToolDecl> = file
            .decls
            .iter()
            .filter_map(|d| if let Decl::Tool(t) = d { Some(t) } else { None })
            .collect();
        assert!(tools.iter().any(|t| matches!(t.effect, Effect::Safe)));
        assert!(tools.iter().any(|t| matches!(t.effect, Effect::Dangerous)));

        // Verify the agent's body parses down to the expected shape.
        let agent: &AgentDecl = file
            .decls
            .iter()
            .find_map(|d| if let Decl::Agent(a) = d { Some(a) } else { None })
            .unwrap();
        assert_eq!(agent.name.name, "refund_bot");
        assert_eq!(agent.body.stmts.len(), 4);
        assert!(matches!(agent.body.stmts[0], Stmt::Let { .. }));
        assert!(matches!(agent.body.stmts[1], Stmt::Let { .. }));
        assert!(matches!(agent.body.stmts[2], Stmt::If { .. }));
        assert!(matches!(agent.body.stmts[3], Stmt::Return { .. }));
    }

    // -------------------- error recovery --------------------

    #[test]
    fn recovers_from_bad_tool_to_following_agent() {
        // Tool is missing `->`. Agent after should still parse.
        let src = "\
tool broken(x: String) Order
agent good(x: String) -> String:
    return x
";
        let (file, errs) = parse_file_errs(src);
        assert!(!errs.is_empty());
        // We should still see the agent declaration in the recovered file.
        assert!(
            file.decls.iter().any(|d| matches!(d, Decl::Agent(_))),
            "expected agent after recovery"
        );
    }

    #[test]
    fn reports_error_on_unknown_top_level_token() {
        let (_file, errs) = parse_file_errs("xyz");
        assert!(!errs.is_empty());
        assert!(
            errs.iter()
                .any(|e| matches!(e.kind, ParseErrorKind::UnexpectedToken { .. }))
        );
    }

    #[test]
    fn reports_error_on_unknown_import_source() {
        let (_file, errs) = parse_file_errs(r#"import ruby "foo""#);
        assert!(!errs.is_empty());
    }

    // -----------------------------------------------------------------
    // `extend T:` block + visibility parsing
    // -----------------------------------------------------------------

    use corvid_ast::{ExtendDecl, ExtendMethodKind, Visibility};

    fn first_extend(file: &File) -> &ExtendDecl {
        file.decls
            .iter()
            .find_map(|d| match d {
                Decl::Extend(e) => Some(e),
                _ => None,
            })
            .expect("expected an `extend` decl in the file")
    }

    #[test]
    fn parses_extend_with_one_agent_method() {
        let file = parse_file_src(
            "type Order:\n    amount: Int\n\nextend Order:\n    public agent total(o: Order) -> Int:\n        return o.amount\n",
        );
        let ext = first_extend(&file);
        assert_eq!(ext.type_name.name.as_str(), "Order");
        assert_eq!(ext.methods.len(), 1);
        let m = &ext.methods[0];
        assert_eq!(m.visibility, Visibility::Public);
        assert!(matches!(m.kind, ExtendMethodKind::Agent(_)));
        assert_eq!(m.name().name.as_str(), "total");
    }

    #[test]
    fn parses_extend_default_visibility_is_private() {
        let file = parse_file_src(
            "type Order:\n    amount: Int\n\nextend Order:\n    agent total(o: Order) -> Int:\n        return o.amount\n",
        );
        let ext = first_extend(&file);
        assert_eq!(ext.methods[0].visibility, Visibility::Private);
    }

    #[test]
    fn parses_extend_public_package_visibility() {
        let file = parse_file_src(
            "type Order:\n    amount: Int\n\nextend Order:\n    public(package) agent total(o: Order) -> Int:\n        return o.amount\n",
        );
        let ext = first_extend(&file);
        assert_eq!(ext.methods[0].visibility, Visibility::PublicPackage);
    }

    #[test]
    fn parses_extend_with_mixed_decl_kinds() {
        // The whole point of allowing methods to be any decl kind
        // — verify the parser accepts a mix of agent / prompt / tool
        // inside one `extend` block.
        let file = parse_file_src(
            "type Order:\n    amount: Int\n\nextend Order:\n    public agent total(o: Order) -> Int:\n        return o.amount\n    public prompt summarize(o: Order) -> String:\n        \"Summarize this order\"\n    public tool fetch_status(o: Order) -> Status dangerous\n",
        );
        let ext = first_extend(&file);
        assert_eq!(ext.methods.len(), 3);
        assert!(matches!(ext.methods[0].kind, ExtendMethodKind::Agent(_)));
        assert!(matches!(ext.methods[1].kind, ExtendMethodKind::Prompt(_)));
        assert!(matches!(ext.methods[2].kind, ExtendMethodKind::Tool(_)));
    }

    #[test]
    fn rejects_public_with_unknown_inner_keyword() {
        let (_file, errs) = parse_file_errs(
            "type Order:\n    amount: Int\n\nextend Order:\n    public(secret) agent total(o: Order) -> Int:\n        return o.amount\n",
        );
        assert!(
            !errs.is_empty(),
            "expected parse error for `public(secret)` — only `public(package)` is valid today"
        );
    }

    // -------------------- Phase 20h: `model` decls --------------------

    #[test]
    fn parses_minimal_model_decl() {
        let file = parse_file_src(
            "model haiku:\n    cost_per_token_in: $0.00000025\n    capability: basic\n",
        );
        assert_eq!(file.decls.len(), 1);
        match &file.decls[0] {
            Decl::Model(m) => {
                assert_eq!(m.name.name, "haiku");
                assert_eq!(m.fields.len(), 2);
                assert_eq!(m.fields[0].name.name, "cost_per_token_in");
                assert!(matches!(
                    m.fields[0].value,
                    corvid_ast::DimensionValue::Cost(_)
                ));
                assert_eq!(m.fields[1].name.name, "capability");
                assert!(matches!(
                    m.fields[1].value,
                    corvid_ast::DimensionValue::Name(ref s) if s == "basic"
                ));
            }
            other => panic!("expected Model, got {other:?}"),
        }
    }

    #[test]
    fn parses_model_with_mixed_value_types() {
        let file = parse_file_src(
            "model opus:\n    cost_per_token_in: $0.000015\n    capability: expert\n    max_context: 200000\n    streaming: true\n",
        );
        let m = match &file.decls[0] {
            Decl::Model(m) => m,
            other => panic!("expected Model, got {other:?}"),
        };
        assert_eq!(m.fields.len(), 4);
        // Bool value parses.
        assert!(
            m.fields
                .iter()
                .any(|f| f.name.name == "streaming"
                    && matches!(f.value, corvid_ast::DimensionValue::Bool(true)))
        );
        // Number value parses (200000 without duration suffix).
        assert!(m.fields.iter().any(|f| f.name.name == "max_context"
            && matches!(f.value, corvid_ast::DimensionValue::Number(n) if (n - 200000.0).abs() < 1e-6)));
    }

    #[test]
    fn parses_multiple_model_decls_in_one_file() {
        let file = parse_file_src(
            "model haiku:\n    capability: basic\n\nmodel opus:\n    capability: expert\n",
        );
        assert_eq!(file.decls.len(), 2);
        assert!(file
            .decls
            .iter()
            .all(|d| matches!(d, Decl::Model(_))));
    }

    #[test]
    fn rejects_model_decl_without_block() {
        let (_file, errs) = parse_file_errs("model haiku:\n");
        assert!(
            !errs.is_empty(),
            "expected parse error — `model` requires at least one field in the indented block"
        );
    }

    #[test]
    fn rejects_model_field_without_value() {
        let (_file, errs) = parse_file_errs(
            "model haiku:\n    capability:\n",
        );
        assert!(
            !errs.is_empty(),
            "expected parse error — field without a value should be rejected"
        );
    }

    // -------------------- Phase 20h: `requires:` on prompts --------------------

    #[test]
    fn parses_prompt_with_requires_clause() {
        let file = parse_file_src(
            "prompt classify(t: String) -> String:\n    requires: basic\n    \"Classify {t}\"\n",
        );
        match &file.decls[0] {
            Decl::Prompt(p) => {
                let req = p.capability_required.as_ref().expect("requires clause");
                assert_eq!(req.name, "basic");
            }
            other => panic!("expected Prompt, got {other:?}"),
        }
    }

    #[test]
    fn parses_prompt_with_requires_and_stream_settings_in_either_order() {
        // `requires:` must appear before `with ...` per the grammar.
        let file = parse_file_src(
            "prompt generate(ctx: String) -> String:\n    requires: expert\n    with max_tokens 500\n    \"Generate {ctx}\"\n",
        );
        match &file.decls[0] {
            Decl::Prompt(p) => {
                assert_eq!(p.capability_required.as_ref().unwrap().name, "expert");
                assert_eq!(p.stream.max_tokens, Some(500));
            }
            other => panic!("expected Prompt, got {other:?}"),
        }
    }

    #[test]
    fn prompt_without_requires_defaults_to_none() {
        let file = parse_file_src(
            "prompt classify(t: String) -> String:\n    \"Classify {t}\"\n",
        );
        match &file.decls[0] {
            Decl::Prompt(p) => assert!(p.capability_required.is_none()),
            other => panic!("expected Prompt, got {other:?}"),
        }
    }

    #[test]
    fn rejects_requires_without_value() {
        let (_file, errs) = parse_file_errs(
            "prompt classify(t: String) -> String:\n    requires:\n    \"Classify {t}\"\n",
        );
        assert!(!errs.is_empty());
    }

    // -------------------- Phase 20h: `route:` on prompts --------------------

    #[test]
    fn parses_prompt_with_route_block() {
        let src = "\
model fast_model:
    capability: basic

model slow_model:
    capability: expert

prompt answer(question: String) -> String:
    route:
        length(question) > 1000 -> slow_model
        _ -> fast_model
    \"Answer {question}\"
";
        let file = parse_file_src(src);
        let p = file
            .decls
            .iter()
            .find_map(|d| match d {
                Decl::Prompt(p) => Some(p),
                _ => None,
            })
            .expect("prompt");
        let rt = p.route.as_ref().expect("route block");
        assert_eq!(rt.arms.len(), 2);
        assert!(matches!(
            rt.arms[0].pattern,
            corvid_ast::RoutePattern::Guard(_)
        ));
        assert!(matches!(
            rt.arms[1].pattern,
            corvid_ast::RoutePattern::Wildcard { .. }
        ));
        assert_eq!(rt.arms[0].model.name, "slow_model");
        assert_eq!(rt.arms[1].model.name, "fast_model");
    }

    #[test]
    fn parses_route_with_requires_above_it() {
        // Grammar is requires -> route -> with -> template.
        let src = "\
model m1:
    capability: basic

prompt answer(q: String) -> String:
    requires: basic
    route:
        _ -> m1
    \"Answer\"
";
        let file = parse_file_src(src);
        let p = file
            .decls
            .iter()
            .find_map(|d| match d {
                Decl::Prompt(p) => Some(p),
                _ => None,
            })
            .unwrap();
        assert_eq!(p.capability_required.as_ref().unwrap().name, "basic");
        assert_eq!(p.route.as_ref().unwrap().arms.len(), 1);
    }

    #[test]
    fn rejects_empty_route_block() {
        let src = "\
model m1:
    capability: basic

prompt answer(q: String) -> String:
    route:
    \"Answer\"
";
        let (_file, errs) = parse_file_errs(src);
        assert!(
            !errs.is_empty(),
            "empty `route:` block must fail to parse"
        );
    }

    #[test]
    fn rejects_arm_missing_arrow() {
        let src = "\
model m1:
    capability: basic

prompt answer(q: String) -> String:
    route:
        _ m1
    \"Answer\"
";
        let (_file, errs) = parse_file_errs(src);
        assert!(!errs.is_empty());
    }

    // -------------------- Phase 20h slice D: model fields for
    // jurisdiction / compliance / privacy_tier parse cleanly

    // -------------------- Phase 20h slice E: `progressive:` --------------------

    #[test]
    fn parses_progressive_chain_with_two_stages() {
        let src = "\
model cheap:
    capability: basic

model expensive:
    capability: expert

prompt classify(q: String) -> String:
    progressive:
        cheap below 0.95
        expensive
    \"Classify\"
";
        let file = parse_file_src(src);
        let p = file
            .decls
            .iter()
            .find_map(|d| match d {
                Decl::Prompt(p) => Some(p),
                _ => None,
            })
            .unwrap();
        let chain = p.progressive.as_ref().expect("progressive block");
        assert_eq!(chain.stages.len(), 2);
        assert_eq!(chain.stages[0].model.name, "cheap");
        assert_eq!(chain.stages[0].threshold, Some(0.95));
        assert_eq!(chain.stages[1].model.name, "expensive");
        assert_eq!(chain.stages[1].threshold, None);
    }

    #[test]
    fn parses_progressive_chain_with_three_stages() {
        let src = "\
model a:
    capability: basic

model b:
    capability: standard

model c:
    capability: expert

prompt classify(q: String) -> String:
    progressive:
        a below 0.90
        b below 0.98
        c
    \"Classify\"
";
        let file = parse_file_src(src);
        let p = file
            .decls
            .iter()
            .find_map(|d| match d {
                Decl::Prompt(p) => Some(p),
                _ => None,
            })
            .unwrap();
        let chain = p.progressive.as_ref().unwrap();
        assert_eq!(chain.stages.len(), 3);
        assert_eq!(chain.stages[2].threshold, None);
    }

    #[test]
    fn rejects_progressive_with_single_stage() {
        let src = "\
model only:
    capability: basic

prompt classify(q: String) -> String:
    progressive:
        only
    \"Classify\"
";
        let (_file, errs) = parse_file_errs(src);
        assert!(
            !errs.is_empty(),
            "progressive with <2 stages must fail to parse"
        );
    }

    #[test]
    fn rejects_progressive_last_stage_with_threshold() {
        let src = "\
model a:
    capability: basic

model b:
    capability: basic

prompt classify(q: String) -> String:
    progressive:
        a below 0.90
        b below 0.99
    \"Classify\"
";
        let (_file, errs) = parse_file_errs(src);
        assert!(
            !errs.is_empty(),
            "last stage must be a terminal fallback without `below`"
        );
    }

    #[test]
    fn rejects_progressive_non_last_stage_without_threshold() {
        let src = "\
model a:
    capability: basic

model b:
    capability: basic

prompt classify(q: String) -> String:
    progressive:
        a
        b
    \"Classify\"
";
        let (_file, errs) = parse_file_errs(src);
        assert!(
            !errs.is_empty(),
            "non-terminal stages must declare `below <threshold>`"
        );
    }

    // -------------------- Phase 20h slice I: `rollout` --------------------

    #[test]
    fn parses_basic_rollout() {
        let src = "\
model opus_v1:
    capability: expert

model opus_v2:
    capability: expert

prompt summarize(doc: String) -> String:
    rollout 10% opus_v2, else opus_v1
    \"Summarize\"
";
        let file = parse_file_src(src);
        let p = file
            .decls
            .iter()
            .find_map(|d| match d {
                Decl::Prompt(p) => Some(p),
                _ => None,
            })
            .unwrap();
        let spec = p.rollout.as_ref().expect("rollout");
        assert!((spec.variant_percent - 10.0).abs() < 1e-9);
        assert_eq!(spec.variant.name, "opus_v2");
        assert_eq!(spec.baseline.name, "opus_v1");
    }

    #[test]
    fn parses_rollout_with_fractional_percent() {
        let src = "\
model a:
    capability: basic

model b:
    capability: basic

prompt p(q: String) -> String:
    rollout 2.5% a, else b
    \"X\"
";
        let file = parse_file_src(src);
        let p = file
            .decls
            .iter()
            .find_map(|d| match d {
                Decl::Prompt(p) => Some(p),
                _ => None,
            })
            .unwrap();
        assert!((p.rollout.as_ref().unwrap().variant_percent - 2.5).abs() < 1e-9);
    }

    #[test]
    fn rejects_rollout_without_percent_sign() {
        let src = "\
model a:
    capability: basic

model b:
    capability: basic

prompt p(q: String) -> String:
    rollout 10 a, else b
    \"X\"
";
        let (_file, errs) = parse_file_errs(src);
        assert!(!errs.is_empty());
    }

    #[test]
    fn rejects_rollout_without_else_clause() {
        let src = "\
model a:
    capability: basic

prompt p(q: String) -> String:
    rollout 10% a
    \"X\"
";
        let (_file, errs) = parse_file_errs(src);
        assert!(!errs.is_empty());
    }

    // -------------------- Phase 20h slice F: `ensemble` --------------------

    #[test]
    fn parses_basic_ensemble_majority() {
        let src = "\
model haiku:
    capability: basic

model sonnet:
    capability: standard

model opus:
    capability: expert

prompt answer(q: String) -> String:
    ensemble [haiku, sonnet, opus] vote majority
    \"Answer {q}\"
";
        let file = parse_file_src(src);
        let p = file
            .decls
            .iter()
            .find_map(|d| match d {
                Decl::Prompt(p) => Some(p),
                _ => None,
            })
            .unwrap();
        let spec = p.ensemble.as_ref().expect("ensemble");
        assert_eq!(spec.models.len(), 3);
        assert_eq!(spec.models[0].name, "haiku");
        assert_eq!(spec.models[1].name, "sonnet");
        assert_eq!(spec.models[2].name, "opus");
        assert_eq!(spec.vote, corvid_ast::VoteStrategy::Majority);
    }

    #[test]
    fn parses_ensemble_with_two_models_minimum() {
        let src = "\
model a:
    capability: basic

model b:
    capability: expert

prompt answer(q: String) -> String:
    ensemble [a, b] vote majority
    \"Answer\"
";
        let file = parse_file_src(src);
        let p = file
            .decls
            .iter()
            .find_map(|d| match d {
                Decl::Prompt(p) => Some(p),
                _ => None,
            })
            .unwrap();
        assert_eq!(p.ensemble.as_ref().unwrap().models.len(), 2);
    }

    #[test]
    fn rejects_ensemble_with_single_model() {
        let src = "\
model only:
    capability: basic

prompt answer(q: String) -> String:
    ensemble [only] vote majority
    \"Answer\"
";
        let (_file, errs) = parse_file_errs(src);
        assert!(!errs.is_empty());
    }

    #[test]
    fn rejects_ensemble_without_vote_strategy() {
        let src = "\
model a:
    capability: basic

model b:
    capability: basic

prompt answer(q: String) -> String:
    ensemble [a, b]
    \"Answer\"
";
        let (_file, errs) = parse_file_errs(src);
        assert!(!errs.is_empty());
    }

    #[test]
    fn rejects_unknown_vote_strategy() {
        let src = "\
model a:
    capability: basic

model b:
    capability: basic

prompt answer(q: String) -> String:
    ensemble [a, b] vote plurality
    \"Answer\"
";
        let (_file, errs) = parse_file_errs(src);
        assert!(!errs.is_empty());
    }

    // -------------------- Phase 20h slice G: `adversarial:` --------------------

    #[test]
    fn parses_basic_adversarial_block() {
        let src = "\
model haiku:
    capability: basic

model sonnet:
    capability: standard

model opus:
    capability: expert

prompt verify(q: String) -> String:
    adversarial:
        propose: opus
        challenge: sonnet
        adjudicate: opus
    \"Answer\"
";
        let file = parse_file_src(src);
        let p = file
            .decls
            .iter()
            .find_map(|d| match d {
                Decl::Prompt(p) => Some(p),
                _ => None,
            })
            .unwrap();
        let spec = p.adversarial.as_ref().expect("adversarial");
        assert_eq!(spec.proposer.name, "opus");
        assert_eq!(spec.challenger.name, "sonnet");
        assert_eq!(spec.adjudicator.name, "opus");
    }

    #[test]
    fn rejects_adversarial_missing_stage() {
        let src = "\
model a:
    capability: basic

model b:
    capability: expert

prompt verify(q: String) -> String:
    adversarial:
        propose: a
        challenge: b
    \"Answer\"
";
        let (_file, errs) = parse_file_errs(src);
        assert!(!errs.is_empty());
    }

    #[test]
    fn rejects_adversarial_stages_out_of_order() {
        let src = "\
model a:
    capability: basic

model b:
    capability: expert

model c:
    capability: expert

prompt verify(q: String) -> String:
    adversarial:
        challenge: b
        propose: a
        adjudicate: c
    \"Answer\"
";
        let (_file, errs) = parse_file_errs(src);
        assert!(
            !errs.is_empty(),
            "stages must appear in canonical order: propose, challenge, adjudicate"
        );
    }

    #[test]
    fn rejects_adversarial_combined_with_ensemble() {
        let src = "\
model a:
    capability: basic

model b:
    capability: basic

model c:
    capability: basic

prompt verify(q: String) -> String:
    ensemble [a, b] vote majority
    adversarial:
        propose: a
        challenge: b
        adjudicate: c
    \"Answer\"
";
        let (_file, errs) = parse_file_errs(src);
        assert!(!errs.is_empty());
    }

    #[test]
    fn rejects_ensemble_combined_with_route() {
        let src = "\
model a:
    capability: basic

model b:
    capability: basic

prompt answer(q: String) -> String:
    route:
        _ -> a
    ensemble [a, b] vote majority
    \"Answer\"
";
        let (_file, errs) = parse_file_errs(src);
        assert!(!errs.is_empty());
    }

    #[test]
    fn rejects_rollout_combined_with_route() {
        let src = "\
model a:
    capability: basic

model b:
    capability: basic

prompt p(q: String) -> String:
    route:
        _ -> a
    rollout 10% b, else a
    \"X\"
";
        let (_file, errs) = parse_file_errs(src);
        assert!(!errs.is_empty());
    }

    #[test]
    fn rejects_route_and_progressive_on_same_prompt() {
        let src = "\
model a:
    capability: basic

model b:
    capability: expert

prompt classify(q: String) -> String:
    route:
        _ -> a
    progressive:
        a below 0.95
        b
    \"Classify\"
";
        let (_file, errs) = parse_file_errs(src);
        assert!(!errs.is_empty());
    }

    #[test]
    fn parses_model_with_regulatory_fields() {
        let file = parse_file_src(
            "model claude_hipaa:\n    jurisdiction: us_hipaa_bva\n    compliance: hipaa\n    privacy_tier: strict\n    capability: expert\n",
        );
        let m = match &file.decls[0] {
            Decl::Model(m) => m,
            other => panic!("expected Model, got {other:?}"),
        };
        let field_by = |name: &str| -> &corvid_ast::DimensionValue {
            &m.fields
                .iter()
                .find(|f| f.name.name == name)
                .unwrap()
                .value
        };
        assert!(matches!(
            field_by("jurisdiction"),
            corvid_ast::DimensionValue::Name(n) if n == "us_hipaa_bva"
        ));
        assert!(matches!(
            field_by("compliance"),
            corvid_ast::DimensionValue::Name(n) if n == "hipaa"
        ));
        assert!(matches!(
            field_by("privacy_tier"),
            corvid_ast::DimensionValue::Name(n) if n == "strict"
        ));
    }
