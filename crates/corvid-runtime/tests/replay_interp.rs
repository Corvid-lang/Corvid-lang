use std::path::Path;
use std::sync::Arc;

use corvid_ir::lower;
use corvid_resolve::resolve;
use corvid_runtime::{
    llm::mock::MockAdapter, ProgrammaticApprover, Runtime, RuntimeBuilder, RuntimeError,
    RegisteredModel, TraceEvent,
};
use corvid_syntax::{lex, parse_file};
use corvid_trace_schema::{read_events_from_path, write_events_to_path};
use corvid_types::typecheck;
use corvid_vm::{build_struct, run_agent, InterpErrorKind, Value};
use serde_json::json;

const REFUND_BOT_SRC: &str = include_str!("../../../examples/refund_bot_demo/src/refund_bot.cor");

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

fn refund_bot_builder() -> RuntimeBuilder {
    refund_bot_builder_with_model_version("mock-fixture-v1")
}

fn refund_bot_builder_with_model_version(version: &str) -> RuntimeBuilder {
    Runtime::builder()
        .tool("get_order", |args| async move {
            let id = args.first().and_then(|v| v.as_str()).unwrap_or("");
            Ok(json!({
                "id": id,
                "amount": 49.99,
                "user_id": "user_1",
            }))
        })
        .tool("issue_refund", |args| async move {
            let id = args.first().and_then(|v| v.as_str()).unwrap_or("");
            let amount = args.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
            Ok(json!({
                "refund_id": format!("rf_{id}"),
                "amount": amount,
            }))
        })
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(MockAdapter::new("mock-1").reply(
            "decide_refund",
            json!({
                "should_refund": true,
                "reason": "user reported legitimate complaint",
            }),
        )))
        .default_model("mock-1")
        .model(RegisteredModel::new("mock-1").version(version))
}

fn refund_bot_runtime(trace_dir: Option<&Path>) -> Runtime {
    let mut builder = refund_bot_builder();
    if let Some(trace_dir) = trace_dir {
        builder = builder.trace_to(trace_dir);
    }
    builder.build()
}

fn refund_bot_replay_runtime(trace_path: &Path, trace_dir: Option<&Path>) -> Runtime {
    let mut builder = refund_bot_builder().replay_from(trace_path);
    if let Some(trace_dir) = trace_dir {
        builder = builder.trace_to(trace_dir);
    }
    builder.build()
}

fn refund_bot_replay_runtime_with_model_version(
    trace_path: &Path,
    trace_dir: Option<&Path>,
    version: &str,
) -> Runtime {
    let mut builder = refund_bot_builder_with_model_version(version).replay_from(trace_path);
    if let Some(trace_dir) = trace_dir {
        builder = builder.trace_to(trace_dir);
    }
    builder.build()
}

async fn run_refund_bot(runtime: &Runtime) -> Result<Value, corvid_vm::InterpError> {
    let ir = ir_of(REFUND_BOT_SRC);
    let ticket_id = ir
        .types
        .iter()
        .find(|t| t.name == "Ticket")
        .expect("Ticket type")
        .id;
    let ticket = build_struct(
        ticket_id,
        "Ticket",
        [
            ("order_id".to_string(), Value::String(Arc::from("ord_42"))),
            ("user_id".to_string(), Value::String(Arc::from("user_1"))),
            (
                "message".to_string(),
                Value::String(Arc::from("package arrived broken")),
            ),
        ],
    );
    run_agent(&ir, "refund_bot", vec![ticket], runtime).await
}

fn normalized_events(path: &Path) -> Vec<serde_json::Value> {
    read_events_from_path(path)
        .expect("trace should deserialize")
        .into_iter()
        .map(|event| {
            let mut json = serde_json::to_value(event).expect("trace event should serialize");
            if let serde_json::Value::Object(ref mut object) = json {
                object.remove("run_id");
                object.remove("ts_ms");
                if object.get("kind").and_then(serde_json::Value::as_str)
                    == Some("approval_token_issued")
                {
                    object.insert(
                        "token_id".to_string(),
                        serde_json::Value::String("<token>".into()),
                    );
                    object.insert(
                        "issued_at_ms".to_string(),
                        serde_json::Value::String("<issued>".into()),
                    );
                    object.insert(
                        "expires_at_ms".to_string(),
                        serde_json::Value::String("<expires>".into()),
                    );
                }
            }
            json
        })
        .collect()
}

