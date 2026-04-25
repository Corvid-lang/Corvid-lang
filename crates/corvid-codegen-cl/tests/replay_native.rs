use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use corvid_codegen_cl::build_native_to_disk;
use corvid_ir::lower;
use corvid_resolve::resolve;
use corvid_runtime::{llm::mock::MockAdapter, ProgrammaticApprover, Runtime, TraceEvent};
use corvid_syntax::{lex, parse_file};
use corvid_trace_schema::{read_events_from_path, serialize_event_line, write_events_to_path};
use corvid_types::typecheck;
use corvid_vm::{run_agent, Value};
use serde_json::json;

const REPLAY_NATIVE_SRC: &str = r#"
tool answer() -> Int

prompt decide_refund(amount: Int) -> Bool:
    """Should refund {amount}?"""

agent refund_bot() -> Bool:
    amount = answer()
    approve IssueRefund(amount)
    return decide_refund(amount)
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

fn entry_name(ir: &corvid_ir::IrFile) -> &str {
    if ir.agents.len() == 1 {
        &ir.agents[0].name
    } else {
        ir.agents
            .iter()
            .find(|agent| agent.name == "main")
            .map(|agent| agent.name.as_str())
            .expect("entry agent")
    }
}

fn test_tools_lib_path() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf();
    let name = if cfg!(windows) {
        "corvid_test_tools.lib"
    } else {
        "libcorvid_test_tools.a"
    };
    let path = workspace_root.join("target").join("release").join(name);
    let status = Command::new("cargo")
        .arg("build")
        .arg("-p")
        .arg("corvid-test-tools")
        .arg("--release")
        .current_dir(&workspace_root)
        .status()
        .expect("build corvid-test-tools");
    assert!(status.success(), "building corvid-test-tools failed");
    path
}

fn interpreter_runtime(trace_dir: &Path) -> Runtime {
    Runtime::builder()
        .tool("answer", |_args| async move { Ok(json!(42)) })
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(MockAdapter::new("mock-1").reply("decide_refund", json!(true))))
        .default_model("mock-1")
        .rollout_seed(12345)
        .trace_to(trace_dir)
        .build()
}

async fn run_interpreter(trace_dir: &Path) -> (Value, PathBuf) {
    let runtime = interpreter_runtime(trace_dir);
    let ir = ir_of(REPLAY_NATIVE_SRC);
    let result = run_agent(&ir, entry_name(&ir), vec![], &runtime)
        .await
        .expect("interpreter run");
    let trace_path = runtime.tracer().path().to_path_buf();
    (result, trace_path)
}

fn normalize_event(event: &TraceEvent) -> TraceEvent {
    match event {
        TraceEvent::SchemaHeader {
            version,
            commit_sha,
            source_path,
            ..
        } => TraceEvent::SchemaHeader {
            version: *version,
            writer: "normalized-writer".into(),
            commit_sha: commit_sha.clone(),
            source_path: source_path.clone(),
            ts_ms: 0,
            run_id: "normalized-run".into(),
        },
        TraceEvent::RunStarted { agent, args, .. } => TraceEvent::RunStarted {
            ts_ms: 0,
            run_id: "normalized-run".into(),
            agent: agent.clone(),
            args: args.clone(),
        },
        TraceEvent::RunCompleted {
            ok,
            result,
            error,
            ..
        } => TraceEvent::RunCompleted {
            ts_ms: 0,
            run_id: "normalized-run".into(),
            ok: *ok,
            result: result.clone(),
            error: error.clone(),
        },
        TraceEvent::ToolCall { tool, args, .. } => TraceEvent::ToolCall {
            ts_ms: 0,
            run_id: "normalized-run".into(),
            tool: tool.clone(),
            args: args.clone(),
        },
        TraceEvent::ToolResult { tool, result, .. } => TraceEvent::ToolResult {
            ts_ms: 0,
            run_id: "normalized-run".into(),
            tool: tool.clone(),
            result: result.clone(),
        },
        TraceEvent::LlmCall {
            prompt,
            model,
            model_version,
            rendered,
            args,
            ..
        } => TraceEvent::LlmCall {
            ts_ms: 0,
            run_id: "normalized-run".into(),
            prompt: prompt.clone(),
            model: model.clone(),
            model_version: model_version.clone(),
            rendered: rendered.clone(),
            args: args.clone(),
        },
        TraceEvent::LlmResult {
            prompt,
            model,
            model_version,
            result,
            ..
        } => TraceEvent::LlmResult {
            ts_ms: 0,
            run_id: "normalized-run".into(),
            prompt: prompt.clone(),
            model: model.clone(),
            model_version: model_version.clone(),
            result: result.clone(),
        },
        TraceEvent::ApprovalRequest { label, args, .. } => TraceEvent::ApprovalRequest {
            ts_ms: 0,
            run_id: "normalized-run".into(),
            label: label.clone(),
            args: args.clone(),
        },
        TraceEvent::ApprovalResponse {
            label, approved, ..
        } => TraceEvent::ApprovalResponse {
            ts_ms: 0,
            run_id: "normalized-run".into(),
            label: label.clone(),
            approved: *approved,
        },
        TraceEvent::SeedRead { purpose, value, .. } => TraceEvent::SeedRead {
            ts_ms: 0,
            run_id: "normalized-run".into(),
            purpose: purpose.clone(),
            value: *value,
        },
        TraceEvent::ClockRead { source, value, .. } => TraceEvent::ClockRead {
            ts_ms: 0,
            run_id: "normalized-run".into(),
            source: source.clone(),
            value: *value,
        },
        other => panic!("unexpected event in replay_native fixture: {other:?}"),
    }
}

