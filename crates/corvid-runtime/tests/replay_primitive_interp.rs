use std::path::{Path, PathBuf};
use std::sync::Arc;

use corvid_ir::lower;
use corvid_resolve::resolve;
use corvid_runtime::{
    llm::mock::MockAdapter, ProgrammaticApprover, Runtime, RuntimeError, TraceEvent,
};
use corvid_syntax::{lex, parse_file};
use corvid_trace_schema::{read_events_from_path, write_events_to_path, SCHEMA_VERSION, WRITER_INTERPRETER, WRITER_NATIVE};
use corvid_types::typecheck;
use corvid_vm::{run_agent, InterpErrorKind, Value};
use serde_json::json;

const REPLAY_SRC: &str = r#"
tool get_order(id: String) -> String
tool panic_tool() -> String

prompt classify(order_id: String) -> String:
    """Classify {order_id}"""

agent record_full() -> String:
    approve IssueRefund("ord_42")
    order = get_order("ord_42")
    label = classify(order)
    return label

agent record_tools_only() -> String:
    return get_order("ord_42")

agent replay_llm(trace: String) -> String:
    return replay trace:
        when llm("classify") as recorded -> recorded
        else "else"

agent replay_no_match(trace: String) -> String:
    return replay trace:
        when llm("classify") -> "matched"
        else "else"

agent replay_first_match(trace: String) -> String:
    return replay trace:
        when llm("classify") -> "a"
        when llm("classify") -> panic_tool()
        else "else"

agent replay_tool_arg(trace: String) -> String:
    return replay trace:
        when tool("get_order", ticket) -> ticket
        else "else"

agent replay_approve(trace: String) -> Bool:
    return replay trace:
        when approve("IssueRefund") as verdict -> verdict
        else false
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

fn replay_runtime(trace_dir: Option<&Path>) -> Runtime {
    let mut builder = Runtime::builder()
        .tool("get_order", |args| async move {
            let id = args.first().and_then(|v| v.as_str()).unwrap_or("");
            Ok(json!(id))
        })
        .tool("panic_tool", |_args| async move {
            panic!("panic_tool should not be called when replay first-match-wins")
        })
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(
            MockAdapter::new("mock-1").reply("classify", json!("refund")),
        ))
        .default_model("mock-1");
    if let Some(trace_dir) = trace_dir {
        builder = builder.trace_to(trace_dir);
    }
    builder.build()
}

async fn run_agent_no_args(src: &str, agent: &str, runtime: &Runtime) -> Result<Value, corvid_vm::InterpError> {
    let ir = ir_of(src);
    run_agent(&ir, agent, vec![], runtime).await
}

async fn run_agent_with_trace(
    src: &str,
    agent: &str,
    trace_path: &Path,
    runtime: &Runtime,
) -> Result<Value, corvid_vm::InterpError> {
    let ir = ir_of(src);
    run_agent(
        &ir,
        agent,
        vec![Value::String(Arc::from(trace_path.display().to_string()))],
        runtime,
    )
    .await
}

async fn record_trace(agent: &str) -> PathBuf {
    let trace_dir = tempfile::tempdir().unwrap().keep();
    let runtime = replay_runtime(Some(&trace_dir));
    let _ = run_agent_no_args(REPLAY_SRC, agent, &runtime)
        .await
        .expect("recording run should succeed");
    let path = runtime.tracer().path().to_path_buf();
    drop(runtime);
    path
}

fn empty_trace(path: &Path, writer: &str) {
    write_events_to_path(
        path,
        &[
            TraceEvent::SchemaHeader {
                version: SCHEMA_VERSION,
                writer: writer.to_string(),
                commit_sha: None,
                source_path: None,
                ts_ms: 0,
                run_id: "r-empty".into(),
            },
            TraceEvent::RunStarted {
                ts_ms: 1,
                run_id: "r-empty".into(),
                agent: "noop".into(),
                args: vec![],
            },
            TraceEvent::RunCompleted {
                ts_ms: 2,
                run_id: "r-empty".into(),
                ok: true,
                result: Some(json!(null)),
                error: None,
            },
        ],
    )
    .unwrap();
}

