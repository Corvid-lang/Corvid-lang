//! JSONL round-trip + schema-version validation for TraceEvent.

use corvid_trace_schema::{
    read_events, read_events_from_path, schema_version_of, source_path_of,
    validate_supported_schema, write_events_to_path, ReadError, TraceEvent, SCHEMA_VERSION,
    WRITER_INTERPRETER,
};
use serde_json::json;

fn sample_header(run_id: &str) -> TraceEvent {
    TraceEvent::SchemaHeader {
        version: SCHEMA_VERSION,
        writer: WRITER_INTERPRETER.into(),
        commit_sha: Some("cafe1234".into()),
        source_path: Some("examples/refund_bot.cor".into()),
        ts_ms: 0,
        run_id: run_id.into(),
    }
}

fn sample_trace() -> Vec<TraceEvent> {
    vec![
        sample_header("r-roundtrip"),
        TraceEvent::RunStarted {
            ts_ms: 1,
            run_id: "r-roundtrip".into(),
            agent: "demo".into(),
            args: vec![json!("ticket-42")],
        },
        TraceEvent::ClockRead {
            ts_ms: 2,
            run_id: "r-roundtrip".into(),
            source: "wall".into(),
            value: 1_700_000_000_000,
        },
        TraceEvent::SeedRead {
            ts_ms: 3,
            run_id: "r-roundtrip".into(),
            purpose: "rollout_cohort".into(),
            value: 0x1234_5678_9ABC_DEF0,
        },
        TraceEvent::ToolCall {
            ts_ms: 4,
            run_id: "r-roundtrip".into(),
            tool: "get_order".into(),
            args: vec![json!("ticket-42")],
        },
        TraceEvent::ToolResult {
            ts_ms: 5,
            run_id: "r-roundtrip".into(),
            tool: "get_order".into(),
            result: json!({"id": "ticket-42", "amount": 99.0}),
        },
        TraceEvent::LlmCall {
            ts_ms: 6,
            run_id: "r-roundtrip".into(),
            prompt: "classify".into(),
            model: Some("claude-opus-4-7".into()),
            rendered: Some("Classify: ticket-42".into()),
            args: vec![],
        },
        TraceEvent::LlmResult {
            ts_ms: 7,
            run_id: "r-roundtrip".into(),
            prompt: "classify".into(),
            model: Some("claude-opus-4-7".into()),
            result: json!("refund"),
        },
        TraceEvent::ApprovalRequest {
            ts_ms: 8,
            run_id: "r-roundtrip".into(),
            label: "IssueRefund".into(),
            args: vec![json!("ticket-42"), json!(99.0)],
        },
        TraceEvent::ApprovalResponse {
            ts_ms: 9,
            run_id: "r-roundtrip".into(),
            label: "IssueRefund".into(),
            approved: true,
        },
        TraceEvent::RunCompleted {
            ts_ms: 10,
            run_id: "r-roundtrip".into(),
            ok: true,
            result: Some(json!("refunded")),
            error: None,
        },
    ]
}

#[test]
fn writes_and_reads_full_trace_roundtrip() {
    let dir = tempfile_dir();
    let path = dir.join("roundtrip.jsonl");
    let written = sample_trace();
    write_events_to_path(&path, &written).unwrap();

    let read = read_events_from_path(&path).unwrap();
    assert_eq!(read.len(), written.len());
    // Spot-check a few variants by serialized shape — Clone+PartialEq
    // aren't derived on TraceEvent, so compare via JSON text.
    for (a, b) in written.iter().zip(read.iter()) {
        let sa = serde_json::to_value(a).unwrap();
        let sb = serde_json::to_value(b).unwrap();
        assert_eq!(sa, sb);
    }
}

#[test]
fn blank_lines_between_events_are_tolerated() {
    let jsonl = format!(
        "{}\n\n{}\n",
        serde_json::to_string(&sample_header("r-blank")).unwrap(),
        serde_json::to_string(&TraceEvent::RunCompleted {
            ts_ms: 1,
            run_id: "r-blank".into(),
            ok: true,
            result: None,
            error: None,
        })
        .unwrap()
    );
    let events = read_events(jsonl.as_bytes()).unwrap();
    assert_eq!(events.len(), 2);
}

#[test]
fn corrupted_line_surfaces_line_number_and_text() {
    let jsonl = format!(
        "{}\n{{not valid json\n",
        serde_json::to_string(&sample_header("r-bad")).unwrap()
    );
    let err = read_events(jsonl.as_bytes()).unwrap_err();
    match err {
        ReadError::Parse {
            line_number, line, ..
        } => {
            assert_eq!(line_number, 2);
            assert!(line.starts_with('{'));
        }
        other => panic!("expected Parse error, got {other:?}"),
    }
}

#[test]
fn schema_version_is_readable_from_header() {
    let events = sample_trace();
    assert_eq!(schema_version_of(&events), Some(SCHEMA_VERSION));
}

#[test]
fn schema_version_is_none_for_headerless_trace() {
    let events = vec![TraceEvent::RunCompleted {
        ts_ms: 0,
        run_id: "r".into(),
        ok: true,
        result: None,
        error: None,
    }];
    assert!(schema_version_of(&events).is_none());
}

#[test]
fn validate_supported_schema_accepts_current_version() {
    let events = sample_trace();
    validate_supported_schema(&events).unwrap();
}

