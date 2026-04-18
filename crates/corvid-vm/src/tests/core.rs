use super::*;

// ----------------------------- Value tests ------------------------------

#[test]
fn value_equality_is_structural() {
    let a = Value::String(Arc::from("hi"));
    let b = Value::String(Arc::from("hi"));
    assert_eq!(a, b);
    assert_ne!(a, Value::String(Arc::from("bye")));
}

#[test]
fn numeric_equality_crosses_int_and_float() {
    assert_eq!(Value::Int(3), Value::Float(3.0));
    assert_eq!(Value::Float(3.0), Value::Int(3));
    assert_ne!(Value::Int(3), Value::Float(3.5));
}

// -------------------------- Literal & arithmetic ------------------------

#[tokio::test]
async fn returns_integer_literal() {
    let ir = ir_of("agent answer() -> Int:\n    return 42\n");
    let rt = empty_runtime();
    let v = run_agent(&ir, "answer", vec![], &rt).await.expect("run");
    assert_eq!(v, Value::Int(42));
}

#[tokio::test]
async fn arithmetic_follows_precedence() {
    let ir = ir_of("agent calc() -> Int:\n    return 1 + 2 * 3\n");
    let rt = empty_runtime();
    let v = run_agent(&ir, "calc", vec![], &rt).await.expect("run");
    assert_eq!(v, Value::Int(7));
}

#[tokio::test]
async fn division_by_zero_is_a_runtime_error() {
    let ir = ir_of("agent bad() -> Int:\n    x = 0\n    return 10 / x\n");
    let rt = empty_runtime();
    let err = run_agent(&ir, "bad", vec![], &rt).await.unwrap_err();
    assert!(matches!(err.kind, InterpErrorKind::Arithmetic(ref m) if m.contains("division")));
}

#[tokio::test]
async fn integer_overflow_is_a_runtime_error() {
    let src = "agent oops() -> Int:\n    return 9223372036854775807 + 1\n";
    let ir = ir_of(src);
    let rt = empty_runtime();
    let err = run_agent(&ir, "oops", vec![], &rt).await.unwrap_err();
    assert!(matches!(err.kind, InterpErrorKind::Arithmetic(ref m) if m.contains("overflow")));
}

#[tokio::test]
async fn int_float_mixing_widens_to_float() {
    let ir = ir_of("agent mix() -> Float:\n    return 3 + 0.5\n");
    let rt = empty_runtime();
    let v = run_agent(&ir, "mix", vec![], &rt).await.expect("run");
    assert_eq!(v, Value::Float(3.5));
}

#[tokio::test]
async fn strings_concatenate_with_plus() {
    let ir = ir_of("agent hi() -> String:\n    return \"hello \" + \"world\"\n");
    let rt = empty_runtime();
    let v = run_agent(&ir, "hi", vec![], &rt).await.expect("run");
    assert_eq!(v, Value::String(Arc::from("hello world")));
}

// ------------------------------ Control flow ---------------------------

#[tokio::test]
async fn if_true_takes_then_branch() {
    let src = "\
agent pick(flag: Bool) -> Int:
    if flag:
        return 1
    else:
        return 2
";
    let ir = ir_of(src);
    let rt = empty_runtime();
    let v = run_agent(&ir, "pick", vec![Value::Bool(true)], &rt).await.expect("run");
    assert_eq!(v, Value::Int(1));
}

#[tokio::test]
async fn if_false_takes_else_branch() {
    let src = "\
agent pick(flag: Bool) -> Int:
    if flag:
        return 1
    else:
        return 2
";
    let ir = ir_of(src);
    let rt = empty_runtime();
    let v = run_agent(&ir, "pick", vec![Value::Bool(false)], &rt).await.expect("run");
    assert_eq!(v, Value::Int(2));
}

#[tokio::test]
async fn if_non_bool_condition_is_defensive_runtime_error() {
    // The type checker normally catches this. We construct IR by hand
    // to prove the interpreter's defensive branch still produces a
    // clean TypeMismatch if something ever bypasses the checker.
    use corvid_ir::{
        IrAgent, IrBlock, IrExpr, IrExprKind, IrFile, IrLiteral, IrStmt,
    };
    use corvid_resolve::DefId;
    use corvid_types::Type;

    let sp = Span::new(0, 0);
    let cond = IrExpr {
        kind: IrExprKind::Literal(IrLiteral::Int(1)),
        ty: Type::Int,
        span: sp,
    };
    let if_stmt = IrStmt::If {
        cond,
        then_block: IrBlock {
            stmts: vec![IrStmt::Return {
                value: Some(IrExpr {
                    kind: IrExprKind::Literal(IrLiteral::Int(1)),
                    ty: Type::Int,
                    span: sp,
                }),
                span: sp,
            }],
            span: sp,
        },
        else_block: None,
        span: sp,
    };
    let fallback = IrStmt::Return {
        value: Some(IrExpr {
            kind: IrExprKind::Literal(IrLiteral::Int(0)),
            ty: Type::Int,
            span: sp,
        }),
        span: sp,
    };
    let agent = IrAgent {
        id: DefId(0),
        name: "bad".into(),
        params: vec![],
        return_ty: Type::Int,
        cost_budget: None,
        body: IrBlock {
            stmts: vec![if_stmt, fallback],
            span: sp,
        },
        span: sp,
        borrow_sig: None,
    };
    let ir = IrFile {
        imports: vec![],
        types: vec![],
        tools: vec![],
        prompts: vec![],
        agents: vec![agent],
        evals: vec![],
    };
    let rt = empty_runtime();
    let err = run_agent(&ir, "bad", vec![], &rt).await.unwrap_err();
    assert!(
        matches!(err.kind, InterpErrorKind::TypeMismatch { .. }),
        "expected TypeMismatch, got {:?}",
        err.kind
    );
}

