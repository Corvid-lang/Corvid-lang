//! Tree-walking interpreter for the Corvid IR.
//!
//! Two roles:
//!
//! 1. **Dev tier.** During development, `corvid run` dispatches through this
//!    interpreter so changes show up without a native recompile step.
//! 2. **Correctness oracle.** Once the Cranelift native compiler (Phase 12+)
//!    is in flight, compiler output is validated against interpreter output
//!    for every fixture.
//!
//! This crate is deliberately slim. The runtime library (`corvid-runtime`)
//! hosts the HTTP client, LLM adapters, tool registry, and approval flow;
//! the interpreter calls into it at the boundaries between pure Corvid
//! computation and side-effecting operations.
//!
//! See `ARCHITECTURE.md` §4 (pipeline) and `ROADMAP.md` Phase 11.

#![forbid(unsafe_code)]
#![allow(dead_code)]

pub mod env;
pub mod errors;
pub mod interp;
pub mod value;

pub use env::Env;
pub use errors::{InterpError, InterpErrorKind};
pub use interp::{bind_and_run_agent, build_struct, run_agent};
pub use value::{StructValue, Value};

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_ast::Span;
    use corvid_ir::lower;
    use corvid_resolve::resolve;
    use corvid_syntax::{lex, parse_file};
    use corvid_types::typecheck;
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

    #[test]
    fn returns_integer_literal() {
        let ir = ir_of("agent answer() -> Int:\n    return 42\n");
        let v = run_agent(&ir, "answer", vec![]).expect("run");
        assert_eq!(v, Value::Int(42));
    }

    #[test]
    fn arithmetic_follows_precedence() {
        let ir = ir_of("agent calc() -> Int:\n    return 1 + 2 * 3\n");
        let v = run_agent(&ir, "calc", vec![]).expect("run");
        assert_eq!(v, Value::Int(7));
    }

    #[test]
    fn division_by_zero_is_a_runtime_error() {
        let ir = ir_of("agent bad() -> Int:\n    x = 0\n    return 10 / x\n");
        let err = run_agent(&ir, "bad", vec![]).unwrap_err();
        assert!(matches!(err.kind, InterpErrorKind::Arithmetic(ref m) if m.contains("division")));
    }

    #[test]
    fn integer_overflow_is_a_runtime_error() {
        // Max i64 + 1 overflows.
        let src = "agent oops() -> Int:\n    return 9223372036854775807 + 1\n";
        let ir = ir_of(src);
        let err = run_agent(&ir, "oops", vec![]).unwrap_err();
        assert!(matches!(err.kind, InterpErrorKind::Arithmetic(ref m) if m.contains("overflow")));
    }

    #[test]
    fn int_float_mixing_widens_to_float() {
        let ir = ir_of("agent mix() -> Float:\n    return 3 + 0.5\n");
        let v = run_agent(&ir, "mix", vec![]).expect("run");
        assert_eq!(v, Value::Float(3.5));
    }

    #[test]
    fn strings_concatenate_with_plus() {
        let ir = ir_of("agent hi() -> String:\n    return \"hello \" + \"world\"\n");
        let v = run_agent(&ir, "hi", vec![]).expect("run");
        assert_eq!(v, Value::String(Arc::from("hello world")));
    }

    // ------------------------------ Control flow ---------------------------

    #[test]
    fn if_true_takes_then_branch() {
        let src = "\
agent pick(flag: Bool) -> Int:
    if flag:
        return 1
    else:
        return 2
";
        let ir = ir_of(src);
        let v = run_agent(&ir, "pick", vec![Value::Bool(true)]).expect("run");
        assert_eq!(v, Value::Int(1));
    }

    #[test]
    fn if_false_takes_else_branch() {
        let src = "\
agent pick(flag: Bool) -> Int:
    if flag:
        return 1
    else:
        return 2
";
        let ir = ir_of(src);
        let v = run_agent(&ir, "pick", vec![Value::Bool(false)]).expect("run");
        assert_eq!(v, Value::Int(2));
    }

    #[test]
    fn if_non_bool_condition_is_defensive_runtime_error() {
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
        let err = run_agent(&ir, "bad", vec![]).unwrap_err();
        assert!(
            matches!(err.kind, InterpErrorKind::TypeMismatch { .. }),
            "expected TypeMismatch, got {:?}",
            err.kind
        );
    }

    #[test]
    fn for_loop_iterates_a_list() {
        // List literal evaluation test via for-loop sum.
        let src = "\
agent sum_list() -> Int:
    total = 0
    for x in [1, 2, 3, 4]:
        total = total + x
    return total
";
        let ir = ir_of(src);
        let v = run_agent(&ir, "sum_list", vec![]).expect("run");
        assert_eq!(v, Value::Int(10));
    }

    #[test]
    fn break_exits_loop_early() {
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
        let v = run_agent(&ir, "early", vec![]).expect("run");
        assert_eq!(v, Value::Int(3));
    }

    #[test]
    fn continue_skips_to_next_iteration() {
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
        let v = run_agent(&ir, "skip_odd", vec![]).expect("run");
        assert_eq!(v, Value::Int(7));
    }

    #[test]
    fn pass_is_a_noop() {
        let src = "\
agent noop(x: Int) -> Int:
    if x > 0:
        pass
    return x
";
        let ir = ir_of(src);
        let v = run_agent(&ir, "noop", vec![Value::Int(5)]).expect("run");
        assert_eq!(v, Value::Int(5));
    }

    // ------------------------------ Field access ---------------------------

    #[test]
    fn field_access_reads_struct_field() {
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
        let v = run_agent(&ir, "read", vec![ticket]).expect("run");
        assert_eq!(v, Value::String(Arc::from("ord_42")));
    }

    #[test]
    fn missing_field_is_runtime_error() {
        let src = "\
type Ticket:
    order_id: String

agent grab(t: Ticket) -> String:
    return t.nonexistent
";
        // typecheck will catch this statically, but we still want the runtime
        // to produce a clean error if somehow bypassed. Simulate by running
        // the interpreter with a struct that doesn't have the field.
        let ir = ir_of("type Ticket:\n    order_id: String\n\nagent grab(t: Ticket) -> String:\n    return t.order_id\n");
        let ticket_id = ir.types[0].id;
        let empty = Value::Struct(Arc::new(StructValue {
            type_id: ticket_id,
            type_name: "Ticket".into(),
            fields: std::collections::HashMap::new(),
        }));
        let err = run_agent(&ir, "grab", vec![empty]).unwrap_err();
        let _ = src; // keep for documentation
        assert!(matches!(err.kind, InterpErrorKind::UnknownField { .. }));
    }

    // ---------------------------- Comparisons / logic ----------------------

    #[test]
    fn comparison_ops() {
        let src = "agent lt(a: Int, b: Int) -> Bool:\n    return a < b\n";
        let ir = ir_of(src);
        let v = run_agent(&ir, "lt", vec![Value::Int(1), Value::Int(2)]).expect("run");
        assert_eq!(v, Value::Bool(true));
        let v = run_agent(&ir, "lt", vec![Value::Int(2), Value::Int(1)]).expect("run");
        assert_eq!(v, Value::Bool(false));
    }

    #[test]
    fn logical_ops_short_circuit_semantics() {
        // v0.5 evaluates both sides (no short-circuit yet). We test the
        // boolean result only; short-circuit is Phase 12+ when we move
        // to Cranelift-aware lowering.
        let src = "agent both(a: Bool, b: Bool) -> Bool:\n    return a and b\n";
        let ir = ir_of(src);
        let v = run_agent(
            &ir,
            "both",
            vec![Value::Bool(true), Value::Bool(false)],
        )
        .expect("run");
        assert_eq!(v, Value::Bool(false));
    }

    #[test]
    fn not_negates_bool() {
        let src = "agent nope(b: Bool) -> Bool:\n    return not b\n";
        let ir = ir_of(src);
        assert_eq!(
            run_agent(&ir, "nope", vec![Value::Bool(true)]).expect("run"),
            Value::Bool(false)
        );
    }

    // ---------------------------- Not-yet-implemented ----------------------

    #[test]
    fn tool_calls_produce_not_implemented_error() {
        let src = "\
tool echo(x: String) -> String

agent caller(s: String) -> String:
    return echo(s)
";
        let ir = ir_of(src);
        let err =
            run_agent(&ir, "caller", vec![Value::String(Arc::from("hi"))]).unwrap_err();
        assert!(matches!(err.kind, InterpErrorKind::NotImplemented(_)));
    }

    #[test]
    fn span_is_preserved_in_errors() {
        let ir = ir_of("agent bad() -> Int:\n    return 10 / 0\n");
        let err = run_agent(&ir, "bad", vec![]).unwrap_err();
        // The span should point somewhere inside the file, not 0..0.
        assert_ne!(err.span, Span::new(0, 0));
    }
}
