//! Tree-walking interpreter for the Corvid IR.
//!
//! Two roles:
//!
//! 1. **Dev tier.** During development, `corvid run` dispatches through this
//!    interpreter so changes show up without a native recompile step.
//! 2. **Correctness oracle.** Once the Cranelift native compiler (Phase 12+)
//!    is in flight, compiler output is validated against interpreter output
//!    for every fixture — which is why this tier is async-native, matching
//!    the future native runtime instead of taking the easier sync route.
//!
//! Side-effecting work (tool dispatch, LLM calls, approvals, tracing) is
//! delegated to `corvid-runtime`. The interpreter converts between
//! `Value` and `serde_json::Value` at the boundary (`crate::conv`).
//!
//! See `ARCHITECTURE.md` §4 (pipeline) and `ROADMAP.md` Phase 11.

#![forbid(unsafe_code)]
#![allow(dead_code)]

pub mod conv;
pub mod env;
pub mod errors;
pub mod interp;
pub mod schema;
pub mod value;

pub use conv::{json_to_value, value_to_json, ConvError};
pub use env::Env;
pub use errors::{InterpError, InterpErrorKind};
pub use interp::{bind_and_run_agent, build_struct, run_agent};
pub use schema::schema_for;
pub use value::{StructValue, Value};

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_ast::Span;
    use corvid_ir::lower;
    use corvid_resolve::resolve;
    use corvid_runtime::{ProgrammaticApprover, Runtime, RuntimeError};
    use corvid_syntax::{lex, parse_file};
    use corvid_types::typecheck;
    use serde_json::json;
    use std::sync::Arc;

    /// Compile source text all the way down to IR. Panics on any frontend
    /// error — tests should pass clean programs.
    fn ir_of(src: &str) -> corvid_ir::IrFile {
        let tokens = lex(src).expect("lex");
        let (file, perr) = parse_file(&tokens);
        assert!(perr.is_empty(), "parse: {perr:?}");
        let resolved = resolve(&file);
        assert!(resolved.errors.is_empty(), "resolve: {:?}", resolved.errors);
        let checked = typecheck(&file, &resolved);
        assert!(checked.errors.is_empty(), "typecheck: {:?}", checked.errors);
        lower(&file, &resolved, &checked)
    }

    /// A runtime with no tools, no LLMs, and an always-yes approver.
    /// Suitable for tests that only exercise pure computation.
    fn empty_runtime() -> Runtime {
        Runtime::builder()
            .approver(Arc::new(ProgrammaticApprover::always_yes()))
            .build()
    }

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
            body: IrBlock {
                stmts: vec![if_stmt, fallback],
                span: sp,
            },
            span: sp,
        };
        let ir = IrFile {
            imports: vec![],
            types: vec![],
            tools: vec![],
            prompts: vec![],
            agents: vec![agent],
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
        let empty = Value::Struct(Arc::new(StructValue {
            type_id: ticket_id,
            type_name: "Ticket".into(),
            fields: std::collections::HashMap::new(),
        }));
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

    // ---------------------------- Runtime integration ----------------------

    #[tokio::test]
    async fn tool_call_with_no_handler_surfaces_unknown_tool() {
        let src = "\
tool echo(x: String) -> String

agent caller(s: String) -> String:
    return echo(s)
";
        let ir = ir_of(src);
        let rt = empty_runtime();
        let err = run_agent(&ir, "caller", vec![Value::String(Arc::from("hi"))], &rt)
            .await
            .unwrap_err();
        match err.kind {
            InterpErrorKind::Runtime(RuntimeError::UnknownTool(ref name)) => {
                assert_eq!(name, "echo");
            }
            other => panic!("expected Runtime(UnknownTool), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn tool_call_with_registered_handler_returns_value() {
        let src = "\
tool double(x: Int) -> Int

agent run(n: Int) -> Int:
    return double(n)
";
        let ir = ir_of(src);
        let rt = Runtime::builder()
            .tool("double", |args| async move {
                let n = args[0].as_i64().unwrap();
                Ok(json!(n * 2))
            })
            .build();
        let v = run_agent(&ir, "run", vec![Value::Int(21)], &rt).await.expect("run");
        assert_eq!(v, Value::Int(42));
    }

    #[tokio::test]
    async fn approve_then_dangerous_tool_call_succeeds_with_yes_approver() {
        let src = "\
type Receipt:
    id: String

tool issue_refund(id: String, amount: Float) -> Receipt dangerous

agent run(id: String, amount: Float) -> Receipt:
    approve IssueRefund(id, amount)
    return issue_refund(id, amount)
";
        let ir = ir_of(src);
        let rt = Runtime::builder()
            .tool("issue_refund", |args| async move {
                let id = args[0].as_str().unwrap_or("");
                Ok(json!({"id": id}))
            })
            .approver(Arc::new(ProgrammaticApprover::always_yes()))
            .build();
        let v = run_agent(
            &ir,
            "run",
            vec![Value::String(Arc::from("ord_1")), Value::Float(99.99)],
            &rt,
        )
        .await
        .expect("run");
        match v {
            Value::Struct(s) => {
                assert_eq!(s.type_name, "Receipt");
                assert_eq!(s.fields.get("id").unwrap(), &Value::String(Arc::from("ord_1")));
            }
            other => panic!("expected Receipt struct, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn approve_with_no_approver_denial_surfaces_as_runtime_error() {
        let src = "\
type Receipt:
    id: String

tool issue_refund(id: String, amount: Float) -> Receipt dangerous

agent run(id: String, amount: Float) -> Receipt:
    approve IssueRefund(id, amount)
    return issue_refund(id, amount)
";
        let ir = ir_of(src);
        let rt = Runtime::builder()
            .tool("issue_refund", |_| async move {
                Ok(json!({"id": "should_never_happen"}))
            })
            .approver(Arc::new(ProgrammaticApprover::always_no()))
            .build();
        let err = run_agent(
            &ir,
            "run",
            vec![Value::String(Arc::from("ord_1")), Value::Float(99.99)],
            &rt,
        )
        .await
        .unwrap_err();
        match err.kind {
            InterpErrorKind::Runtime(RuntimeError::ApprovalDenied { ref action }) => {
                assert_eq!(action, "IssueRefund");
            }
            other => panic!("expected Runtime(ApprovalDenied), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn prompt_call_returns_struct_via_mock_adapter() {
        let src = r#"
type Decision:
    should_refund: Bool

prompt decide(reason: String) -> Decision:
    """Decide based on {reason}."""

agent run(reason: String) -> Decision:
    return decide(reason)
"#;
        let ir = ir_of(src);
        let rt = Runtime::builder()
            .llm(Arc::new(
                corvid_runtime::MockAdapter::new("mock-1")
                    .reply("decide", json!({"should_refund": true})),
            ))
            .default_model("mock-1")
            .build();
        let v = run_agent(&ir, "run", vec![Value::String(Arc::from("legit"))], &rt)
            .await
            .expect("run");
        match v {
            Value::Struct(s) => {
                assert_eq!(s.type_name, "Decision");
                assert_eq!(s.fields.get("should_refund").unwrap(), &Value::Bool(true));
            }
            other => panic!("expected Decision struct, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn agent_to_agent_call_recurses() {
        let src = "\
agent inner(n: Int) -> Int:
    return n + 1

agent outer(n: Int) -> Int:
    return inner(n) + inner(n)
";
        let ir = ir_of(src);
        let rt = empty_runtime();
        let v = run_agent(&ir, "outer", vec![Value::Int(10)], &rt).await.expect("run");
        assert_eq!(v, Value::Int(22));
    }

    #[tokio::test]
    async fn span_is_preserved_in_errors() {
        let ir = ir_of("agent bad() -> Int:\n    return 10 / 0\n");
        let rt = empty_runtime();
        let err = run_agent(&ir, "bad", vec![], &rt).await.unwrap_err();
        assert_ne!(err.span, Span::new(0, 0));
    }
}
