use super::{empty_runtime, ir_of};
use crate::{run_test, run_all_tests, TestAssertionStatus};

#[tokio::test]
async fn test_runner_executes_setup_and_value_assertion() {
    let ir = ir_of(
        r#"
test arithmetic:
    x = 40 + 2
    assert x == 42
"#,
    );
    let result = run_test(&ir, "arithmetic", &empty_runtime())
        .await
        .expect("run test");

    assert!(result.passed());
    assert_eq!(result.assertions.len(), 1);
    assert_eq!(result.assertions[0].status, TestAssertionStatus::Passed);
}

#[tokio::test]
async fn test_runner_reports_false_assertion() {
    let ir = ir_of(
        r#"
test arithmetic:
    x = 40 + 2
    assert x == 41
"#,
    );
    let result = run_test(&ir, "arithmetic", &empty_runtime())
        .await
        .expect("run test");

    assert!(!result.passed());
    assert_eq!(result.assertions[0].status, TestAssertionStatus::Failed);
}

#[tokio::test]
async fn test_runner_reruns_setup_for_statistical_value_assertion() {
    let ir = ir_of(
        r#"
test stable_math:
    x = 1
    assert x == 1 with confidence 1.0 over 3 runs
"#,
    );
    let result = run_test(&ir, "stable_math", &empty_runtime())
        .await
        .expect("run test");

    assert!(result.passed());
    assert!(result.assertions[0]
        .message
        .as_deref()
        .unwrap_or_default()
        .contains("3/3 runs passed"));
}

#[tokio::test]
async fn test_runner_does_not_silently_pass_trace_assertions() {
    let ir = ir_of(
        r#"
tool get_order(id: String) -> String

test trace_later:
    assert called get_order
"#,
    );
    let result = run_all_tests(&ir, &empty_runtime()).await;

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].assertions[0].status, TestAssertionStatus::Unsupported);
    assert!(!result[0].passed());
}
