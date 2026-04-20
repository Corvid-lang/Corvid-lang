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

// ------------------------------------------------------------
// Mutation-mode coverage (21-inv-D-cli-wire)
// ------------------------------------------------------------

#[test]
fn mutation_replay_with_identical_replacement_reports_no_divergence() {
    // Recording has `decide_refund` returning `true`. Replay
    // mutates step 1 (the only substitutable event — the
    // LlmCall) with the SAME value `true`. Report should show
    // zero divergences because the mutation is a no-op.
    let tmp = tempfile::tempdir().unwrap();
    let source_path = write_source_tempfile(tmp.path(), SRC);
    let trace_dir = tmp.path().join("traces");
    std::fs::create_dir_all(&trace_dir).unwrap();
    record_trace(&source_path, &trace_dir, true);
    let trace_path = trace_file_in(&trace_dir);

    let outcome = run_replay_from_source_with_builder(
        &trace_path,
        &source_path,
        ReplayMode::Mutation {
            step_1based: 1,
            replacement: json!(true),
        },
        replay_builder(true),
    )
    .expect("mutation replay runs");

    assert!(
        outcome.ran_cleanly(),
        "replay errored: {:?}",
        outcome.result_error
    );
    let report = outcome
        .mutation_report
        .as_ref()
        .expect("mutation report present");
    assert!(
        report.divergences.is_empty(),
        "expected no divergences for identity mutation, got {:?}",
        report.divergences
    );
}

#[test]
fn mutation_replay_changes_final_output_when_override_differs_from_recording() {
    // Recording has `decide_refund(true)` → agent returns true.
    // Replay mutates step 1 with `false`. The agent's final
    // output changes, so the report should show a completion
    // divergence (recorded ok=true/result=true vs. live result=false).
    let tmp = tempfile::tempdir().unwrap();
    let source_path = write_source_tempfile(tmp.path(), SRC);
    let trace_dir = tmp.path().join("traces");
    std::fs::create_dir_all(&trace_dir).unwrap();
    record_trace(&source_path, &trace_dir, true);
    let trace_path = trace_file_in(&trace_dir);

    let outcome = run_replay_from_source_with_builder(
        &trace_path,
        &source_path,
        ReplayMode::Mutation {
            step_1based: 1,
            replacement: json!(false),
        },
        replay_builder(true),
    )
    .expect("mutation replay runs");

    let report = outcome
        .mutation_report
        .as_ref()
        .expect("mutation report present");
    // The mutation changed the LLM's recorded result, so the
    // final RunCompleted event diverges too.
    assert!(
        report.run_completion_divergence.is_some(),
        "expected completion divergence when mutation changes final result, \
         got divergences={:?}",
        report.divergences
    );
}

#[test]
fn mutation_replay_rejects_out_of_range_step() {
    // Trace has one substitutable event (LlmCall). Step 5 is
    // out of range; the runtime should surface a typed error.
    let tmp = tempfile::tempdir().unwrap();
    let source_path = write_source_tempfile(tmp.path(), SRC);
    let trace_dir = tmp.path().join("traces");
    std::fs::create_dir_all(&trace_dir).unwrap();
    record_trace(&source_path, &trace_dir, true);
    let trace_path = trace_file_in(&trace_dir);

    let outcome = run_replay_from_source_with_builder(
        &trace_path,
        &source_path,
        ReplayMode::Mutation {
            step_1based: 5,
            replacement: json!(false),
        },
        replay_builder(true),
    );

    // Out-of-range step should either fail at orchestrator level
    // or surface as a result_error — both are acceptable outcomes
    // as long as it's not a silent success.
    match outcome {
        Err(err) => {
            let msg = format!("{err:#}");
            assert!(
                msg.contains("mutation") || msg.contains("step") || msg.contains("range"),
                "error should mention mutation/step/range, got: {msg}"
            );
        }
        Ok(outcome) => {
            assert!(
                outcome.result_error.is_some(),
                "out-of-range step must surface as an error, not a silent clean run"
            );
        }
    }
}

#[test]
fn mutation_replay_rejects_wrong_shape_replacement() {
    // The recorded LlmResult for `decide_refund` is `Bool`. The
    // replacement JSON is an object — wrong shape. The runtime's
    // `InvalidReplayMutation` guard should reject this up front
    // (Dev B's wrong-shape handling choice from 21-inv-D-runtime).
    let tmp = tempfile::tempdir().unwrap();
    let source_path = write_source_tempfile(tmp.path(), SRC);
    let trace_dir = tmp.path().join("traces");
    std::fs::create_dir_all(&trace_dir).unwrap();
    record_trace(&source_path, &trace_dir, true);
    let trace_path = trace_file_in(&trace_dir);

    let outcome = run_replay_from_source_with_builder(
        &trace_path,
        &source_path,
        ReplayMode::Mutation {
            step_1based: 1,
            replacement: json!({"not": "a bool"}),
        },
        replay_builder(true),
    );

    // Wrong-shape should either fail at orchestrator level or
    // surface as a result_error. Silent acceptance is a bug.
    match outcome {
        Err(err) => {
            let msg = format!("{err:#}");
            assert!(
                !msg.is_empty(),
                "wrong-shape error should carry a message, got: {msg}"
            );
        }
        Ok(outcome) => {
            assert!(
                outcome.result_error.is_some() || !outcome.mutation_report
                    .as_ref()
                    .map(|r| r.divergences.is_empty())
                    .unwrap_or(true),
                "wrong-shape must surface as an error or a divergence; silent success is wrong"
            );
        }
    }
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
