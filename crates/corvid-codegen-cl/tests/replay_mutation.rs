use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use corvid_codegen_cl::build_native_to_disk;
use corvid_ir::lower;
use corvid_resolve::resolve;
use corvid_runtime::{llm::mock::MockAdapter, ProgrammaticApprover, ReplayMutationReport, Runtime, TraceEvent};
use corvid_syntax::{lex, parse_file};
use corvid_trace_schema::read_events_from_path;
use corvid_types::typecheck;
use corvid_vm::{run_agent, Value};
use serde_json::json;

const SIMPLE_SRC: &str = r#"
prompt decide_refund(amount: Int) -> Bool:
    """Should refund {amount}?"""

agent refund_bot() -> Bool:
    return decide_refund(42)
"#;

const DRIFT_SRC: &str = r#"
prompt choose_label() -> String:
    """Choose label."""

agent drift_bot() -> String:
    label = choose_label()
    approve UseLabel(label)
    return label
"#;

const TWO_STEP_SRC: &str = r#"
prompt classify_first() -> String:
    """First label."""

prompt classify_second(first: String) -> String:
    """Second label from {first}."""

agent two_step() -> String:
    first = classify_first()
    return classify_second(first)
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

fn build_native(ir: &corvid_ir::IrFile, bin_path: &Path) -> PathBuf {
    ensure_runtime_staticlib();
    build_native_to_disk(ir, "corvid_replay_mutation", bin_path, &[]).expect("compile native binary")
}

