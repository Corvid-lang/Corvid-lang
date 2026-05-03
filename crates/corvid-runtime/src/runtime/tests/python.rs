use super::*;
use serde_json::json;
#[cfg(feature = "python")]
#[test]
fn runtime_python_calls_emit_host_events() {
    let dir = tempfile::tempdir().expect("tempdir");
    let trace_path = dir.path().join("python.jsonl");
    let runtime = Runtime::builder()
        .tracer(Tracer::open_path(&trace_path, "python-run"))
        .build();

    let value = runtime
        .call_python_function("math", "sqrt", vec![json!(16.0)])
        .expect("python call");
    assert_eq!(value, json!(4.0));

    let events = corvid_trace_schema::read_events_from_path(&trace_path).unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::HostEvent { name, payload, .. }
            if name == "python.call"
                && payload["module"] == "math"
                && payload["function"] == "sqrt"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::HostEvent { name, payload, .. }
            if name == "python.result" && payload["result"] == json!(4.0)
    )));
}

#[cfg(feature = "python")]
#[test]
fn runtime_python_round_trips_lists_and_objects() {
    let dir = tempfile::tempdir().expect("tempdir");
    let trace_path = dir.path().join("python-roundtrip.jsonl");
    let runtime = Runtime::builder()
        .tracer(Tracer::open_path(&trace_path, "python-roundtrip-run"))
        .build();

    let list = runtime
        .call_python_function("json", "loads", vec![json!(r#"[1,true,"x"]"#)])
        .expect("python list round-trip");
    assert_eq!(list, json!([1, true, "x"]));

    let object = runtime
        .call_python_function(
            "json",
            "loads",
            vec![json!(r#"{"id":"rec-1","count":2,"ok":true}"#)],
        )
        .expect("python dict/object round-trip");
    assert_eq!(object, json!({"id": "rec-1", "count": 2, "ok": true}));

    let events = corvid_trace_schema::read_events_from_path(&trace_path).unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::HostEvent { name, payload, .. }
            if name == "python.result"
                && payload["result"] == json!([1, true, "x"])
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::HostEvent { name, payload, .. }
            if name == "python.result"
                && payload["result"] == json!({"id": "rec-1", "count": 2, "ok": true})
    )));
}

#[cfg(feature = "python")]
#[test]
fn runtime_python_errors_emit_host_events() {
    let dir = tempfile::tempdir().expect("tempdir");
    let trace_path = dir.path().join("python-error.jsonl");
    let runtime = Runtime::builder()
        .tracer(Tracer::open_path(&trace_path, "python-error-run"))
        .build();

    let err = runtime
        .call_python_function("math", "sqrt", vec![json!(-1.0)])
        .expect_err("python error");
    match err {
        RuntimeError::PythonFailed { traceback, .. } => {
            assert!(traceback.contains("Traceback"), "{traceback}");
            assert!(traceback.contains("ValueError"), "{traceback}");
        }
        other => panic!("unexpected python error: {other}"),
    }

    let events = corvid_trace_schema::read_events_from_path(&trace_path).unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::HostEvent { name, payload, .. }
            if name == "python.call"
                && payload["module"] == "math"
                && payload["function"] == "sqrt"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::HostEvent { name, payload, .. }
            if name == "python.error"
                && payload["module"] == "math"
                && payload["function"] == "sqrt"
                && payload["error"].as_str().is_some_and(|error| error.contains("ValueError"))
    )));
}

#[cfg(feature = "python")]
#[test]
fn runtime_python_policy_denials_are_trace_visible() {
    let dir = tempfile::tempdir().expect("tempdir");
    let trace_path = dir.path().join("python-policy.jsonl");
    let runtime = Runtime::builder()
        .tracer(Tracer::open_path(&trace_path, "python-policy-run"))
        .build();

    let err = runtime
        .call_python_function_with_policy(
            "os",
            "getcwd",
            vec![],
            &PythonSandboxProfile::new(["network"]),
        )
        .expect_err("policy denial");
    assert!(matches!(
        err,
        RuntimeError::PythonPolicyDenied {
            required_effect,
            ..
        } if required_effect == "filesystem"
    ));

    let events = corvid_trace_schema::read_events_from_path(&trace_path).unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::HostEvent { name, payload, .. }
            if name == "python.call"
                && payload["module"] == "os"
                && payload["function"] == "getcwd"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::HostEvent { name, payload, .. }
            if name == "python.error"
                && payload["error"]
                    .as_str()
                    .is_some_and(|error| error.contains("filesystem"))
    )));
}