#[tokio::test]
async fn for_loop_iterates_a_list() {
    let src = "\
agent sum_list() -> Int:
    total = 0
    for x in [1, 2, 3, 4]:
        total = total + x
    return total
";
    let ir = ir_of(src);
    let rt = empty_runtime();
    let v = run_agent(&ir, "sum_list", vec![], &rt).await.expect("run");
    assert_eq!(v, Value::Int(10));
}

#[tokio::test]
async fn break_exits_loop_early() {
    let src = "\
agent early() -> Int:
    total = 0
    for x in [1, 2, 3, 4]:
        if x == 3:
            break
        total = total + x
    return total
";
    let ir = ir_of(src);
    let rt = empty_runtime();
    let v = run_agent(&ir, "early", vec![], &rt).await.expect("run");
    assert_eq!(v, Value::Int(3));
}

#[tokio::test]
async fn continue_skips_to_next_iteration() {
    let src = "\
agent skip_odd() -> Int:
    total = 0
    for x in [1, 2, 3, 4]:
        if x == 3:
            continue
        total = total + x
    return total
";
    let ir = ir_of(src);
    let rt = empty_runtime();
    let v = run_agent(&ir, "skip_odd", vec![], &rt).await.expect("run");
    assert_eq!(v, Value::Int(7));
}

#[tokio::test]
async fn pass_is_a_noop() {
    let src = "\
agent noop(x: Int) -> Int:
    if x > 0:
        pass
    return x
";
    let ir = ir_of(src);
    let rt = empty_runtime();
    let v = run_agent(&ir, "noop", vec![Value::Int(5)], &rt).await.expect("run");
    assert_eq!(v, Value::Int(5));
}

// ------------------------------ Field access ---------------------------

#[tokio::test]
async fn field_access_reads_struct_field() {
    let src = "\
type Ticket:
    order_id: String
    count: Int

agent read(t: Ticket) -> String:
    return t.order_id
";
    let ir = ir_of(src);
    let ticket_id = ir.types[0].id;
    let ticket = build_struct(
        ticket_id,
        "Ticket",
        [
            ("order_id".to_string(), Value::String(Arc::from("ord_42"))),
            ("count".to_string(), Value::Int(3)),
        ],
    );
    let rt = empty_runtime();
    let v = run_agent(&ir, "read", vec![ticket], &rt).await.expect("run");
    assert_eq!(v, Value::String(Arc::from("ord_42")));
}

#[tokio::test]
async fn missing_field_is_runtime_error() {
    let ir = ir_of(
        "type Ticket:\n    order_id: String\n\nagent grab(t: Ticket) -> String:\n    return t.order_id\n",
    );
    let ticket_id = ir.types[0].id;
    let empty = Value::Struct(StructValue::new(
        ticket_id,
        "Ticket",
        std::collections::HashMap::new(),
    ));
    let rt = empty_runtime();
    let err = run_agent(&ir, "grab", vec![empty], &rt).await.unwrap_err();
    assert!(matches!(err.kind, InterpErrorKind::UnknownField { .. }));
}

// ---------------------------- Comparisons / logic ----------------------

#[tokio::test]
async fn comparison_ops() {
    let src = "agent lt(a: Int, b: Int) -> Bool:\n    return a < b\n";
    let ir = ir_of(src);
    let rt = empty_runtime();
    let v = run_agent(&ir, "lt", vec![Value::Int(1), Value::Int(2)], &rt).await.expect("run");
    assert_eq!(v, Value::Bool(true));
    let v = run_agent(&ir, "lt", vec![Value::Int(2), Value::Int(1)], &rt).await.expect("run");
    assert_eq!(v, Value::Bool(false));
}

#[tokio::test]
async fn logical_ops_short_circuit_semantics() {
    let src = "agent both(a: Bool, b: Bool) -> Bool:\n    return a and b\n";
    let ir = ir_of(src);
    let rt = empty_runtime();
    let v = run_agent(
        &ir,
        "both",
        vec![Value::Bool(true), Value::Bool(false)],
        &rt,
    )
    .await
    .expect("run");
    assert_eq!(v, Value::Bool(false));
}

#[tokio::test]
async fn not_negates_bool() {
    let src = "agent nope(b: Bool) -> Bool:\n    return not b\n";
    let ir = ir_of(src);
    let rt = empty_runtime();
    assert_eq!(
        run_agent(&ir, "nope", vec![Value::Bool(true)], &rt).await.expect("run"),
        Value::Bool(false)
    );
}