fn normalize_trace(path: &Path) -> Vec<String> {
    read_events_from_path(path)
        .expect("read trace")
        .iter()
        .map(normalize_event)
        .map(|event| serialize_event_line(&event).expect("serialize normalized event"))
        .collect()
}

fn run_native_binary(
    bin: &Path,
    trace_path: &Path,
    replay_trace: Option<&Path>,
) -> std::process::Output {
    let mut cmd = Command::new(bin);
    cmd.env("CORVID_TRACE_PATH", trace_path)
        .env("CORVID_MODEL", "mock-1")
        .env("CORVID_TEST_MOCK_LLM", "1")
        .env("CORVID_TEST_MOCK_LLM_REPLIES", "{\"decide_refund\":true}")
        .env("CORVID_APPROVE_AUTO", "1")
        .env("CORVID_TEST_TOOL_ANSWER", "42")
        .env("CORVID_ROLLOUT_SEED", "12345");
    if let Some(replay_trace) = replay_trace {
        cmd.env("CORVID_REPLAY_TRACE_PATH", replay_trace);
    }
    cmd.output().expect("run native binary")
}

fn build_native_refund_bot(ir: &corvid_ir::IrFile, bin_path: &Path) -> PathBuf {
    build_native_to_disk(
        ir,
        "corvid_replay_native",
        bin_path,
        &[test_tools_lib_path().as_path()],
    )
    .expect("compile native binary")
}

#[test]
fn native_record_then_replay_matches_output_and_trace_shape() {
    let ir = ir_of(REPLAY_NATIVE_SRC);
    let tmp = tempfile::tempdir().unwrap();
    let bin_path = tmp.path().join("refund_native");
    let produced = build_native_refund_bot(&ir, &bin_path);

    let recorded_trace = tmp.path().join("recorded.jsonl");
    let recorded = run_native_binary(&produced, &recorded_trace, None);
    assert!(
        recorded.status.success(),
        "native record run failed: status={:?}\nstdout={}\nstderr={}",
        recorded.status.code(),
        String::from_utf8_lossy(&recorded.stdout),
        String::from_utf8_lossy(&recorded.stderr)
    );

    let replay_trace = tmp.path().join("replayed.jsonl");
    let replayed = run_native_binary(&produced, &replay_trace, Some(&recorded_trace));
    assert!(
        replayed.status.success(),
        "native replay run failed: status={:?}\nstdout={}\nstderr={}",
        replayed.status.code(),
        String::from_utf8_lossy(&replayed.stdout),
        String::from_utf8_lossy(&replayed.stderr)
    );

    assert_eq!(recorded.stdout, replayed.stdout);
    assert_eq!(normalize_trace(&recorded_trace), normalize_trace(&replay_trace));
}

