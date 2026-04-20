use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use corvid_runtime::{
    run_test_from_traces, LlmDivergence, PromoteDecision, PromotePromptMode,
    ReplayDifferentialReport, ReplayDivergence, RuntimeError, TestFromTracesOptions,
    TraceHarnessMode, TraceHarnessRequest, TraceHarnessRun, Verdict,
};
use corvid_trace_schema::{write_events_to_path, TraceEvent, SCHEMA_VERSION, WRITER_INTERPRETER, WRITER_NATIVE};
use serde_json::json;

#[derive(Clone)]
enum Scenario {
    Pass {
        output: serde_json::Value,
    },
    Diverge {
        step: usize,
        got_kind: &'static str,
        got_description: &'static str,
    },
    Differential {
        output: serde_json::Value,
        report: ReplayDifferentialReport,
    },
    RecordCurrent {
        output: serde_json::Value,
        rewritten_result: serde_json::Value,
    },
    Error(RuntimeError),
}

#[derive(Default)]
struct RunnerState {
    scenarios: HashMap<(PathBuf, &'static str), Scenario>,
    replay_counts: HashMap<PathBuf, usize>,
}

impl RunnerState {
    fn set(&mut self, path: &Path, mode: &'static str, scenario: Scenario) {
        self.scenarios.insert((path.to_path_buf(), mode), scenario);
    }
}

fn trace_file(dir: &Path, name: &str, writer: &str) -> PathBuf {
    let path = dir.join(format!("{name}.jsonl"));
    let events = vec![
        TraceEvent::SchemaHeader {
            version: SCHEMA_VERSION,
            writer: writer.to_string(),
            commit_sha: None,
            source_path: None,
            ts_ms: 1,
            run_id: format!("run-{name}"),
        },
        TraceEvent::RunStarted {
            ts_ms: 2,
            run_id: format!("run-{name}"),
            agent: name.to_string(),
            args: vec![],
        },
        TraceEvent::RunCompleted {
            ts_ms: 3,
            run_id: format!("run-{name}"),
            ok: true,
            result: Some(json!("ok")),
            error: None,
        },
    ];
    write_events_to_path(&path, &events).unwrap();
    path
}

fn read_trace_json(path: &Path) -> Vec<serde_json::Value> {
    corvid_trace_schema::read_events_from_path(path)
        .unwrap()
        .into_iter()
        .map(|event| serde_json::to_value(event).unwrap())
        .collect()
}

async fn fake_runner(
    state: Arc<Mutex<RunnerState>>,
    request: TraceHarnessRequest,
) -> Result<TraceHarnessRun, RuntimeError> {
    let mode_key = match &request.mode {
        TraceHarnessMode::Replay => "replay",
        TraceHarnessMode::Differential { .. } => "differential",
        TraceHarnessMode::RecordCurrent => "record_current",
    };

    let mut guard = state.lock().unwrap();
    if matches!(request.mode, TraceHarnessMode::Replay) {
        *guard
            .replay_counts
            .entry(request.trace_path.clone())
            .or_insert(0) += 1;
    }
    let scenario = guard
        .scenarios
        .get(&(request.trace_path.clone(), mode_key))
        .cloned()
        .unwrap_or(Scenario::Pass {
            output: json!("ok"),
        });
    drop(guard);

    match scenario {
        Scenario::Pass { output } => {
            let emitted = request.emit_dir.join("replayed.jsonl");
            std::fs::create_dir_all(&request.emit_dir).unwrap();
            let bytes = std::fs::read(&request.trace_path).unwrap();
            std::fs::write(&emitted, bytes).unwrap();
            Ok(TraceHarnessRun {
                final_output: Some(output),
                ok: true,
                error: None,
                emitted_trace_path: emitted,
                differential_report: None,
            })
        }
        Scenario::Diverge {
            step,
            got_kind,
            got_description,
        } => Err(RuntimeError::ReplayDivergence(ReplayDivergence {
            step,
            expected: TraceEvent::RunStarted {
                ts_ms: 0,
                run_id: "expected".into(),
                agent: "demo".into(),
                args: vec![],
            },
            got_kind,
            got_description: got_description.into(),
        })),
        Scenario::Differential { output, report } => {
            let emitted = request.emit_dir.join("differential.jsonl");
            std::fs::create_dir_all(&request.emit_dir).unwrap();
            let bytes = std::fs::read(&request.trace_path).unwrap();
            std::fs::write(&emitted, bytes).unwrap();
            Ok(TraceHarnessRun {
                final_output: Some(output),
                ok: true,
                error: None,
                emitted_trace_path: emitted,
                differential_report: Some(report),
            })
        }
        Scenario::RecordCurrent {
            output,
            rewritten_result,
        } => {
            let emitted = request.emit_dir.join("recorded.jsonl");
            std::fs::create_dir_all(&request.emit_dir).unwrap();
            let events = vec![
                TraceEvent::SchemaHeader {
                    version: SCHEMA_VERSION,
                    writer: WRITER_INTERPRETER.to_string(),
                    commit_sha: None,
                    source_path: Some("current.cor".into()),
                    ts_ms: 1,
                    run_id: "promoted".into(),
                },
                TraceEvent::RunStarted {
                    ts_ms: 2,
                    run_id: "promoted".into(),
                    agent: "demo".into(),
                    args: vec![],
                },
                TraceEvent::RunCompleted {
                    ts_ms: 3,
                    run_id: "promoted".into(),
                    ok: true,
                    result: Some(rewritten_result),
                    error: None,
                },
            ];
            write_events_to_path(&emitted, &events).unwrap();
            Ok(TraceHarnessRun {
                final_output: Some(output),
                ok: true,
                error: None,
                emitted_trace_path: emitted,
                differential_report: None,
            })
        }
        Scenario::Error(err) => Err(err),
    }
}

#[tokio::test]
async fn harness_all_passing_traces_returns_clean_report() {
    let dir = tempfile::tempdir().unwrap();
    let a = trace_file(dir.path(), "a", WRITER_INTERPRETER);
    let b = trace_file(dir.path(), "b", WRITER_INTERPRETER);
    let c = trace_file(dir.path(), "c", WRITER_INTERPRETER);
    let state = Arc::new(Mutex::new(RunnerState::default()));

    let report = run_test_from_traces(
        vec![a, b, c],
        TestFromTracesOptions::default(),
        |request| fake_runner(state.clone(), request),
    )
    .await;

    assert_eq!(report.summary.total, 3);
    assert_eq!(report.summary.passed, 3);
    assert_eq!(report.summary.diverged, 0);
}

#[tokio::test]
async fn harness_one_diverging_trace_surfaces_in_report() {
    let dir = tempfile::tempdir().unwrap();
    let ok = trace_file(dir.path(), "ok", WRITER_INTERPRETER);
    let bad = trace_file(dir.path(), "bad", WRITER_INTERPRETER);
    let state = Arc::new(Mutex::new(RunnerState::default()));
    state.lock().unwrap().set(
        &bad,
        "replay",
        Scenario::Diverge {
            step: 7,
            got_kind: "tool_call",
            got_description: "tool name changed".into(),
        },
    );

    let report = run_test_from_traces(
        vec![ok, bad.clone()],
        TestFromTracesOptions::default(),
        |request| fake_runner(state.clone(), request),
    )
    .await;

    assert_eq!(report.summary.diverged, 1);
    let outcome = report
        .per_trace
        .iter()
        .find(|outcome| outcome.path == bad)
        .unwrap();
    assert_eq!(outcome.verdict, Verdict::Diverged);
    match &outcome.divergences[0] {
        corvid_runtime::Divergence::Replay(div) => assert_eq!(div.step, 7),
        other => panic!("expected replay divergence, got {other:?}"),
    }
}

#[tokio::test]
async fn harness_replay_model_runs_differential_per_trace() {
    let dir = tempfile::tempdir().unwrap();
    let trace = trace_file(dir.path(), "diff", WRITER_INTERPRETER);
    let state = Arc::new(Mutex::new(RunnerState::default()));
    state.lock().unwrap().set(
        &trace,
        "differential",
        Scenario::Differential {
            output: json!("cancel"),
            report: ReplayDifferentialReport {
                llm_divergences: vec![LlmDivergence {
                    step: 3,
                    prompt: "classify".into(),
                    recorded: json!("refund"),
                    live: json!("cancel"),
                }],
                ..Default::default()
            },
        },
    );

    let report = run_test_from_traces(
        vec![trace.clone()],
        TestFromTracesOptions {
            replay_model: Some("mock-2".into()),
            ..Default::default()
        },
        |request| fake_runner(state.clone(), request),
    )
    .await;

    let outcome = &report.per_trace[0];
    assert_eq!(outcome.verdict, Verdict::Diverged);
    assert!(outcome.model_swap.is_some());
    assert_eq!(report.summary.diverged, 1);
}

#[tokio::test]
async fn harness_promote_noninteractive_stdin_defaults_to_reject() {
    let dir = tempfile::tempdir().unwrap();
    let trace = trace_file(dir.path(), "promote-reject", WRITER_INTERPRETER);
    let before = read_trace_json(&trace);
    let state = Arc::new(Mutex::new(RunnerState::default()));
    state.lock().unwrap().set(
        &trace,
        "replay",
        Scenario::Diverge {
            step: 2,
            got_kind: "llm_call",
            got_description: "prompt changed",
        },
    );

    let report = run_test_from_traces(
        vec![trace.clone()],
        TestFromTracesOptions {
            promote: true,
            ..Default::default()
        },
        |request| fake_runner(state.clone(), request),
    )
    .await;

    assert_eq!(report.per_trace[0].verdict, Verdict::Diverged);
    assert_eq!(before, read_trace_json(&trace));
}

#[tokio::test]
async fn harness_promote_accept_all_rewrites_trace_files() {
    let dir = tempfile::tempdir().unwrap();
    let first = trace_file(dir.path(), "first", WRITER_INTERPRETER);
    let second = trace_file(dir.path(), "second", WRITER_INTERPRETER);
    let state = Arc::new(Mutex::new(RunnerState::default()));
    {
        let mut guard = state.lock().unwrap();
        for path in [&first, &second] {
            guard.set(
                path,
                "replay",
                Scenario::Diverge {
                    step: 4,
                    got_kind: "run_completed",
                    got_description: "result changed",
                },
            );
            guard.set(
                path,
                "record_current",
                Scenario::RecordCurrent {
                    output: json!("new"),
                    rewritten_result: json!("new"),
                },
            );
        }
    }

    let report = run_test_from_traces(
        vec![first.clone(), second.clone()],
        TestFromTracesOptions {
            promote: true,
            prompt_mode: PromotePromptMode::Decisions(vec![PromoteDecision::AcceptAllRemaining]),
            ..Default::default()
        },
        |request| fake_runner(state.clone(), request),
    )
    .await;

    assert_eq!(report.summary.promoted, 2);
    assert!(report.per_trace.iter().all(|outcome| outcome.verdict == Verdict::Promoted));
    for path in [&first, &second] {
        let events = read_trace_json(path);
        assert!(events.iter().any(|event| event.get("run_id") == Some(&json!("promoted"))));
    }
}

#[tokio::test]
async fn harness_flake_detect_on_deterministic_trace_reports_no_flake() {
    let dir = tempfile::tempdir().unwrap();
    let trace = trace_file(dir.path(), "stable", WRITER_INTERPRETER);
    let state = Arc::new(Mutex::new(RunnerState::default()));

    let report = run_test_from_traces(
        vec![trace],
        TestFromTracesOptions {
            flake_detect: Some(5),
            ..Default::default()
        },
        |request| fake_runner(state.clone(), request),
    )
    .await;

    assert_eq!(report.per_trace[0].verdict, Verdict::Passed);
    assert_eq!(report.per_trace[0].flake_rank.as_ref().unwrap().divergent_runs, 0);
}

#[tokio::test]
async fn harness_flake_detect_on_nondeterministic_program_reports_flake() {
    let dir = tempfile::tempdir().unwrap();
    let trace = trace_file(dir.path(), "flaky", WRITER_INTERPRETER);
    let state = Arc::new(Mutex::new(RunnerState::default()));
    let trace_clone = trace.clone();

    let report = run_test_from_traces(
        vec![trace],
        TestFromTracesOptions {
            flake_detect: Some(5),
            ..Default::default()
        },
        |request| {
            let state = state.clone();
            let trace_clone = trace_clone.clone();
            async move {
                if request.trace_path == trace_clone {
                    let mut guard = state.lock().unwrap();
                    let counter = guard.replay_counts.entry(trace_clone.clone()).or_insert(0);
                    *counter += 1;
                    let emitted = request.emit_dir.join("flaky.jsonl");
                    std::fs::create_dir_all(&request.emit_dir).unwrap();
                    let bytes = std::fs::read(&request.trace_path).unwrap();
                    std::fs::write(&emitted, bytes).unwrap();
                    return Ok(TraceHarnessRun {
                        final_output: Some(json!(if *counter % 2 == 0 { "a" } else { "b" })),
                        ok: true,
                        error: None,
                        emitted_trace_path: emitted,
                        differential_report: None,
                    });
                }
                fake_runner(state.clone(), request).await
            }
        },
    )
    .await;

    assert_eq!(report.per_trace[0].verdict, Verdict::Flaky);
    assert!(report.per_trace[0].flake_rank.as_ref().unwrap().divergent_runs > 0);
}

#[tokio::test]
async fn harness_empty_filtered_set_returns_summary_total_zero() {
    let state = Arc::new(Mutex::new(RunnerState::default()));
    let report = run_test_from_traces(Vec::new(), TestFromTracesOptions::default(), |request| {
        fake_runner(state.clone(), request)
    })
    .await;

    assert_eq!(report.summary.total, 0);
    assert_eq!(report.per_trace.len(), 0);
}

#[tokio::test]
async fn harness_malformed_trace_mid_run_surfaces_as_errored_verdict_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    let good = trace_file(dir.path(), "good", WRITER_INTERPRETER);
    let bad = dir.path().join("bad.jsonl");
    std::fs::write(&bad, "{not-json}\n").unwrap();
    let state = Arc::new(Mutex::new(RunnerState::default()));

    let report = run_test_from_traces(vec![good, bad], TestFromTracesOptions::default(), |request| {
        fake_runner(state.clone(), request)
    })
    .await;

    assert_eq!(report.summary.errored, 1);
    assert_eq!(report.summary.passed, 1);
}

#[tokio::test]
async fn harness_cross_tier_trace_is_errored_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    let trace = trace_file(dir.path(), "native", WRITER_NATIVE);
    let state = Arc::new(Mutex::new(RunnerState::default()));
    state.lock().unwrap().set(
        &trace,
        "replay",
        Scenario::Error(RuntimeError::CrossTierReplayUnsupported {
            recorded_writer: WRITER_NATIVE.into(),
            replay_writer: WRITER_INTERPRETER.into(),
        }),
    );

    let report = run_test_from_traces(
        vec![trace],
        TestFromTracesOptions::default(),
        |request| fake_runner(state.clone(), request),
    )
    .await;

    assert_eq!(report.summary.errored, 1);
    assert_eq!(report.per_trace[0].verdict, Verdict::Error);
}
