use std::path::{Path, PathBuf};
use std::sync::Arc;

use corvid_ir::lower;
use corvid_resolve::resolve;
use corvid_runtime::{
    llm::mock::MockAdapter, LlmDivergence, ProgrammaticApprover, Runtime, RuntimeError,
    TraceEvent,
};
use corvid_syntax::{lex, parse_file};
use corvid_trace_schema::{read_events_from_path, write_events_to_path};
use corvid_types::typecheck;
use corvid_vm::{run_agent, InterpErrorKind, Value};
use serde_json::json;

const SIMPLE_SRC: &str = r#"
tool answer() -> Int

prompt decide_refund(amount: Int) -> Bool:
    """Should refund {amount}?"""

agent refund_bot() -> Bool:
    amount = answer()
    return decide_refund(amount)
"#;

const DRIFT_SRC: &str = r#"
tool lookup(label: String) -> Int

prompt choose_label(id: Int) -> String:
    """Choose label for {id}."""

agent drift_bot() -> Int:
    label = choose_label(42)
    score = lookup(label)
    if label == "vip":
        return score + 100
    else:
        return score + 1
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

fn simple_record_runtime(trace_dir: &Path, reply: bool) -> Runtime {
    Runtime::builder()
        .tool("answer", |_args| async move { Ok(json!(42)) })
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(
            MockAdapter::new("mock-1").reply("decide_refund", json!(reply)),
        ))
        .default_model("mock-1")
        .trace_to(trace_dir)
        .build()
}

fn simple_differential_runtime(
    trace_path: &Path,
    trace_dir: &Path,
    live_reply: bool,
) -> Runtime {
    Runtime::builder()
        .tool("answer", |_args| async move { Ok(json!(42)) })
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(
            MockAdapter::new("mock-1").reply("decide_refund", json!(true)),
        ))
        .llm(Arc::new(
            MockAdapter::new("mock-2").reply("decide_refund", json!(live_reply)),
        ))
        .default_model("mock-1")
        .differential_replay_from(trace_path, "mock-2")
        .trace_to(trace_dir)
        .build()
}

fn drift_record_runtime(trace_dir: &Path) -> Runtime {
    Runtime::builder()
        .tool("lookup", |_args| async move { Ok(json!(7)) })
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(
            MockAdapter::new("mock-1").reply("choose_label", json!("std")),
        ))
        .default_model("mock-1")
        .trace_to(trace_dir)
        .build()
}

fn drift_differential_runtime(trace_path: &Path, trace_dir: &Path) -> Runtime {
    Runtime::builder()
        .tool("lookup", |_args| async move { Ok(json!(7)) })
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(
            MockAdapter::new("mock-1").reply("choose_label", json!("std")),
        ))
        .llm(Arc::new(
            MockAdapter::new("mock-2").reply("choose_label", json!("vip")),
        ))
        .default_model("mock-1")
        .differential_replay_from(trace_path, "mock-2")
        .trace_to(trace_dir)
        .build()
}

async fn run_no_args(src: &str, entry: &str, runtime: &Runtime) -> Result<Value, corvid_vm::InterpError> {
    let ir = ir_of(src);
    run_agent(&ir, entry, vec![], runtime).await
}

fn llm_result_step(trace_path: &Path, prompt: &str) -> usize {
    read_events_from_path(trace_path)
        .unwrap()
        .iter()
        .position(|event| {
            matches!(
                event,
                TraceEvent::LlmResult {
                    prompt: expected_prompt,
                    ..
                } if expected_prompt == prompt
            )
        })
        .unwrap()
        + 1
}

fn tool_call_step(trace_path: &Path, tool: &str) -> usize {
    read_events_from_path(trace_path)
        .unwrap()
        .iter()
        .position(|event| {
            matches!(
                event,
                TraceEvent::ToolCall {
                    tool: expected_tool,
                    ..
                } if expected_tool == tool
            )
        })
        .unwrap()
        + 1
}

#[tokio::test]
async fn recorded_result_matches_live_yields_empty_differential_report() {
    let recorded_dir = tempfile::tempdir().unwrap();
    let recorded_runtime = simple_record_runtime(recorded_dir.path(), true);
    let recorded_output = run_no_args(SIMPLE_SRC, "refund_bot", &recorded_runtime)
        .await
        .expect("recorded run");
    let recorded_trace = recorded_runtime.tracer().path().to_path_buf();
    drop(recorded_runtime);

    let replay_dir = tempfile::tempdir().unwrap();
    let replay_runtime = simple_differential_runtime(&recorded_trace, replay_dir.path(), true);
    let replay_output = run_no_args(SIMPLE_SRC, "refund_bot", &replay_runtime)
        .await
        .expect("differential replay");

    assert_eq!(recorded_output, replay_output);
    assert!(replay_runtime
        .replay_differential_report()
        .expect("differential report")
        .is_empty());
}

