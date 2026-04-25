use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use corvid_codegen_cl::build_native_to_disk;
use corvid_ir::lower;
use corvid_resolve::resolve;
use corvid_runtime::{
    llm::mock::MockAdapter, ProgrammaticApprover, ReplayDifferentialReport, Runtime, TraceEvent,
};
use corvid_syntax::{lex, parse_file};
use corvid_trace_schema::{read_events_from_path, write_events_to_path};
use corvid_types::typecheck;
use corvid_vm::{run_agent, Value};
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
prompt choose_label() -> String:
    """Choose a label."""

agent drift_bot() -> String:
    label = choose_label()
    approve UseLabel(label)
    return label
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
    let workspace_root = workspace_root();
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

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

fn ensure_runtime_staticlib() {
    let status = Command::new("cargo")
        .arg("build")
        .arg("-p")
        .arg("corvid-runtime")
        .current_dir(workspace_root())
        .status()
        .expect("build corvid-runtime");
    assert!(status.success(), "building corvid-runtime failed");
}

fn interpreter_runtime(trace_dir: &Path) -> Runtime {
    Runtime::builder()
        .tool("answer", |_args| async move { Ok(json!(42)) })
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(MockAdapter::new("mock-1").reply("decide_refund", json!(true))))
        .default_model("mock-1")
        .trace_to(trace_dir)
        .build()
}

async fn run_interpreter(trace_dir: &Path) -> (Value, PathBuf) {
    let runtime = interpreter_runtime(trace_dir);
    let ir = ir_of(SIMPLE_SRC);
    let result = run_agent(&ir, entry_name(&ir), vec![], &runtime)
        .await
        .expect("interpreter run");
    let trace_path = runtime.tracer().path().to_path_buf();
    (result, trace_path)
}