fn interpreter_runtime(trace_dir: &Path) -> Runtime {
    Runtime::builder()
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

fn run_native_binary(
    bin: &Path,
    trace_path: &Path,
    replay_trace: Option<&Path>,
    mutate_step: Option<usize>,
    mutate_json: Option<&str>,
    report_path: Option<&Path>,
    replies_json: &str,
) -> std::process::Output {
    let mut cmd = Command::new(bin);
    cmd.env("CORVID_TRACE_PATH", trace_path)
        .env("CORVID_MODEL", "mock-1")
        .env("CORVID_TEST_MOCK_LLM", "1")
        .env("CORVID_TEST_MOCK_LLM_REPLIES", replies_json)
        .env("CORVID_APPROVE_AUTO", "1");
    if let Some(replay_trace) = replay_trace {
        cmd.env("CORVID_REPLAY_TRACE_PATH", replay_trace);
    }
    if let Some(step) = mutate_step {
        cmd.env("CORVID_REPLAY_MUTATE_STEP", step.to_string());
    }
    if let Some(mutate_json) = mutate_json {
        cmd.env("CORVID_REPLAY_MUTATE_JSON", mutate_json);
    }
    if let Some(report_path) = report_path {
        cmd.env("CORVID_REPLAY_MUTATION_REPORT_PATH", report_path);
    }
    cmd.output().expect("run native binary")
}

fn read_report(path: &Path) -> ReplayMutationReport {
    serde_json::from_slice(&std::fs::read(path).expect("read report")).expect("parse report")
}

fn normalize_trace(path: &Path) -> Vec<serde_json::Value> {
    read_events_from_path(path)
        .unwrap()
        .into_iter()
        .map(|event| normalize_value(serde_json::to_value(event).unwrap()))
        .collect()
}

fn normalize_value(mut value: serde_json::Value) -> serde_json::Value {
    match &mut value {
        serde_json::Value::Object(map) => {
            map.remove("run_id");
            map.remove("ts_ms");
            for child in map.values_mut() {
                *child = normalize_value(child.take());
            }
            value
        }
        serde_json::Value::Array(items) => {
            for child in items.iter_mut() {
                *child = normalize_value(child.take());
            }
            value
        }
        _ => value,
    }
}

#[test]
fn mutation_at_first_llm_step_changes_final_output() {
    let ir = ir_of(SIMPLE_SRC);
    let tmp = tempfile::tempdir().unwrap();
    let bin = build_native(&ir, &tmp.path().join("refund_native"));

    let recorded_trace = tmp.path().join("recorded.jsonl");
    let recorded = run_native_binary(&bin, &recorded_trace, None, None, None, None, "{\"decide_refund\":true}");
    assert!(recorded.status.success(), "record run should succeed");

    let replay_trace = tmp.path().join("replayed.jsonl");
    let report_path = tmp.path().join("report.json");
    let replayed = run_native_binary(
        &bin,
        &replay_trace,
        Some(&recorded_trace),
        Some(1),
        Some("false"),
        Some(&report_path),
        "{\"decide_refund\":true}",
    );
    assert!(replayed.status.success(), "mutation replay should succeed");
    assert!(String::from_utf8_lossy(&replayed.stdout).contains("false"));
}

#[test]
fn mutation_divergence_report_lists_downstream_shape_mismatches() {
    let ir = ir_of(DRIFT_SRC);
    let tmp = tempfile::tempdir().unwrap();
    let bin = build_native(&ir, &tmp.path().join("drift_native"));

    let recorded_trace = tmp.path().join("recorded.jsonl");
    let recorded = run_native_binary(&bin, &recorded_trace, None, None, None, None, "{\"choose_label\":\"std\"}");
    assert!(recorded.status.success(), "record run should succeed");
    assert!(String::from_utf8_lossy(&recorded.stdout).contains("std"));

    let replay_trace = tmp.path().join("replayed.jsonl");
    let report_path = tmp.path().join("report.json");
    let replayed = run_native_binary(
        &bin,
        &replay_trace,
        Some(&recorded_trace),
        Some(1),
        Some("\"vip\""),
        Some(&report_path),
        "{\"choose_label\":\"std\"}",
    );
    assert!(replayed.status.success(), "mutation replay should succeed");
    assert!(String::from_utf8_lossy(&replayed.stdout).contains("vip"));

    let report = read_report(&report_path);
    assert_eq!(report.divergences.len(), 1);
    assert_eq!(report.divergences[0].step, 2);
    assert_eq!(report.divergences[0].kind, "approval_request");
    assert_eq!(
        normalize_value(report.divergences[0].recorded.clone()),
        normalize_value(
            serde_json::to_value(TraceEvent::ApprovalRequest {
                ts_ms: 0,
                run_id: String::new(),
                label: "UseLabel".into(),
                args: vec![json!("std")],
            })
            .unwrap()
        )
    );
    assert_eq!(
        report.divergences[0].got,
        json!({
            "label": "UseLabel",
            "args": ["vip"],
        })
    );
    assert!(report.run_completion_divergence.is_some());
}

#[test]
fn mutation_at_step_zero_or_out_of_range_returns_clean_error() {
    let ir = ir_of(SIMPLE_SRC);
    let tmp = tempfile::tempdir().unwrap();
    let bin = build_native(&ir, &tmp.path().join("refund_native"));

    let recorded_trace = tmp.path().join("recorded.jsonl");
    let recorded = run_native_binary(&bin, &recorded_trace, None, None, None, None, "{\"decide_refund\":true}");
    assert!(recorded.status.success(), "record run should succeed");

    for step in [0usize, 99usize] {
        let replayed = run_native_binary(
            &bin,
            &tmp.path().join(format!("replayed-{step}.jsonl")),
            Some(&recorded_trace),
            Some(step),
            Some("false"),
            Some(&tmp.path().join(format!("report-{step}.json"))),
            "{\"decide_refund\":true}",
        );
        assert!(!replayed.status.success(), "invalid mutation step should fail");
        assert!(
            String::from_utf8_lossy(&replayed.stderr).contains("invalid replay mutation"),
            "stderr should mention invalid replay mutation: {}",
            String::from_utf8_lossy(&replayed.stderr)
        );
    }
}

#[test]
fn mutation_replacement_with_wrong_json_shape_surfaces_typed_error() {
    let ir = ir_of(DRIFT_SRC);
    let tmp = tempfile::tempdir().unwrap();
    let bin = build_native(&ir, &tmp.path().join("drift_native"));

    let recorded_trace = tmp.path().join("recorded.jsonl");
    let recorded = run_native_binary(&bin, &recorded_trace, None, None, None, None, "{\"choose_label\":\"std\"}");
    assert!(recorded.status.success(), "record run should succeed");

    let replayed = run_native_binary(
        &bin,
        &tmp.path().join("wrong-shape.jsonl"),
        Some(&recorded_trace),
        Some(1),
        Some("{\"label\":\"vip\"}"),
        Some(&tmp.path().join("wrong-shape-report.json")),
        "{\"choose_label\":\"std\"}",
    );
    assert!(!replayed.status.success(), "wrong-shape mutation should fail");
    assert!(
        String::from_utf8_lossy(&replayed.stderr).contains("invalid replay mutation"),
        "stderr should mention invalid replay mutation: {}",
        String::from_utf8_lossy(&replayed.stderr)
    );
}

#[test]
fn mutation_preserves_byte_identity_for_steps_before_step() {
    let ir = ir_of(TWO_STEP_SRC);
    let tmp = tempfile::tempdir().unwrap();
    let bin = build_native(&ir, &tmp.path().join("two-step-native"));

    let recorded_trace = tmp.path().join("recorded.jsonl");
    let recorded = run_native_binary(
        &bin,
        &recorded_trace,
        None,
        None,
        None,
        None,
        "{\"classify_first\":\"refund\",\"classify_second\":\"approve\"}",
    );
    assert!(recorded.status.success(), "record run should succeed");

    let replay_trace = tmp.path().join("replayed.jsonl");
    let report_path = tmp.path().join("report.json");
    let replayed = run_native_binary(
        &bin,
        &replay_trace,
        Some(&recorded_trace),
        Some(2),
        Some("\"cancel\""),
        Some(&report_path),
        "{\"classify_first\":\"refund\",\"classify_second\":\"approve\"}",
    );
    assert!(replayed.status.success(), "mutation replay should succeed");

    let recorded = normalize_trace(&recorded_trace);
    let replayed = normalize_trace(&replay_trace);
    let second_step_index = read_events_from_path(&recorded_trace)
        .unwrap()
        .iter()
        .position(|event| {
            matches!(
                event,
                TraceEvent::LlmCall {
                    prompt,
                    ..
                } if prompt == "classify_second"
            )
        })
        .unwrap();
    assert_eq!(recorded[..second_step_index], replayed[..second_step_index]);
}

#[tokio::test]
async fn cross_tier_guard_still_rejects_interpreter_trace_from_native_mutation() {
    let trace_dir = tempfile::tempdir().unwrap();
    let (interp_result, interpreter_trace) = run_interpreter(trace_dir.path()).await;
    assert_eq!(interp_result, Value::Bool(true));

    let ir = ir_of(SIMPLE_SRC);
    let tmp = tempfile::tempdir().unwrap();
    let bin = build_native(&ir, &tmp.path().join("refund_native"));
    let replayed = run_native_binary(
        &bin,
        &tmp.path().join("native-replay.jsonl"),
        Some(&interpreter_trace),
        Some(1),
        Some("false"),
        Some(&tmp.path().join("report.json")),
        "{\"decide_refund\":true}",
    );
    assert!(!replayed.status.success(), "cross-tier replay should fail");
    assert!(
        String::from_utf8_lossy(&replayed.stderr).contains("cross-tier replay is not supported in v1"),
        "stderr should mention cross-tier replay rejection: {}",
        String::from_utf8_lossy(&replayed.stderr)
    );
}