#[tokio::test]
async fn replay_refund_bot_matches_recorded_output_and_trace_shape() {
    let recorded_dir = tempfile::tempdir().unwrap();
    let recorded_runtime = refund_bot_runtime(Some(recorded_dir.path()));
    let recorded_output = run_refund_bot(&recorded_runtime)
        .await
        .expect("recorded run should succeed");
    let recorded_trace = recorded_runtime.tracer().path().to_path_buf();
    drop(recorded_runtime);

    let replay_dir = tempfile::tempdir().unwrap();
    let replay_runtime = refund_bot_replay_runtime(&recorded_trace, Some(replay_dir.path()));
    let replay_output = run_refund_bot(&replay_runtime)
        .await
        .expect("replay run should succeed");
    let replay_trace = replay_runtime.tracer().path().to_path_buf();
    drop(replay_runtime);

    assert_eq!(recorded_output, replay_output);
    assert_eq!(normalized_events(&recorded_trace), normalized_events(&replay_trace));
}

#[tokio::test]
async fn replay_rejects_same_model_name_with_different_recorded_version() {
    let recorded_dir = tempfile::tempdir().unwrap();
    let recorded_runtime = refund_bot_runtime(Some(recorded_dir.path()));
    let _ = run_refund_bot(&recorded_runtime)
        .await
        .expect("recorded run should succeed");
    let recorded_trace = recorded_runtime.tracer().path().to_path_buf();
    drop(recorded_runtime);

    let replay_dir = tempfile::tempdir().unwrap();
    let replay_runtime = refund_bot_replay_runtime_with_model_version(
        &recorded_trace,
        Some(replay_dir.path()),
        "mock-fixture-v2",
    );
    let err = run_refund_bot(&replay_runtime)
        .await
        .expect_err("model version drift must diverge replay");
    match err.kind {
        InterpErrorKind::Runtime(RuntimeError::ReplayDivergence(_)) => {}
        other => panic!("expected replay divergence, got {other:?}"),
    }
}

#[tokio::test]
async fn replay_divergence_reports_the_mismatched_step() {
    let recorded_dir = tempfile::tempdir().unwrap();
    let recorded_runtime = refund_bot_runtime(Some(recorded_dir.path()));
    let _ = run_refund_bot(&recorded_runtime)
        .await
        .expect("recorded run should succeed");
    let recorded_trace = recorded_runtime.tracer().path().to_path_buf();
    drop(recorded_runtime);

    let mut events = read_events_from_path(&recorded_trace).unwrap();
    let tool_step = events
        .iter()
        .position(|event| matches!(event, TraceEvent::ToolCall { .. }))
        .expect("trace should contain a tool call");
    let original_tool = match &events[tool_step] {
        TraceEvent::ToolCall { tool, .. } => tool.clone(),
        other => panic!("expected tool call, got {other:?}"),
    };
    events[tool_step] = match &events[tool_step] {
        TraceEvent::ToolCall {
            ts_ms,
            run_id,
            args,
            ..
        } => TraceEvent::ToolCall {
            ts_ms: *ts_ms,
            run_id: run_id.clone(),
            tool: format!("{original_tool}_mutated"),
            args: args.clone(),
        },
        other => panic!("expected tool call, got {other:?}"),
    };
    let mutated_dir = tempfile::tempdir().unwrap();
    let mutated_path = mutated_dir.path().join("mutated.jsonl");
    write_events_to_path(&mutated_path, &events).unwrap();

    let replay_runtime = refund_bot_replay_runtime(&mutated_path, None);
    let err = run_refund_bot(&replay_runtime)
        .await
        .expect_err("mutated trace should diverge");
    match err.kind {
        InterpErrorKind::Runtime(RuntimeError::ReplayDivergence(divergence)) => {
            assert_eq!(divergence.step, tool_step);
            assert_eq!(divergence.got_kind, "tool_call");
        }
        other => panic!("expected replay divergence, got {other:?}"),
    }
}

#[tokio::test]
async fn replay_empty_and_malformed_traces_fail_with_typed_errors() {
    let empty_dir = tempfile::tempdir().unwrap();
    let empty_path = empty_dir.path().join("empty.jsonl");
    std::fs::write(&empty_path, "").unwrap();
    let empty_runtime = refund_bot_replay_runtime(&empty_path, None);
    let empty_err = run_refund_bot(&empty_runtime)
        .await
        .expect_err("empty trace should fail");
    assert!(matches!(
        empty_err.kind,
        InterpErrorKind::Runtime(RuntimeError::ReplayTraceLoad { .. })
    ));

    let malformed_dir = tempfile::tempdir().unwrap();
    let malformed_path = malformed_dir.path().join("malformed.jsonl");
    std::fs::write(&malformed_path, "{not-json}\n").unwrap();
    let malformed_runtime = refund_bot_replay_runtime(&malformed_path, None);
    let malformed_err = run_refund_bot(&malformed_runtime)
        .await
        .expect_err("malformed trace should fail");
    assert!(matches!(
        malformed_err.kind,
        InterpErrorKind::Runtime(RuntimeError::ReplayTraceLoad { .. })
    ));
}
