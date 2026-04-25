use super::{empty_runtime, ir_of};
use crate::{
    run_all_tests, run_all_tests_with_options, run_test, SnapshotOptions, TestAssertionStatus,
    TestRunOptions, TraceFixtureOptions,
};
use corvid_trace_schema::{write_events_to_path, TraceEvent, SCHEMA_VERSION, WRITER_INTERPRETER};

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

#[tokio::test]
async fn test_runner_creates_and_compares_snapshots() {
    let dir = std::env::temp_dir().join(format!(
        "corvid-vm-snapshot-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let ir = ir_of(
        r#"
test snapshot_value:
    value = "stable"
    assert_snapshot value
"#,
    );
    let options = TestRunOptions {
        snapshots: Some(SnapshotOptions {
            root: dir.join(".corvid-snapshots").join("suite"),
            update: false,
        }),
        ..TestRunOptions::default()
    };

    let first = run_all_tests_with_options(&ir, &empty_runtime(), options.clone()).await;
    assert!(first[0].passed());
    assert_eq!(first[0].assertions[0].status, TestAssertionStatus::Updated);

    let second = run_all_tests_with_options(&ir, &empty_runtime(), options).await;
    assert!(second[0].passed());
    assert_eq!(second[0].assertions[0].status, TestAssertionStatus::Passed);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_runner_evaluates_trace_fixture_assertions() {
    let dir = std::env::temp_dir().join(format!(
        "corvid-vm-trace-fixture-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tempdir");
    let trace_path = dir.join("refund.jsonl");
    write_events_to_path(
        &trace_path,
        &[
            TraceEvent::SchemaHeader {
                version: SCHEMA_VERSION,
                writer: WRITER_INTERPRETER.into(),
                commit_sha: None,
                source_path: Some("refund.cor".into()),
                ts_ms: 0,
                run_id: "run-1".into(),
            },
            TraceEvent::RunStarted {
                ts_ms: 1,
                run_id: "run-1".into(),
                agent: "refund_bot".into(),
                args: vec![],
            },
            TraceEvent::ToolCall {
                ts_ms: 2,
                run_id: "run-1".into(),
                tool: "get_order".into(),
                args: vec![serde_json::json!("ord_42")],
            },
            TraceEvent::ApprovalRequest {
                ts_ms: 3,
                run_id: "run-1".into(),
                label: "IssueRefund".into(),
                args: vec![serde_json::json!("ord_42")],
            },
            TraceEvent::ApprovalResponse {
                ts_ms: 4,
                run_id: "run-1".into(),
                label: "IssueRefund".into(),
                approved: true,
            },
            TraceEvent::ModelSelected {
                ts_ms: 5,
                run_id: "run-1".into(),
                prompt: "decide".into(),
                model: "cheap".into(),
                model_version: None,
                capability_required: None,
                capability_picked: None,
                output_format_required: None,
                output_format_picked: None,
                cost_estimate: 0.031,
                arm_index: None,
                stage_index: None,
            },
            TraceEvent::ToolCall {
                ts_ms: 6,
                run_id: "run-1".into(),
                tool: "issue_refund".into(),
                args: vec![serde_json::json!("ord_42")],
            },
            TraceEvent::ProvenanceEdge {
                ts_ms: 7,
                run_id: "run-1".into(),
                node_id: "tool:1".into(),
                parents: vec![],
                op: "tool_call:get_order".into(),
                label: Some("order".into()),
            },
            TraceEvent::RunCompleted {
                ts_ms: 8,
                run_id: "run-1".into(),
                ok: true,
                result: Some(serde_json::json!({"tag": "grounded", "value": "ok", "sources": ["get_order"]})),
                error: None,
            },
        ],
    )
    .expect("write trace");
    let ir = ir_of(
        r#"
tool get_order(id: String) -> String
tool issue_refund(id: String) -> String dangerous

test refund_trace from_trace "refund.jsonl":
    assert called get_order before issue_refund
    assert approved IssueRefund
    assert cost < $0.50
"#,
    );
    let options = TestRunOptions {
        trace_fixtures: Some(TraceFixtureOptions { root: dir.clone() }),
        ..TestRunOptions::default()
    };

    let results = run_all_tests_with_options(&ir, &empty_runtime(), options).await;
    assert_eq!(results.len(), 1);
    assert!(results[0].passed(), "result: {:?}", results[0]);
    assert_eq!(results[0].assertions.len(), 3);

    let failing_ir = ir_of(
        r#"
tool cancel_order(id: String) -> String

test missing_call from_trace "refund.jsonl":
    assert called cancel_order
"#,
    );
    let failing_options = TestRunOptions {
        trace_fixtures: Some(TraceFixtureOptions { root: dir.clone() }),
        ..TestRunOptions::default()
    };
    let failing = run_all_tests_with_options(&failing_ir, &empty_runtime(), failing_options).await;
    assert!(!failing[0].passed(), "result: {:?}", failing[0]);
    assert_eq!(
        failing[0].assertions[0].status,
        TestAssertionStatus::Failed
    );
    let _ = std::fs::remove_dir_all(&dir);
}