#[tokio::test]
async fn replay_finds_first_matching_llm_arm_and_executes_body_with_capture() {
    let recorded = record_trace("record_full").await;
    let runtime = replay_runtime(None);
    let value = run_agent_with_trace(REPLAY_SRC, "replay_llm", &recorded, &runtime)
        .await
        .expect("replay primitive should succeed");
    assert_eq!(value, Value::String(Arc::from("refund")));
}

#[tokio::test]
async fn replay_no_matching_event_falls_through_to_else() {
    let recorded = record_trace("record_tools_only").await;
    let runtime = replay_runtime(None);
    let value = run_agent_with_trace(REPLAY_SRC, "replay_no_match", &recorded, &runtime)
        .await
        .expect("replay primitive should fall through");
    assert_eq!(value, Value::String(Arc::from("else")));
}

#[tokio::test]
async fn replay_first_match_wins_even_when_later_arm_also_matches() {
    let recorded = record_trace("record_full").await;
    let runtime = replay_runtime(None);
    let value = run_agent_with_trace(REPLAY_SRC, "replay_first_match", &recorded, &runtime)
        .await
        .expect("first arm should win");
    assert_eq!(value, Value::String(Arc::from("a")));
}

#[tokio::test]
async fn replay_tool_arm_binds_tool_arg_capture_to_recorded_value() {
    let recorded = record_trace("record_tools_only").await;
    let runtime = replay_runtime(None);
    let value = run_agent_with_trace(REPLAY_SRC, "replay_tool_arg", &recorded, &runtime)
        .await
        .expect("tool arg capture should bind");
    assert_eq!(value, Value::String(Arc::from("ord_42")));
}

#[tokio::test]
async fn replay_approve_arm_binds_verdict_capture_as_bool() {
    let recorded = record_trace("record_full").await;
    let runtime = replay_runtime(None);
    let value = run_agent_with_trace(REPLAY_SRC, "replay_approve", &recorded, &runtime)
        .await
        .expect("approval capture should bind");
    assert_eq!(value, Value::Bool(true));
}

#[tokio::test]
async fn replay_on_empty_trace_executes_else() {
    let temp = tempfile::tempdir().unwrap();
    let trace_path = temp.path().join("empty.jsonl");
    empty_trace(&trace_path, WRITER_INTERPRETER);
    let runtime = replay_runtime(None);
    let value = run_agent_with_trace(REPLAY_SRC, "replay_no_match", &trace_path, &runtime)
        .await
        .expect("empty trace should fall through");
    assert_eq!(value, Value::String(Arc::from("else")));
}

#[tokio::test]
async fn replay_on_malformed_trace_returns_typed_error() {
    let temp = tempfile::tempdir().unwrap();
    let trace_path = temp.path().join("malformed.jsonl");
    std::fs::write(&trace_path, "{not-json}\n").unwrap();
    let runtime = replay_runtime(None);
    let err = run_agent_with_trace(REPLAY_SRC, "replay_no_match", &trace_path, &runtime)
        .await
        .expect_err("malformed trace should fail");
    assert!(matches!(
        err.kind,
        InterpErrorKind::Runtime(RuntimeError::ReplayTraceLoad { .. })
    ));
}

#[tokio::test]
async fn replay_cross_tier_trace_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let trace_path = temp.path().join("native.jsonl");
    empty_trace(&trace_path, WRITER_NATIVE);
    let runtime = replay_runtime(None);
    let err = run_agent_with_trace(REPLAY_SRC, "replay_no_match", &trace_path, &runtime)
        .await
        .expect_err("cross-tier trace should fail");
    assert!(matches!(
        err.kind,
        InterpErrorKind::Runtime(RuntimeError::CrossTierReplayUnsupported { .. })
    ));
}

#[tokio::test]
async fn recorded_trace_contains_expected_substitutable_events() {
    let recorded = record_trace("record_full").await;
    let events = read_events_from_path(&recorded).unwrap();
    assert!(events.iter().any(|event| matches!(event, TraceEvent::ApprovalRequest { .. })));
    assert!(events.iter().any(|event| matches!(event, TraceEvent::ToolCall { .. })));
    assert!(events.iter().any(|event| matches!(event, TraceEvent::LlmCall { .. })));
}
