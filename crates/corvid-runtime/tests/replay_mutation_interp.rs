use std::path::Path;
use std::sync::Arc;

use corvid_ir::lower;
use corvid_resolve::resolve;
use corvid_runtime::{
    llm::mock::MockAdapter, ProgrammaticApprover, ReplayMutationReport,
    Runtime, RuntimeError, TraceEvent,
};
use corvid_syntax::{lex, parse_file};
use corvid_trace_schema::read_events_from_path;
use corvid_types::typecheck;
use corvid_vm::{run_agent, InterpErrorKind, Value};
use serde_json::json;

const SIMPLE_SRC: &str = r#"
prompt decide_refund(amount: Int) -> Bool:
    """Should refund {amount}?"""

agent refund_bot() -> Bool:
    return decide_refund(42)
"#;

const DRIFT_SRC: &str = r#"
tool lookup(label: String) -> Int

prompt choose_label() -> String:
    """Choose label."""

agent drift_bot() -> Int:
    label = choose_label()
    score = lookup(label)
    if label == "vip":
        return score + 100
    else:
        return score + 1
"#;

const TWO_STEP_SRC: &str = r#"
prompt classify_first() -> String:
    """First label."""

prompt classify_second(first: String) -> String:
    """Second label from {first}."""

agent two_step() -> String:
    first = classify_first()
    return classify_second(first)
"#;

fn ir_of(src: &str) -> corvid_ir::IrFile {
    let tokens = lex(src).expect("lex");
    let (file, parse_errors) = parse_file(&tokens);
    assert!(parse_errors.is_empty(), "parse errors: {parse_errors:?}");
    let resolved = resolve(&file);
    assert!(resolved.errors.is_empty(), "resolve errors: {:?}", resolved.errors);
    let checked = typecheck(&file, &resolved);
    assert!(checked.errors.is_empty(), "type errors: {:?}", checked.errors);
    lower(&file, &resolved, &checked)
}

fn runtime_for_simple(trace_dir: &Path, decision: bool) -> Runtime {
    Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(
            MockAdapter::new("mock-1").reply("decide_refund", json!(decision)),
        ))
        .default_model("mock-1")
        .trace_to(trace_dir)
        .build()
}

fn runtime_for_drift(trace_dir: &Path, label: &str) -> Runtime {
    let label = label.to_string();
    Runtime::builder()
        .tool("lookup", |_args| async move { Ok(json!(7)) })
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(MockAdapter::new("mock-1").reply("choose_label", json!(label))))
        .default_model("mock-1")
        .trace_to(trace_dir)
        .build()
}

fn runtime_for_two_step(trace_dir: &Path, first: &str, second: &str) -> Runtime {
    Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(
            MockAdapter::new("mock-1")
                .reply("classify_first", json!(first))
                .reply("classify_second", json!(second)),
        ))
        .default_model("mock-1")
        .trace_to(trace_dir)
        .build()
}

async fn run_no_args(
    src: &str,
    entry: &str,
    runtime: &Runtime,
) -> Result<Value, corvid_vm::InterpError> {
    let ir = ir_of(src);
    run_agent(&ir, entry, vec![], runtime).await
}

fn normalize_trace(path: &Path) -> Vec<serde_json::Value> {
    read_events_from_path(path)
        .unwrap()
        .into_iter()
        .map(|event| normalize_value(serde_json::to_value(event).unwrap()))
        .collect()
}

fn normalize_value(mut value: serde_json::Value) -> serde_json::Value {
    match &mut value {
        serde_json::Value::Object(map) => {
            map.remove("run_id");
            map.remove("ts_ms");
            for child in map.values_mut() {
                *child = normalize_value(child.take());
            }
            value
        }
        serde_json::Value::Array(items) => {
            for child in items.iter_mut() {
                *child = normalize_value(child.take());
            }
            value
        }
        _ => value,
    }
}

#[tokio::test]
async fn mutation_at_first_llm_step_changes_final_output() {
    let record_dir = tempfile::tempdir().unwrap();
    let recorded_runtime = runtime_for_simple(record_dir.path(), true);
    let recorded_output = run_no_args(SIMPLE_SRC, "refund_bot", &recorded_runtime)
        .await
        .expect("recorded run");
    assert_eq!(recorded_output, Value::Bool(true));
    let trace_path = recorded_runtime.tracer().path().to_path_buf();
    drop(recorded_runtime);

    let replay_dir = tempfile::tempdir().unwrap();
    let replay_runtime = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(
            MockAdapter::new("mock-1").reply("decide_refund", json!(true)),
        ))
        .default_model("mock-1")
        .mutation_replay_from(&trace_path, 1, json!(false))
        .trace_to(replay_dir.path())
        .build();

    let replay_output = run_no_args(SIMPLE_SRC, "refund_bot", &replay_runtime)
        .await
        .expect("mutation replay");
    assert_eq!(replay_output, Value::Bool(false));
}

