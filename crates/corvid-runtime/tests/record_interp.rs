use std::sync::Arc;

use corvid_ir::lower;
use corvid_resolve::resolve;
use corvid_runtime::{
    llm::mock::MockAdapter, ProgrammaticApprover, Runtime, TraceEvent,
};
use corvid_syntax::{lex, parse_file};
use corvid_trace_schema::{read_events_from_path, validate_supported_schema};
use corvid_types::typecheck;
use corvid_vm::{build_struct, run_agent, Value};
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

fn refund_bot_runtime(trace_dir: Option<&std::path::Path>) -> Runtime {
    let mut builder = Runtime::builder()
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
        .default_model("mock-1");
    if let Some(trace_dir) = trace_dir {
        builder = builder.trace_to(trace_dir);
    }
    builder.build()
}

async fn run_refund_bot(runtime: &Runtime) -> Value {
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
    run_agent(&ir, "refund_bot", vec![ticket], runtime)
        .await
        .expect("refund_bot should run")
}

#[tokio::test]
async fn refund_bot_interpreter_run_records_trace_jsonl() {
    let trace_dir = tempfile::tempdir().unwrap();
    let runtime = refund_bot_runtime(Some(trace_dir.path()));
    let result = run_refund_bot(&runtime).await;
    match result {
        Value::Struct(decision) => {
            assert_eq!(decision.type_name(), "Decision");
            assert_eq!(decision.get_field("should_refund").unwrap(), Value::Bool(true));
        }
        other => panic!("expected Decision struct, got {other:?}"),
    }

    let trace_path = runtime.tracer().path().to_path_buf();
    drop(runtime);
    assert!(trace_path.exists(), "trace path should exist: {}", trace_path.display());
    let events = read_events_from_path(&trace_path).expect("trace should deserialize");
    validate_supported_schema(&events).expect("trace schema should validate");
    assert!(!events.is_empty(), "trace should not be empty");

    assert!(matches!(events.first(), Some(TraceEvent::SchemaHeader { .. })));
    assert!(matches!(events.last(), Some(TraceEvent::RunCompleted { .. })));
    assert!(
        events.iter().any(|event| matches!(event, TraceEvent::RunStarted { .. })),
        "trace should contain RunStarted"
    );
    assert!(
        events.iter().any(|event| matches!(event, TraceEvent::ToolCall { .. })),
        "trace should contain ToolCall"
    );
    assert!(
        events.iter().any(|event| matches!(event, TraceEvent::ToolResult { .. })),
        "trace should contain ToolResult"
    );
    assert!(
        events.iter().any(|event| matches!(event, TraceEvent::LlmCall { .. })),
        "trace should contain LlmCall"
    );
    assert!(
        events.iter().any(|event| matches!(event, TraceEvent::LlmResult { .. })),
        "trace should contain LlmResult"
    );
    assert!(
        events.iter().any(|event| matches!(event, TraceEvent::ApprovalRequest { .. })),
        "trace should contain ApprovalRequest"
    );
    assert!(
        events.iter().any(|event| matches!(event, TraceEvent::ApprovalResponse { .. })),
        "trace should contain ApprovalResponse"
    );
}

#[tokio::test]
#[ignore = "manual acceptance measurement for 21-B-rec-interp"]
async fn refund_bot_recording_overhead_smoke() {
    let unrecorded = {
        let runtime = refund_bot_runtime(None);
        let start = std::time::Instant::now();
        for _ in 0..10 {
            let _ = run_refund_bot(&runtime).await;
        }
        start.elapsed()
    };

    let recorded = {
        let trace_dir = tempfile::tempdir().unwrap();
        let runtime = refund_bot_runtime(Some(trace_dir.path()));
        let start = std::time::Instant::now();
        for _ in 0..10 {
            let _ = run_refund_bot(&runtime).await;
        }
        start.elapsed()
    };

    let delta = (recorded.as_secs_f64() / unrecorded.as_secs_f64()) - 1.0;
    eprintln!(
        "recording overhead: unrecorded={:?} recorded={:?} delta={:.2}%",
        unrecorded,
        recorded,
        delta * 100.0
    );
}
