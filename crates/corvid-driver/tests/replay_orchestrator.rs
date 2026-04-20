//! Driver-level end-to-end tests for the replay orchestrator.
//!
//! These exercise the full `run_replay_from_source_with_builder`
//! path: record a trace by running a tiny Corvid source through
//! mock adapters, then invoke the orchestrator with a different
//! mock adapter registered under a differential replay mode, and
//! assert the divergence report surfaces the disagreement.
//!
//! The `_with_builder` variant lets tests inject mock adapters
//! rather than relying on env-driven real API keys; production
//! CLI calls the env-driven [`run_replay_from_source`] wrapper.

use std::sync::Arc;

use corvid_driver::{
    compile_to_ir, run_ir_with_runtime, run_replay_from_source_with_builder, ReplayMode,
};
use corvid_runtime::{llm::mock::MockAdapter, ProgrammaticApprover, Runtime};
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
    // The Runtime's Tracer flushes on drop; force that here so the
    // trace file is readable before we pop the tempdir.
    drop(runtime);
}

fn trace_file_in(trace_dir: &std::path::Path) -> std::path::PathBuf {
    // Recorder creates one .jsonl per run under the dir.
    for entry in std::fs::read_dir(trace_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            return path;
        }
    }
    panic!("no .jsonl file in {}", trace_dir.display());
}

/// Builder the replay orchestrator should receive:
/// - `mock-1` adapter (matches the recording's default_model so
///   recorded LlmCall events' `model: Some("mock-1")` field
///   reproduces identically at replay — without this the runtime
///   reports a trace-shape divergence for the model field before
///   any LLM value comparison happens)
/// - `mock-2` adapter (the differential target; this is the one
///   the live call dispatches against)
/// - `default_model("mock-1")` (same reason as above — the
///   runtime emits LlmCall.model from this field)
/// - approver (Corvid runs expect one)
fn replay_builder(mock2_reply: bool) -> corvid_runtime::RuntimeBuilder {
    Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(
            MockAdapter::new("mock-1").reply("decide_refund", json!(true)),
        ))
        .llm(Arc::new(
            MockAdapter::new("mock-2").reply("decide_refund", json!(mock2_reply)),
        ))
        .default_model("mock-1")
}

#[test]
fn differential_replay_with_agreeing_live_model_reports_no_llm_divergence() {
    let tmp = tempfile::tempdir().unwrap();
    let source_path = write_source_tempfile(tmp.path(), SRC);
    let trace_dir = tmp.path().join("traces");
    std::fs::create_dir_all(&trace_dir).unwrap();
    record_trace(&source_path, &trace_dir, true);
    let trace_path = trace_file_in(&trace_dir);

    // Live mock-2 returns the SAME value as mock-1 did when
    // recording (both true). Differential replay should report
    // zero LLM divergences.
    let outcome = run_replay_from_source_with_builder(
        &trace_path,
        &source_path,
        ReplayMode::Differential("mock-2".into()),
        replay_builder(true),
    )
    .expect("differential replay runs");

    assert!(
        outcome.ran_cleanly(),
        "replay errored: {:?}",
        outcome.result_error
    );
    let report = outcome
        .differential_report
        .as_ref()
        .expect("differential report present");
    assert!(
        report.llm_divergences.is_empty(),
        "expected no LLM divergences, got {:?}",
        report.llm_divergences
    );
}

#[test]
fn differential_replay_with_disagreeing_live_model_reports_llm_divergence() {
    let tmp = tempfile::tempdir().unwrap();
    let source_path = write_source_tempfile(tmp.path(), SRC);
    let trace_dir = tmp.path().join("traces");
    std::fs::create_dir_all(&trace_dir).unwrap();
    record_trace(&source_path, &trace_dir, true);
    let trace_path = trace_file_in(&trace_dir);

    // Live mock-2 returns `false` while the recording has `true`.
    // Differential report should flag exactly one LLM divergence.
    let outcome = run_replay_from_source_with_builder(
        &trace_path,
        &source_path,
        ReplayMode::Differential("mock-2".into()),
        replay_builder(false),
    )
    .expect("differential replay runs");

    let report = outcome
        .differential_report
        .expect("differential report present");
    assert_eq!(
        report.llm_divergences.len(),
        1,
        "expected one LLM divergence, got {:?}",
        report.llm_divergences
    );
    let divergence = &report.llm_divergences[0];
    assert_eq!(divergence.prompt, "decide_refund");
    assert_eq!(divergence.recorded, json!(true));
    assert_eq!(divergence.live, json!(false));
}

