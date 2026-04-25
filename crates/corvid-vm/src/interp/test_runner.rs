use super::{Flow, Interpreter};
use crate::errors::{InterpError, InterpErrorKind};
use crate::value::Value;
use corvid_ast::Span;
use corvid_ir::{IrEvalAssert, IrExpr, IrFile, IrTest};
use corvid_runtime::Runtime;

/// Execute one lowered `test` declaration and evaluate its assertions.
///
/// This intentionally lives beside the interpreter rather than in the driver:
/// setup bodies and assertion expressions must use the same evaluator as
/// agents, including tool/prompt dispatch and runtime errors.
pub async fn run_test(
    ir: &IrFile,
    test_name: &str,
    runtime: &Runtime,
) -> Result<TestExecution, InterpError> {
    let test = ir
        .tests
        .iter()
        .find(|test| test.name == test_name)
        .ok_or_else(|| {
            InterpError::new(
                InterpErrorKind::DispatchFailed(format!("no test named `{test_name}`")),
                Span::new(0, 0),
            )
        })?;
    run_test_decl(ir, test, runtime).await
}

pub async fn run_all_tests(ir: &IrFile, runtime: &Runtime) -> Vec<TestExecution> {
    let mut results = Vec::with_capacity(ir.tests.len());
    for test in &ir.tests {
        match run_test_decl(ir, test, runtime).await {
            Ok(result) => results.push(result),
            Err(error) => results.push(TestExecution {
                name: test.name.clone(),
                assertions: Vec::new(),
                setup_error: Some(error.to_string()),
            }),
        }
    }
    results
}

async fn run_test_decl(
    ir: &IrFile,
    test: &IrTest,
    runtime: &Runtime,
) -> Result<TestExecution, InterpError> {
    let mut interp = Interpreter::new(ir, runtime).with_mocks();
    run_test_setup(&mut interp, test).await?;

    let mut assertions = Vec::with_capacity(test.assertions.len());
    for assertion in &test.assertions {
        assertions.push(eval_test_assertion(ir, test, runtime, &mut interp, assertion).await);
    }
    Ok(TestExecution {
        name: test.name.clone(),
        assertions,
        setup_error: None,
    })
}

async fn run_test_setup<'ir>(
    interp: &mut Interpreter<'ir>,
    test: &'ir IrTest,
) -> Result<(), InterpError> {
    match interp.eval_block(&test.body).await? {
        Flow::Normal => Ok(()),
        Flow::Return(_) => Err(InterpError::new(
            InterpErrorKind::Other("test setup returned before assertions".into()),
            test.span,
        )),
        Flow::Break | Flow::Continue => Err(InterpError::new(
            InterpErrorKind::Other("loop control flow escaped test setup".into()),
            test.span,
        )),
    }
}

async fn eval_test_assertion<'ir>(
    ir: &'ir IrFile,
    test: &'ir IrTest,
    runtime: &'ir Runtime,
    interp: &mut Interpreter<'ir>,
    assertion: &'ir IrEvalAssert,
) -> TestAssertionExecution {
    match assertion {
        IrEvalAssert::Value {
            expr,
            confidence,
            runs,
            ..
        } => {
            if confidence.is_some() || runs.is_some() {
                eval_statistical_value_assertion(ir, test, runtime, assertion).await
            } else {
                eval_value_assertion(interp, expr, assertion_label(assertion)).await
            }
        }
        IrEvalAssert::Called { .. }
        | IrEvalAssert::Approved { .. }
        | IrEvalAssert::Cost { .. }
        | IrEvalAssert::Ordering { .. } => TestAssertionExecution {
            label: assertion_label(assertion),
            status: TestAssertionStatus::Unsupported,
            message: Some(
                "trace assertions are reserved for Phase 26-E trace fixtures; this runner does not silently pass them".into(),
            ),
        },
    }
}