fn build_native(ir: &corvid_ir::IrFile, bin_path: &Path) -> PathBuf {
    ensure_runtime_staticlib();
    let tool_lib = if ir
        .tools
        .iter()
        .any(|tool| tool.name.as_str() == "answer")
    {
        Some(test_tools_lib_path())
    } else {
        None
    };
    let libs: Vec<&Path> = tool_lib
        .iter()
        .map(|path| path.as_path())
        .collect();
    build_native_to_disk(ir, "corvid_replay_differential", bin_path, &libs).expect("compile native binary")
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

fn approval_step(trace_path: &Path, label: &str) -> usize {
    read_events_from_path(trace_path)
        .unwrap()
        .iter()
        .position(|event| {
            matches!(
                event,
                TraceEvent::ApprovalRequest {
                    label: expected_label,
                    ..
                } if expected_label == label
            )
        })
        .unwrap()
        + 1
}

fn run_native_binary(
    bin: &Path,
    trace_path: &Path,
    replay_trace: Option<&Path>,
    replay_model: Option<&str>,
    report_path: Option<&Path>,
    replies_json: &str,
) -> std::process::Output {
    let mut cmd = Command::new(bin);
    cmd.env("CORVID_TRACE_PATH", trace_path)
        .env("CORVID_MODEL", "mock-1")
        .env("CORVID_TEST_MOCK_LLM", "1")
        .env("CORVID_TEST_MOCK_LLM_REPLIES", replies_json)
        .env("CORVID_APPROVE_AUTO", "1")
        .env("CORVID_TEST_TOOL_ANSWER", "42");
    if let Some(replay_trace) = replay_trace {
        cmd.env("CORVID_REPLAY_TRACE_PATH", replay_trace);
    }
    if let Some(replay_model) = replay_model {
        cmd.env("CORVID_REPLAY_MODEL", replay_model);
    }
    if let Some(report_path) = report_path {
        cmd.env("CORVID_REPLAY_DIFFERENTIAL_REPORT_PATH", report_path);
    }
    cmd.output().expect("run native binary")
}

fn read_report(path: &Path) -> ReplayDifferentialReport {
    serde_json::from_slice(&std::fs::read(path).expect("read report")).expect("parse report")
}

#[test]
fn native_differential_replay_with_matching_result_has_empty_report() {
    let ir = ir_of(SIMPLE_SRC);
    let tmp = tempfile::tempdir().unwrap();
    let bin_path = tmp.path().join("refund_native");
    let produced = build_native(&ir, &bin_path);

    let recorded_trace = tmp.path().join("recorded.jsonl");
    let recorded = run_native_binary(
        &produced,
        &recorded_trace,
        None,
        None,
        None,
        "{\"decide_refund\":true}",
    );
    assert!(recorded.status.success(), "record run should succeed");

    let replay_trace = tmp.path().join("replayed.jsonl");
    let report_path = tmp.path().join("report.json");
    let replayed = run_native_binary(
        &produced,
        &replay_trace,
        Some(&recorded_trace),
        Some("mock-2"),
        Some(&report_path),
        "{\"decide_refund\":true}",
    );
    assert!(
        replayed.status.success(),
        "differential replay should succeed\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&replayed.stdout),
        String::from_utf8_lossy(&replayed.stderr)
    );
    assert_eq!(recorded.stdout, replayed.stdout);
    assert!(read_report(&report_path).is_empty());
}

#[test]
fn native_differential_llm_divergence_is_reported() {
    let ir = ir_of(SIMPLE_SRC);
    let tmp = tempfile::tempdir().unwrap();
    let bin_path = tmp.path().join("refund_native");
    let produced = build_native(&ir, &bin_path);

    let recorded_trace = tmp.path().join("recorded.jsonl");
    let recorded = run_native_binary(
        &produced,
        &recorded_trace,
        None,
        None,
        None,
        "{\"decide_refund\":true}",
    );
    assert!(recorded.status.success(), "record run should succeed");

    let replay_trace = tmp.path().join("replayed.jsonl");
    let report_path = tmp.path().join("report.json");
    let replayed = run_native_binary(
        &produced,
        &replay_trace,
        Some(&recorded_trace),
        Some("mock-2"),
        Some(&report_path),
        "{\"decide_refund\":false}",
    );
    assert!(replayed.status.success(), "differential replay should succeed");
    let report = read_report(&report_path);
    assert_eq!(report.llm_divergences.len(), 1);
    assert_eq!(report.llm_divergences[0].step, llm_result_step(&recorded_trace, "decide_refund"));
    assert_eq!(report.llm_divergences[0].recorded, json!(true));
    assert_eq!(report.llm_divergences[0].live, json!(false));
}

#[test]
fn native_differential_tracks_downstream_drift_through_approval_substitution() {
    let ir = ir_of(DRIFT_SRC);
    let tmp = tempfile::tempdir().unwrap();
    let bin_path = tmp.path().join("drift_native");
    let produced = build_native(&ir, &bin_path);

    let recorded_trace = tmp.path().join("recorded.jsonl");
    let recorded = run_native_binary(
        &produced,
        &recorded_trace,
        None,
        None,
        None,
        "{\"choose_label\":\"std\"}",
    );
    assert!(recorded.status.success(), "record run should succeed");
    assert!(String::from_utf8_lossy(&recorded.stdout).contains("std"));

    let replay_trace = tmp.path().join("replayed.jsonl");
    let report_path = tmp.path().join("report.json");
    let replayed = run_native_binary(
        &produced,
        &replay_trace,
        Some(&recorded_trace),
        Some("mock-2"),
        Some(&report_path),
        "{\"choose_label\":\"vip\"}",
    );
    assert!(replayed.status.success(), "differential replay should succeed");
    assert!(String::from_utf8_lossy(&replayed.stdout).contains("vip"));

    let report = read_report(&report_path);
    assert_eq!(report.llm_divergences.len(), 1);
    assert_eq!(report.substitution_divergences.len(), 1);
    assert_eq!(
        report.substitution_divergences[0].step,
        approval_step(&recorded_trace, "UseLabel")
    );
    assert!(report.run_completion_divergence.is_some());
}

#[test]
fn native_differential_prompt_mismatch_is_replay_divergence() {
    let ir = ir_of(SIMPLE_SRC);
    let tmp = tempfile::tempdir().unwrap();
    let bin_path = tmp.path().join("refund_native");
    let produced = build_native(&ir, &bin_path);

    let recorded_trace = tmp.path().join("recorded.jsonl");
    let recorded = run_native_binary(
        &produced,
        &recorded_trace,
        None,
        None,
        None,
        "{\"decide_refund\":true}",
    );
    assert!(recorded.status.success(), "record run should succeed");

    let mut events = read_events_from_path(&recorded_trace).unwrap();
    let prompt_step = events
        .iter()
        .position(|event| matches!(event, TraceEvent::LlmCall { .. }))
        .unwrap();
    events[prompt_step] = match &events[prompt_step] {
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
    let mutated_trace = tmp.path().join("mutated.jsonl");
    write_events_to_path(&mutated_trace, &events).unwrap();

    let replay_trace = tmp.path().join("replayed.jsonl");
    let report_path = tmp.path().join("report.json");
    let replayed = run_native_binary(
        &produced,
        &replay_trace,
        Some(&mutated_trace),
        Some("mock-2"),
        Some(&report_path),
        "{\"decide_refund\":false}",
    );
    assert!(!replayed.status.success(), "prompt mismatch should fail");
    assert!(
        String::from_utf8_lossy(&replayed.stderr).contains("replay divergence at step"),
        "stderr should mention replay divergence: {}",
        String::from_utf8_lossy(&replayed.stderr)
    );
}

#[tokio::test]
async fn native_differential_rejects_interpreter_trace() {
    let trace_dir = tempfile::tempdir().unwrap();
    let (interp_result, interpreter_trace) = run_interpreter(trace_dir.path()).await;
    assert_eq!(interp_result, Value::Bool(true));

    let ir = ir_of(SIMPLE_SRC);
    let tmp = tempfile::tempdir().unwrap();
    let bin_path = tmp.path().join("refund_native");
    let produced = build_native(&ir, &bin_path);
    let native_replay_trace = tmp.path().join("native-replay.jsonl");
    let report_path = tmp.path().join("report.json");
    let replayed = run_native_binary(
        &produced,
        &native_replay_trace,
        Some(&interpreter_trace),
        Some("mock-2"),
        Some(&report_path),
        "{\"decide_refund\":false}",
    );
    assert!(!replayed.status.success(), "cross-tier replay should fail");
    assert!(
        String::from_utf8_lossy(&replayed.stderr)
            .contains("cross-tier replay is not supported in v1"),
        "stderr should mention cross-tier replay rejection: {}",
        String::from_utf8_lossy(&replayed.stderr)
    );
}
