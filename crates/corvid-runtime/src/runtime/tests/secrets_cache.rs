use super::*;
use serde_json::json;
#[test]
fn secret_reads_are_trace_visible_without_secret_value() {
    std::env::set_var("CORVID_TEST_RUNTIME_SECRET", "super-secret");
    let dir = tempfile::tempdir().unwrap();
    let trace_path = dir.path().join("secret.jsonl");
    let r = Runtime::builder()
        .tracer(Tracer::open_path(&trace_path, "secret-run"))
        .build();

    let read = r.read_env_secret("CORVID_TEST_RUNTIME_SECRET").unwrap();
    assert!(read.present);
    assert_eq!(read.value.as_deref(), Some("super-secret"));

    let events = corvid_trace_schema::read_events_from_path(&trace_path).unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::HostEvent { name, payload, .. }
            if name == "std.secrets.read"
                && payload["name"] == "CORVID_TEST_RUNTIME_SECRET"
                && payload["present"] == true
                && payload.get("value").is_none()
    )));
}

#[test]
fn cache_keys_are_trace_visible_without_cached_payloads() {
    let dir = tempfile::tempdir().unwrap();
    let trace_path = dir.path().join("cache.jsonl");
    let r = Runtime::builder()
        .tracer(Tracer::open_path(&trace_path, "cache-run"))
        .build();

    let key = r
        .cache_key(CacheKeyInput {
            namespace: "tool".to_string(),
            subject: "lookup".to_string(),
            model: None,
            effect_key: Some("io:read".to_string()),
            provenance_key: Some("doc:123".to_string()),
            version: Some("v1".to_string()),
            args: json!({"id": 7}),
        })
        .unwrap();

    assert_eq!(key.namespace, "tool");
    assert_eq!(key.subject, "lookup");
    assert_eq!(key.fingerprint.len(), 64);

    let events = corvid_trace_schema::read_events_from_path(&trace_path).unwrap();
    let event = events
        .iter()
        .find_map(|event| match event {
            TraceEvent::HostEvent { name, payload, .. } if name == "std.cache.key" => Some(payload),
            _ => None,
        })
        .expect("std.cache key event");
    assert_eq!(event["namespace"], json!("tool"));
    assert_eq!(event["subject"], json!("lookup"));
    assert_eq!(event["effect_key"], json!("io:read"));
    assert_eq!(event["provenance_key"], json!("doc:123"));
    assert_eq!(event.get("value"), None);
    assert_eq!(event.get("payload"), None);
}