#[test]
fn replay_orchestrator_extracts_agent_name_from_trace() {
    // Regression: the orchestrator must use the recorded agent
    // from RunStarted, not guess (even when only one agent is
    // declared, explicit-lookup must still work).
    let tmp = tempfile::tempdir().unwrap();
    let source_path = write_source_tempfile(tmp.path(), SRC);
    let trace_dir = tmp.path().join("traces");
    std::fs::create_dir_all(&trace_dir).unwrap();
    record_trace(&source_path, &trace_dir, true);
    let trace_path = trace_file_in(&trace_dir);

    let outcome = run_replay_from_source_with_builder(
        &trace_path,
        &source_path,
        ReplayMode::Differential("mock-2".into()),
        replay_builder(true),
    )
    .expect("differential replay runs");

    assert_eq!(outcome.agent_name, "refund_bot");
}

#[test]
fn replay_orchestrator_rejects_missing_source() {
    let tmp = tempfile::tempdir().unwrap();
    let trace_dir = tmp.path().join("traces");
    std::fs::create_dir_all(&trace_dir).unwrap();
    // No source written at this path.
    let nonexistent_source = tmp.path().join("does-not-exist.cor");
    // Need a valid trace to get past the trace-load step.
    let real_source_path = write_source_tempfile(tmp.path(), SRC);
    record_trace(&real_source_path, &trace_dir, true);
    let trace_path = trace_file_in(&trace_dir);

    let err = run_replay_from_source_with_builder(
        &trace_path,
        &nonexistent_source,
        ReplayMode::Differential("mock-2".into()),
        replay_builder(true),
    )
    .expect_err("missing source file must fail");

    let msg = format!("{err:#}");
    assert!(
        msg.contains("source"),
        "expected error to mention source, got: {msg}"
    );
}

#[test]
fn replay_orchestrator_rejects_empty_trace() {
    let tmp = tempfile::tempdir().unwrap();
    let source_path = write_source_tempfile(tmp.path(), SRC);
    let trace_path = tmp.path().join("empty.jsonl");
    std::fs::write(&trace_path, "").unwrap();

    let err = run_replay_from_source_with_builder(
        &trace_path,
        &source_path,
        ReplayMode::Differential("mock-2".into()),
        replay_builder(true),
    )
    .expect_err("empty trace must fail");

    let msg = format!("{err:#}");
    assert!(msg.contains("empty"), "got: {msg}");
}

#[test]
fn replay_orchestrator_rejects_agent_not_in_source() {
    // The trace was recorded against a source containing agent
    // `refund_bot`. Compiling a DIFFERENT source (without that
    // agent) should surface a clean error, not an interpreter
    // panic.
    let tmp = tempfile::tempdir().unwrap();
    let recording_source = write_source_tempfile(tmp.path(), SRC);
    let trace_dir = tmp.path().join("traces");
    std::fs::create_dir_all(&trace_dir).unwrap();
    record_trace(&recording_source, &trace_dir, true);
    let trace_path = trace_file_in(&trace_dir);

    // Different source with a differently-named agent.
    let mismatched_source = tmp.path().join("mismatched.cor");
    std::fs::write(
        &mismatched_source,
        r#"
agent other_bot() -> Int:
    return 7
"#,
    )
    .unwrap();

    let err = run_replay_from_source_with_builder(
        &trace_path,
        &mismatched_source,
        ReplayMode::Differential("mock-2".into()),
        replay_builder(true),
    )
    .expect_err("mismatched agent name must fail");

    let msg = format!("{err:#}");
    assert!(
        msg.contains("refund_bot") && msg.contains("not present"),
        "got: {msg}"
    );
}