#[test]
fn native_replay_divergence_reports_mutated_step() {
    let ir = ir_of(REPLAY_NATIVE_SRC);
    let tmp = tempfile::tempdir().unwrap();
    let bin_path = tmp.path().join("refund_native");
    let produced = build_native_refund_bot(&ir, &bin_path);

    let recorded_trace = tmp.path().join("recorded.jsonl");
    let recorded = run_native_binary(&produced, &recorded_trace, None);
    assert!(recorded.status.success(), "record run should succeed");

    let mut events = read_events_from_path(&recorded_trace).expect("read recorded trace");
    let tool_step = events
        .iter()
        .position(|event| matches!(event, TraceEvent::ToolCall { .. }))
        .expect("recorded trace should contain a tool call");
    events[tool_step] = match &events[tool_step] {
        TraceEvent::ToolCall {
            ts_ms,
            run_id,
            tool,
            args,
        } => TraceEvent::ToolCall {
            ts_ms: *ts_ms,
            run_id: run_id.clone(),
            tool: format!("{tool}_mutated"),
            args: args.clone(),
        },
        other => panic!("expected tool call, got {other:?}"),
    };
    let mutated_trace = tmp.path().join("mutated.jsonl");
    write_events_to_path(&mutated_trace, &events).expect("write mutated trace");

    let replay_trace = tmp.path().join("mutated-replay.jsonl");
    let replayed = run_native_binary(&produced, &replay_trace, Some(&mutated_trace));
    assert!(
        !replayed.status.success(),
        "mutated replay should fail\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&replayed.stdout),
        String::from_utf8_lossy(&replayed.stderr)
    );
    let stderr = String::from_utf8_lossy(&replayed.stderr);
    assert!(stderr.contains("replay divergence at step"));
    assert!(stderr.contains(&tool_step.to_string()));
}

#[test]
fn native_replay_empty_and_malformed_traces_fail_cleanly() {
    let ir = ir_of(REPLAY_NATIVE_SRC);
    let tmp = tempfile::tempdir().unwrap();
    let bin_path = tmp.path().join("refund_native");
    let produced = build_native_refund_bot(&ir, &bin_path);

    let empty_trace = tmp.path().join("empty.jsonl");
    std::fs::write(&empty_trace, "").unwrap();
    let empty_replay_trace = tmp.path().join("empty-replay.jsonl");
    let empty = run_native_binary(&produced, &empty_replay_trace, Some(&empty_trace));
    assert!(!empty.status.success(), "empty replay should fail");
    assert!(
        String::from_utf8_lossy(&empty.stderr).contains("failed to load replay trace"),
        "stderr should mention trace load failure: {}",
        String::from_utf8_lossy(&empty.stderr)
    );

    let malformed_trace = tmp.path().join("malformed.jsonl");
    std::fs::write(&malformed_trace, "{not-json}\n").unwrap();
    let malformed_replay_trace = tmp.path().join("malformed-replay.jsonl");
    let malformed = run_native_binary(&produced, &malformed_replay_trace, Some(&malformed_trace));
    assert!(!malformed.status.success(), "malformed replay should fail");
    assert!(
        String::from_utf8_lossy(&malformed.stderr).contains("failed to load replay trace"),
        "stderr should mention malformed replay trace: {}",
        String::from_utf8_lossy(&malformed.stderr)
    );
}

#[tokio::test]
async fn native_replay_rejects_interpreter_trace() {
    let trace_dir = tempfile::tempdir().unwrap();
    let (interp_result, interpreter_trace) = run_interpreter(trace_dir.path()).await;
    assert_eq!(interp_result, Value::Bool(true));

    let ir = ir_of(REPLAY_NATIVE_SRC);
    let tmp = tempfile::tempdir().unwrap();
    let bin_path = tmp.path().join("refund_native");
    let produced = build_native_refund_bot(&ir, &bin_path);
    let native_replay_trace = tmp.path().join("native-replay.jsonl");
    let replayed = run_native_binary(&produced, &native_replay_trace, Some(&interpreter_trace));
    assert!(!replayed.status.success(), "cross-tier replay should fail");
    assert!(
        String::from_utf8_lossy(&replayed.stderr)
            .contains("cross-tier replay is not supported in v1"),
        "stderr should mention cross-tier replay rejection: {}",
        String::from_utf8_lossy(&replayed.stderr)
    );
}