#[tokio::test]
async fn mutation_divergence_report_lists_downstream_shape_mismatches() {
    let record_dir = tempfile::tempdir().unwrap();
    let recorded_runtime = runtime_for_drift(record_dir.path(), "std");
    let recorded_output = run_no_args(DRIFT_SRC, "drift_bot", &recorded_runtime)
        .await
        .expect("recorded run");
    assert_eq!(recorded_output, Value::Int(8));
    let trace_path = recorded_runtime.tracer().path().to_path_buf();
    drop(recorded_runtime);

    let replay_dir = tempfile::tempdir().unwrap();
    let replay_runtime = Runtime::builder()
        .tool("lookup", |_args| async move { Ok(json!(7)) })
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(
            MockAdapter::new("mock-1").reply("choose_label", json!("std")),
        ))
        .default_model("mock-1")
        .mutation_replay_from(&trace_path, 1, json!("vip"))
        .trace_to(replay_dir.path())
        .build();

    let replay_output = run_no_args(DRIFT_SRC, "drift_bot", &replay_runtime)
        .await
        .expect("mutation replay");
    assert_eq!(replay_output, Value::Int(107));

    let report: ReplayMutationReport = replay_runtime.replay_mutation_report().unwrap();
    assert_eq!(report.divergences.len(), 1);
    assert_eq!(report.divergences[0].step, 2);
    assert_eq!(report.divergences[0].kind, "tool_call");
    assert_eq!(
        normalize_value(report.divergences[0].recorded.clone()),
        normalize_value(
            serde_json::to_value(TraceEvent::ToolCall {
                ts_ms: 0,
                run_id: String::new(),
                tool: "lookup".into(),
                args: vec![json!("std")],
            })
            .unwrap()
        )
    );
    assert_eq!(
        report.divergences[0].got,
        json!({
            "tool": "lookup",
            "args": ["vip"],
        })
    );
    assert!(report.run_completion_divergence.is_some());
}

#[tokio::test]
async fn mutation_at_step_zero_or_out_of_range_returns_clean_error() {
    let record_dir = tempfile::tempdir().unwrap();
    let recorded_runtime = runtime_for_simple(record_dir.path(), true);
    let _ = run_no_args(SIMPLE_SRC, "refund_bot", &recorded_runtime)
        .await
        .expect("recorded run");
    let trace_path = recorded_runtime.tracer().path().to_path_buf();
    drop(recorded_runtime);

    for step in [0usize, 99usize] {
        let replay_runtime = Runtime::builder()
            .approver(Arc::new(ProgrammaticApprover::always_yes()))
            .llm(Arc::new(
                MockAdapter::new("mock-1").reply("decide_refund", json!(true)),
            ))
            .default_model("mock-1")
            .mutation_replay_from(&trace_path, step, json!(false))
            .build();

        let err = run_no_args(SIMPLE_SRC, "refund_bot", &replay_runtime)
            .await
            .expect_err("invalid mutation step should fail");
        match err.kind {
            InterpErrorKind::Runtime(RuntimeError::InvalidReplayMutation { step: err_step, .. }) => {
                assert_eq!(err_step, step);
            }
            other => panic!("expected InvalidReplayMutation, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn mutation_replacement_with_wrong_json_shape_surfaces_typed_error() {
    let record_dir = tempfile::tempdir().unwrap();
    let recorded_runtime = runtime_for_drift(record_dir.path(), "std");
    let _ = run_no_args(DRIFT_SRC, "drift_bot", &recorded_runtime)
        .await
        .expect("recorded run");
    let trace_path = recorded_runtime.tracer().path().to_path_buf();
    drop(recorded_runtime);

    let replay_runtime = Runtime::builder()
        .tool("lookup", |_args| async move { Ok(json!(7)) })
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(
            MockAdapter::new("mock-1").reply("choose_label", json!("std")),
        ))
        .default_model("mock-1")
        .mutation_replay_from(&trace_path, 1, json!({ "label": "vip" }))
        .build();

    let err = run_no_args(DRIFT_SRC, "drift_bot", &replay_runtime)
        .await
        .expect_err("wrong-shape mutation should fail");
    match err.kind {
        InterpErrorKind::Runtime(RuntimeError::InvalidReplayMutation { step, .. }) => {
            assert_eq!(step, 1);
        }
        other => panic!("expected InvalidReplayMutation, got {other:?}"),
    }
}

#[tokio::test]
async fn mutation_preserves_byte_identity_for_steps_before_step() {
    let record_dir = tempfile::tempdir().unwrap();
    let recorded_runtime = runtime_for_two_step(record_dir.path(), "refund", "approve");
    let _ = run_no_args(TWO_STEP_SRC, "two_step", &recorded_runtime)
        .await
        .expect("recorded run");
    let recorded_trace = recorded_runtime.tracer().path().to_path_buf();
    drop(recorded_runtime);

    let replay_dir = tempfile::tempdir().unwrap();
    let replay_runtime = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(
            MockAdapter::new("mock-1")
                .reply("classify_first", json!("refund"))
                .reply("classify_second", json!("approve")),
        ))
        .default_model("mock-1")
        .mutation_replay_from(&recorded_trace, 2, json!("cancel"))
        .trace_to(replay_dir.path())
        .build();

    let replay_trace = replay_runtime.tracer().path().to_path_buf();
    let replay_output = run_no_args(TWO_STEP_SRC, "two_step", &replay_runtime)
        .await
        .expect("mutation replay");
    assert_eq!(replay_output, Value::String("cancel".into()));
    drop(replay_runtime);

    let recorded = normalize_trace(&recorded_trace);
    let replayed = normalize_trace(&replay_trace);
    let second_step_index = read_events_from_path(&recorded_trace)
        .unwrap()
        .iter()
        .position(|event| {
            matches!(
                event,
                TraceEvent::LlmCall {
                    prompt,
                    ..
                } if prompt == "classify_second"
            )
        })
        .unwrap();
    assert_eq!(recorded[..second_step_index], replayed[..second_step_index]);
}
