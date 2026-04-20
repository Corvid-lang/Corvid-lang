//! Driver-level end-to-end tests for the fresh-record-from-trace
//! orchestrator used by the `--promote` path of
//! `corvid test --from-traces`.
//!
//! These exercise [`corvid_driver::run_fresh_from_source_async`]:
//! record a trace by running a tiny Corvid source through a mock
//! adapter, then invoke the fresh-record helper against an `emit_dir`
//! and assert that a new `.jsonl` appears there with the expected
//! shape (same agent, same args, current-source behavior).

use std::sync::Arc;

use corvid_driver::{compile_to_ir, run_fresh_from_source_async, run_ir_with_runtime};
use corvid_runtime::{llm::mock::MockAdapter, ProgrammaticApprover, Runtime};
use corvid_trace_schema::{read_events_from_path, TraceEvent};
use serde_json::json;

const SRC: &str = r#"
prompt decide_refund(amount: Int) -> Bool:
    """Should we refund {amount}?"""

agent refund_bot() -> Bool:
    return decide_refund(42)
"#;

fn write_source_tempfile(dir: &std::path::Path, source: &str) -> std::path::PathBuf {
    let path = dir.join("refund_bot.cor");
    std::fs::write(&path, source).unwrap();
    path
}

fn record_trace(source_path: &std::path::Path, trace_dir: &std::path::Path, reply: bool) {
    let source = std::fs::read_to_string(source_path).unwrap();
    let ir = compile_to_ir(&source).expect("recorded source compiles");
    let runtime = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(
            MockAdapter::new("mock-1").reply("decide_refund", json!(reply)),
        ))
        .default_model("mock-1")
        .trace_to(trace_dir)
        .build();
    let tokio_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    tokio_rt
        .block_on(run_ir_with_runtime(&ir, Some("refund_bot"), vec![], &runtime))
        .expect("recorded run succeeds");
    drop(runtime);
}

fn trace_file_in(trace_dir: &std::path::Path) -> std::path::PathBuf {
    for entry in std::fs::read_dir(trace_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            return path;
        }
    }
    panic!("no .jsonl file in {}", trace_dir.display());
}

/// Promote-mode runtime builder: registers the *current* mock adapter
/// (what the live code should say now), plus the default_model the
/// trace's LlmCall.model field expects. Real CLI calls use
/// env-driven real adapters; tests inject mocks via this helper.
fn promote_builder(mock_reply: bool) -> corvid_runtime::RuntimeBuilder {
    Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(
            MockAdapter::new("mock-1").reply("decide_refund", json!(mock_reply)),
        ))
        .default_model("mock-1")
}

#[test]
fn fresh_run_emits_new_trace_file_under_emit_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let source_path = write_source_tempfile(tmp.path(), SRC);
    let trace_dir = tmp.path().join("traces");
    std::fs::create_dir_all(&trace_dir).unwrap();
    record_trace(&source_path, &trace_dir, true);
    let original_trace = trace_file_in(&trace_dir);

    let emit_dir = tmp.path().join("promote-emit");
    // emit_dir deliberately does NOT exist — the helper must
    // mkdir it. The harness allocates a fresh path per request and
    // relies on the runner to create it on demand.
    let tokio_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let emitted = tokio_rt
        .block_on(run_fresh_from_source_async(
            &original_trace,
            &source_path,
            &emit_dir,
            promote_builder(true),
        ))
        .expect("fresh run succeeds");

    assert!(emitted.exists(), "emitted trace path {emitted:?} must exist");
    assert!(
        emitted.starts_with(&emit_dir),
        "emitted trace {emitted:?} must live under emit_dir {emit_dir:?}"
    );
    assert_eq!(
        emitted.extension().and_then(|e| e.to_str()),
        Some("jsonl")
    );
}

#[test]
fn fresh_run_records_same_agent_and_args_as_original_trace() {
    let tmp = tempfile::tempdir().unwrap();
    let source_path = write_source_tempfile(tmp.path(), SRC);
    let trace_dir = tmp.path().join("traces");
    std::fs::create_dir_all(&trace_dir).unwrap();
    record_trace(&source_path, &trace_dir, true);
    let original_trace = trace_file_in(&trace_dir);

    let emit_dir = tmp.path().join("promote-emit");
    let tokio_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let emitted = tokio_rt
        .block_on(run_fresh_from_source_async(
            &original_trace,
            &source_path,
            &emit_dir,
            promote_builder(true),
        ))
        .expect("fresh run succeeds");

    let events = read_events_from_path(&emitted).expect("emitted trace parses");
    let (agent, args) = events
        .iter()
        .find_map(|e| match e {
            TraceEvent::RunStarted { agent, args, .. } => Some((agent.clone(), args.clone())),
            _ => None,
        })
        .expect("emitted trace has RunStarted");

    assert_eq!(agent, "refund_bot");
    assert!(
        args.is_empty(),
        "refund_bot takes no args; RunStarted.args should be empty, got {args:?}"
    );
}