#[test]
fn validate_supported_schema_rejects_future_version() {
    let events = vec![TraceEvent::SchemaHeader {
        version: SCHEMA_VERSION + 1,
        writer: "corvid-vm".into(),
        commit_sha: None,
        source_path: None,
        ts_ms: 0,
        run_id: "r".into(),
    }];
    let err = validate_supported_schema(&events).unwrap_err();
    assert_eq!(err.found, SCHEMA_VERSION + 1);
    assert_eq!(err.supported, SCHEMA_VERSION);
}

#[test]
fn validate_supported_schema_accepts_legacy_v1_trace_line() {
    // A raw v1 JSONL line (pre-source_path) must still deserialize
    // and validate under the current binary. Deserialization fills
    // `source_path` with its serde default (`None`), and the range
    // check accepts v1 as a legacy-but-supported version.
    let v1_line = r#"{"kind":"schema_header","version":1,"writer":"corvid-vm","commit_sha":null,"ts_ms":0,"run_id":"r-legacy"}"#;
    let events = read_events(v1_line.as_bytes()).unwrap();
    validate_supported_schema(&events).unwrap();
    assert_eq!(schema_version_of(&events), Some(1));
    assert_eq!(source_path_of(&events), None);
}

#[test]
fn source_path_round_trips_through_jsonl() {
    let events = sample_trace();
    let jsonl: String = events
        .iter()
        .map(|e| serde_json::to_string(e).unwrap() + "\n")
        .collect();
    let read_back = read_events(jsonl.as_bytes()).unwrap();
    assert_eq!(
        source_path_of(&read_back),
        Some("examples/refund_bot.cor")
    );
}

#[test]
fn absent_source_path_serializes_as_null_and_reads_as_none() {
    let header = TraceEvent::SchemaHeader {
        version: SCHEMA_VERSION,
        writer: WRITER_INTERPRETER.into(),
        commit_sha: None,
        source_path: None,
        ts_ms: 0,
        run_id: "r-no-source".into(),
    };
    let line = serde_json::to_string(&header).unwrap();
    assert!(line.contains("\"source_path\":null"), "got: {line}");
    let round: TraceEvent = serde_json::from_str(&line).unwrap();
    assert_eq!(source_path_of(std::slice::from_ref(&round)), None);
}

#[test]
fn validate_supported_schema_accepts_headerless_trace() {
    // Legacy traces from before 21-A-schema don't carry a header.
    // Validation treats them as unknown-version and pass — readers
    // that care about the version should use `schema_version_of`.
    let events = vec![TraceEvent::RunCompleted {
        ts_ms: 0,
        run_id: "r".into(),
        ok: true,
        result: None,
        error: None,
    }];
    validate_supported_schema(&events).unwrap();
}

#[test]
fn provenance_edge_round_trips_through_jsonl() {
    let edge = TraceEvent::ProvenanceEdge {
        ts_ms: 42,
        run_id: "r-prov".into(),
        node_id: "tool:3".into(),
        parents: vec!["literal:0".into(), "tool:1".into()],
        op: "tool_call:get_order".into(),
        label: Some("order lookup".into()),
    };
    let line = serde_json::to_string(&edge).unwrap();
    assert!(line.contains("\"kind\":\"provenance_edge\""), "got: {line}");
    assert!(line.contains("\"node_id\":\"tool:3\""));
    assert!(line.contains("\"parents\":[\"literal:0\",\"tool:1\"]"));
    let round: TraceEvent = serde_json::from_str(&line).unwrap();
    match round {
        TraceEvent::ProvenanceEdge {
            node_id,
            parents,
            op,
            label,
            ..
        } => {
            assert_eq!(node_id, "tool:3");
            assert_eq!(parents, vec!["literal:0", "tool:1"]);
            assert_eq!(op, "tool_call:get_order");
            assert_eq!(label.as_deref(), Some("order lookup"));
        }
        other => panic!("expected ProvenanceEdge, got {other:?}"),
    }
}

#[test]
fn provenance_edge_with_empty_parents_and_no_label_round_trips() {
    let edge = TraceEvent::ProvenanceEdge {
        ts_ms: 1,
        run_id: "r".into(),
        node_id: "literal:0".into(),
        parents: vec![],
        op: "literal:42".into(),
        label: None,
    };
    let line = serde_json::to_string(&edge).unwrap();
    let round: TraceEvent = serde_json::from_str(&line).unwrap();
    match round {
        TraceEvent::ProvenanceEdge {
            parents, label, ..
        } => {
            assert!(parents.is_empty());
            assert!(label.is_none());
        }
        other => panic!("expected ProvenanceEdge, got {other:?}"),
    }
}

#[test]
fn seed_and_clock_events_roundtrip() {
    let events = vec![
        TraceEvent::SeedRead {
            ts_ms: 1,
            run_id: "r".into(),
            purpose: "retry_jitter".into(),
            value: 42,
        },
        TraceEvent::ClockRead {
            ts_ms: 2,
            run_id: "r".into(),
            source: "monotonic".into(),
            value: 1_500_000,
        },
    ];
    let line = serde_json::to_string(&events[0]).unwrap();
    assert!(line.contains("\"kind\":\"seed_read\""));
    assert!(line.contains("\"purpose\":\"retry_jitter\""));
    let line = serde_json::to_string(&events[1]).unwrap();
    assert!(line.contains("\"kind\":\"clock_read\""));
    assert!(line.contains("\"kind\":\"clock_read\""));
}

// Tiny in-process temp dir so we don't add a dev-dep on tempfile
// just for round-trip tests. Creates a fresh directory under
// std::env::temp_dir() named with a unique suffix, and never
// cleans up (acceptable for tests — OS wipes the temp dir eventually).
fn tempfile_dir() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("corvid-trace-schema-test-{pid}-{n}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