#[tokio::test]
async fn llm_divergence_surfaces_with_correct_step_and_payloads() {
    let recorded_dir = tempfile::tempdir().unwrap();
    let recorded_runtime = simple_record_runtime(recorded_dir.path(), true);
    let _ = run_no_args(SIMPLE_SRC, "refund_bot", &recorded_runtime)
        .await
        .expect("recorded run");
    let recorded_trace = recorded_runtime.tracer().path().to_path_buf();
    drop(recorded_runtime);

    let replay_dir = tempfile::tempdir().unwrap();
    let replay_runtime = simple_differential_runtime(&recorded_trace, replay_dir.path(), false);
    let replay_output = run_no_args(SIMPLE_SRC, "refund_bot", &replay_runtime)
        .await
        .expect("differential replay");

    assert_eq!(replay_output, Value::Bool(false));
    let report = replay_runtime.replay_differential_report().unwrap();
    assert_eq!(
        report.llm_divergences,
        vec![LlmDivergence {
            step: llm_result_step(&recorded_trace, "decide_refund"),
            prompt: "decide_refund".into(),
            recorded: json!(true),
            live: json!(false),
        }]
    );
}

#[tokio::test]
async fn downstream_drift_keeps_substituting_tools_but_changes_final_result() {
    let recorded_dir = tempfile::tempdir().unwrap();
    let recorded_runtime = drift_record_runtime(recorded_dir.path());
    let recorded_output = run_no_args(DRIFT_SRC, "drift_bot", &recorded_runtime)
        .await
        .expect("recorded run");
    assert_eq!(recorded_output, Value::Int(8));
    let recorded_trace = recorded_runtime.tracer().path().to_path_buf();
    drop(recorded_runtime);

    let replay_dir = tempfile::tempdir().unwrap();
    let replay_runtime = drift_differential_runtime(&recorded_trace, replay_dir.path());
    let replay_output = run_no_args(DRIFT_SRC, "drift_bot", &replay_runtime)
        .await
        .expect("differential replay");

    assert_eq!(replay_output, Value::Int(107));
    let report = replay_runtime.replay_differential_report().unwrap();
    assert_eq!(report.llm_divergences.len(), 1);
    assert_eq!(report.substitution_divergences.len(), 1);
    assert_eq!(
        report.substitution_divergences[0].step,
        tool_call_step(&recorded_trace, "lookup")
    );
    assert!(report.run_completion_divergence.is_some());
}

#[tokio::test]
async fn prompt_mismatch_is_a_replay_divergence_not_a_model_swap_diff() {
    let recorded_dir = tempfile::tempdir().unwrap();
    let recorded_runtime = simple_record_runtime(recorded_dir.path(), true);
    let _ = run_no_args(SIMPLE_SRC, "refund_bot", &recorded_runtime)
        .await
        .expect("recorded run");
    let recorded_trace = recorded_runtime.tracer().path().to_path_buf();
    drop(recorded_runtime);

    let mut events = read_events_from_path(&recorded_trace).unwrap();
    let prompt_index = events
        .iter()
        .position(|event| matches!(event, TraceEvent::LlmCall { .. }))
        .unwrap();
    events[prompt_index] = match &events[prompt_index] {
        TraceEvent::LlmCall {
            ts_ms,
            run_id,
            model,
            model_version,
            rendered,
            args,
            ..
        } => TraceEvent::LlmCall {
            ts_ms: *ts_ms,
            run_id: run_id.clone(),
            prompt: "mutated_prompt".into(),
            model: model.clone(),
            model_version: model_version.clone(),
            rendered: rendered.clone(),
            args: args.clone(),
        },
        other => panic!("expected llm call, got {other:?}"),
    };
    let mutated_dir = tempfile::tempdir().unwrap();
    let mutated_path: PathBuf = mutated_dir.path().join("mutated.jsonl");
    write_events_to_path(&mutated_path, &events).unwrap();

    let replay_dir = tempfile::tempdir().unwrap();
    let replay_runtime = simple_differential_runtime(&mutated_path, replay_dir.path(), false);
    let err = run_no_args(SIMPLE_SRC, "refund_bot", &replay_runtime)
        .await
        .expect_err("prompt mismatch should fail");
    match err.kind {
        InterpErrorKind::Runtime(RuntimeError::ReplayDivergence(divergence)) => {
            assert_eq!(divergence.got_kind, "llm_call");
        }
        other => panic!("expected ReplayDivergence, got {other:?}"),
    }
}