#[test]
fn fresh_run_captures_current_behavior_when_it_differs_from_recording() {
    let tmp = tempfile::tempdir().unwrap();
    let source_path = write_source_tempfile(tmp.path(), SRC);
    let trace_dir = tmp.path().join("traces");
    std::fs::create_dir_all(&trace_dir).unwrap();
    // Record with the LLM returning `true`.
    record_trace(&source_path, &trace_dir, true);
    let original_trace = trace_file_in(&trace_dir);

    // Promote with the LLM returning `false`. The emitted trace
    // should contain the `false` answer — that's the whole point:
    // the promoted trace captures current behavior, not the
    // recording's behavior.
    let emit_dir = tmp.path().join("promote-emit");
    let tokio_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let emitted = tokio_rt
        .block_on(run_fresh_from_source_async(
            &original_trace,
            &source_path,
            &emit_dir,
            promote_builder(false),
        ))
        .expect("fresh run succeeds");

    let events = read_events_from_path(&emitted).expect("emitted trace parses");
    let llm_result = events.iter().find_map(|e| match e {
        TraceEvent::LlmResult { result, .. } => Some(result.clone()),
        _ => None,
    });
    assert_eq!(llm_result, Some(json!(false)));
}

#[test]
fn fresh_run_rejects_empty_trace() {
    let tmp = tempfile::tempdir().unwrap();
    let source_path = write_source_tempfile(tmp.path(), SRC);
    let empty_trace = tmp.path().join("empty.jsonl");
    std::fs::write(&empty_trace, "").unwrap();
    let emit_dir = tmp.path().join("promote-emit");

    let tokio_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let err = tokio_rt
        .block_on(run_fresh_from_source_async(
            &empty_trace,
            &source_path,
            &emit_dir,
            promote_builder(true),
        ))
        .expect_err("empty trace must fail");
    let msg = format!("{err:#}");
    assert!(msg.contains("empty"), "got: {msg}");
}

#[test]
fn fresh_run_rejects_missing_source() {
    let tmp = tempfile::tempdir().unwrap();
    let source_path = write_source_tempfile(tmp.path(), SRC);
    let trace_dir = tmp.path().join("traces");
    std::fs::create_dir_all(&trace_dir).unwrap();
    record_trace(&source_path, &trace_dir, true);
    let original_trace = trace_file_in(&trace_dir);

    let missing_source = tmp.path().join("does-not-exist.cor");
    let emit_dir = tmp.path().join("promote-emit");
    let tokio_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let err = tokio_rt
        .block_on(run_fresh_from_source_async(
            &original_trace,
            &missing_source,
            &emit_dir,
            promote_builder(true),
        ))
        .expect_err("missing source must fail");
    let msg = format!("{err:#}");
    assert!(msg.contains("failed to read source"), "got: {msg}");
}

#[test]
fn fresh_run_rejects_agent_not_in_current_source() {
    // Recording has `refund_bot`. Current source defines a
    // different-named agent. Promote must bail rather than silently
    // run the wrong entrypoint — the trace's agent no longer
    // exists.
    let tmp = tempfile::tempdir().unwrap();
    let recording_source = write_source_tempfile(tmp.path(), SRC);
    let trace_dir = tmp.path().join("traces");
    std::fs::create_dir_all(&trace_dir).unwrap();
    record_trace(&recording_source, &trace_dir, true);
    let original_trace = trace_file_in(&trace_dir);

    let current_source = tmp.path().join("current.cor");
    std::fs::write(
        &current_source,
        r#"
agent other_bot() -> Int:
    return 7
"#,
    )
    .unwrap();

    let emit_dir = tmp.path().join("promote-emit");
    let tokio_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let err = tokio_rt
        .block_on(run_fresh_from_source_async(
            &original_trace,
            &current_source,
            &emit_dir,
            promote_builder(true),
        ))
        .expect_err("agent-missing must fail");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("refund_bot") && msg.contains("not present"),
        "got: {msg}"
    );
}
