use corvid_repl::Repl;
use std::io::Cursor;

#[test]
fn replay_commands_step_through_recorded_trace() {
    let trace_path = format!(
        "{}/tests/fixtures/sample_replay.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    let input = Cursor::new(format!(
        ":replay {trace_path}\n:step\n:show\n\n:step 2\n:where\n:run\n:q\n"
    ));
    let mut output = Vec::new();
    Repl::run(input, &mut output).expect("repl run succeeds");
    let text = String::from_utf8(output).expect("valid utf8");
    assert!(text.contains("loaded replay"), "unexpected output: {text}");
    assert!(text.contains("[step 1/5] run start: refund_bot"), "unexpected output: {text}");
    assert!(text.contains("tool call"), "unexpected output: {text}");
    assert!(text.contains("llm call"), "unexpected output: {text}");
    assert!(text.contains("approval"), "unexpected output: {text}");
    assert!(text.contains("replay position: 4/5"), "unexpected output: {text}");
    assert!(text.contains("run complete"), "unexpected output: {text}");
    assert!(text.contains("end of replay (OK)"), "unexpected output: {text}");
    assert!(text.contains("left replay mode"), "unexpected output: {text}");
}

#[test]
fn replay_rejects_invalid_trace_without_leaving_normal_mode() {
    let missing_path = format!(
        "{}/tests/fixtures/missing_replay.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    let invalid_path = format!(
        "{}/tests/fixtures/invalid_replay.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    let input = Cursor::new(format!(
        ":replay {missing_path}\n:replay {invalid_path}\n1 + 1\n"
    ));
    let mut output = Vec::new();
    Repl::run(input, &mut output).expect("repl run succeeds");
    let text = String::from_utf8(output).expect("valid utf8");
    assert!(text.contains("cannot read replay"), "unexpected output: {text}");
    assert!(
        text.contains("has invalid JSONL at line 1"),
        "unexpected output: {text}"
    );
    assert!(text.contains("2"), "unexpected output: {text}");
}

#[test]
fn replay_marks_truncated_trace_and_keeps_step_output() {
    let trace_path = format!(
        "{}/tests/fixtures/truncated_replay.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    let input = Cursor::new(format!(":replay {trace_path}\n:run\n"));
    let mut output = Vec::new();
    Repl::run(input, &mut output).expect("repl run succeeds");
    let text = String::from_utf8(output).expect("valid utf8");
    assert!(
        text.contains("final status: TRUNCATED"),
        "unexpected output: {text}"
    );
    assert!(text.contains("tool call"), "unexpected output: {text}");
    assert!(text.contains("llm call"), "unexpected output: {text}");
    assert!(
        text.contains("output : <missing>"),
        "unexpected output: {text}"
    );
    assert!(
        text.contains("end of replay (TRUNCATED)"),
        "unexpected output: {text}"
    );
}

#[test]
fn replay_accepts_routing_trace_events() {
    let trace_path = format!(
        "{}/tests/fixtures/routing_events_replay.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    let input = Cursor::new(format!(":replay {trace_path}\n:run\n:q\n"));
    let mut output = Vec::new();
    Repl::run(input, &mut output).expect("repl run succeeds");
    let text = String::from_utf8(output).expect("valid utf8");
    assert!(text.contains("loaded replay"), "unexpected output: {text}");
    assert!(text.contains("llm call"), "unexpected output: {text}");
    assert!(text.contains("run complete"), "unexpected output: {text}");
    assert!(text.contains("end of replay (OK)"), "unexpected output: {text}");
}