async fn eval_statistical_value_assertion<'ir>(
    ir: &'ir IrFile,
    test: &'ir IrTest,
    runtime: &'ir Runtime,
    assertion: &'ir IrEvalAssert,
) -> TestAssertionExecution {
    let IrEvalAssert::Value {
        expr,
        confidence,
        runs,
        ..
    } = assertion else {
        unreachable!("caller only passes value assertions");
    };
    let runs = runs.unwrap_or(1);
    let confidence = confidence.unwrap_or(1.0);
    let mut passed = 0_u64;
    for _ in 0..runs {
        let mut fresh = Interpreter::new(ir, runtime).with_mocks();
        if let Err(error) = run_test_setup(&mut fresh, test).await {
            return TestAssertionExecution {
                label: assertion_label(assertion),
                status: TestAssertionStatus::Error,
                message: Some(format!("statistical setup failed: {error}")),
            };
        }
        match eval_bool_assertion(&mut fresh, expr).await {
            Ok(true) => passed += 1,
            Ok(false) => {}
            Err(error) => {
                return TestAssertionExecution {
                    label: assertion_label(assertion),
                    status: TestAssertionStatus::Error,
                    message: Some(error.to_string()),
                };
            }
        }
    }
    let observed = passed as f64 / runs as f64;
    if observed >= confidence {
        TestAssertionExecution {
            label: assertion_label(assertion),
            status: TestAssertionStatus::Passed,
            message: Some(format!(
                "{passed}/{runs} runs passed; observed confidence {observed:.3} >= required {confidence:.3}"
            )),
        }
    } else {
        TestAssertionExecution {
            label: assertion_label(assertion),
            status: TestAssertionStatus::Failed,
            message: Some(format!(
                "{passed}/{runs} runs passed; observed confidence {observed:.3} < required {confidence:.3}"
            )),
        }
    }
}

async fn eval_value_assertion<'ir>(
    interp: &mut Interpreter<'ir>,
    expr: &'ir IrExpr,
    label: String,
) -> TestAssertionExecution {
    match eval_bool_assertion(interp, expr).await {
        Ok(true) => TestAssertionExecution {
            label,
            status: TestAssertionStatus::Passed,
            message: None,
        },
        Ok(false) => TestAssertionExecution {
            label,
            status: TestAssertionStatus::Failed,
            message: Some("assertion evaluated to false".into()),
        },
        Err(error) => TestAssertionExecution {
            label,
            status: TestAssertionStatus::Error,
            message: Some(error.to_string()),
        },
    }
}

async fn eval_bool_assertion<'ir>(
    interp: &mut Interpreter<'ir>,
    expr: &'ir IrExpr,
) -> Result<bool, InterpError> {
    let value = match interp.eval_expr(expr).await?.into_value() {
        Ok(value) | Err(value) => value,
    };
    match value {
        Value::Bool(value) => Ok(value),
        other => Err(InterpError::new(
            InterpErrorKind::TypeMismatch {
                expected: "Bool".into(),
                got: other.type_name(),
            },
            expr.span,
        )),
    }
}

fn assertion_label(assertion: &IrEvalAssert) -> String {
    match assertion {
        IrEvalAssert::Value { .. } => "assert <expr>".into(),
        IrEvalAssert::Called { name, .. } => format!("assert called {name}"),
        IrEvalAssert::Approved { label, .. } => format!("assert approved {label}"),
        IrEvalAssert::Cost { bound, .. } => format!("assert cost < {bound}"),
        IrEvalAssert::Ordering {
            before_name,
            after_name,
            ..
        } => format!("assert called {before_name} before {after_name}"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestExecution {
    pub name: String,
    pub assertions: Vec<TestAssertionExecution>,
    pub setup_error: Option<String>,
}

impl TestExecution {
    pub fn passed(&self) -> bool {
        self.setup_error.is_none()
            && self
                .assertions
                .iter()
                .all(|assertion| assertion.status == TestAssertionStatus::Passed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestAssertionExecution {
    pub label: String,
    pub status: TestAssertionStatus,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestAssertionStatus {
    Passed,
    Failed,
    Error,
    Unsupported,
}

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_ir::{lower, IrFile};
    use corvid_resolve::resolve;
    use corvid_runtime::Runtime;
    use corvid_syntax::{lex, parse_file};
    use corvid_types::typecheck;

    fn lower_src(src: &str) -> IrFile {
        let tokens = lex(src).expect("lex");
        let (file, parse_errors) = parse_file(&tokens);
        assert!(parse_errors.is_empty(), "parse errors: {parse_errors:?}");
        let resolved = resolve(&file);
        assert!(
            resolved.errors.is_empty(),
            "resolve errors: {:?}",
            resolved.errors
        );
        let checked = typecheck(&file, &resolved);
        assert!(
            checked.errors.is_empty(),
            "type errors: {:?}",
            checked.errors
        );
        lower(&file, &resolved, &checked)
    }

    #[tokio::test]
    async fn fixtures_and_mocks_execute_inside_test_runner() {
        let ir = lower_src(
            r#"
tool lookup_score(id: String) -> Int

fixture order_id() -> String:
    return "ord_42"

mock lookup_score(id: String) -> Int:
    if id == "ord_42":
        return 42
    return 0

test mocked_tool_contract:
    score = lookup_score(order_id())
    assert score == 42
"#,
        );
        let runtime = Runtime::builder().build();
        let results = run_all_tests(&ir, &runtime).await;
        assert_eq!(results.len(), 1);
        assert!(results[0].passed(), "result: {:?}", results[0]);
    }
}
