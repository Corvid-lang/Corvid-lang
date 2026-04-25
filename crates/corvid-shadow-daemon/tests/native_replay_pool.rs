use corvid_driver::{build_or_get_cached_native, compile_to_ir_with_config_at_path};
use corvid_shadow_daemon::{
    NativeShadowExecutor, ShadowExecutionMode, ShadowReplayExecutor,
};
use corvid_trace_schema::{read_events_from_path, TraceEvent, WRITER_NATIVE};

#[tokio::test]
async fn native_shadow_executor_replays_native_trace() {
    let tmp = tempfile::tempdir().unwrap();
    let source_path = tmp.path().join("program.cor");
    let source = "\
prompt answer() -> Int:
    \"\"\"Return 42.\"\"\"

agent main() -> Int:
    return answer()
";
    std::fs::write(&source_path, source).unwrap();

    let ir = compile_to_ir_with_config_at_path(source, &source_path, None).unwrap();
    let binary = build_or_get_cached_native(&source_path, source, &ir, None)
        .expect("native compile")
        .path;
    let recorded_trace = tmp.path().join("recorded-native.jsonl");
    let recorded = std::process::Command::new(&binary)
        .env("CORVID_TRACE_PATH", &recorded_trace)
        .env("CORVID_TEST_MOCK_LLM", "1")
        .env("CORVID_TEST_MOCK_LLM_REPLIES", "{\"answer\":42}")
        .output()
        .expect("run native recording");
    assert!(
        recorded.status.success(),
        "recording failed\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&recorded.stdout),
        String::from_utf8_lossy(&recorded.stderr)
    );

    let recorded_events = read_events_from_path(&recorded_trace).unwrap();
    assert!(recorded_events.iter().any(|event| {
        matches!(event, TraceEvent::SchemaHeader { writer, .. } if writer == WRITER_NATIVE)
    }));

    let executor = NativeShadowExecutor::from_program_path(&source_path).unwrap();
    let outcome = executor
        .execute(&recorded_trace, ShadowExecutionMode::Replay)
        .await
        .unwrap();

    assert!(outcome.ok, "native shadow error: {:?}", outcome.error);
    assert_eq!(outcome.recorded_output, Some(serde_json::json!(42)));
    assert_eq!(outcome.shadow_output, Some(serde_json::json!(42)));
    assert!(outcome.traces_match(), "native replay trace should match");
    assert!(outcome.shadow_events.iter().any(|event| {
        matches!(event, TraceEvent::SchemaHeader { writer, .. } if writer == WRITER_NATIVE)
    }));
}
