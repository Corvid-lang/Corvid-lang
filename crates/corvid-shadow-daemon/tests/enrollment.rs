use corvid_shadow_daemon::alerts::{Alert, AlertKind, AlertSeverity};
use corvid_shadow_daemon::config::EnrollmentConfig;
use corvid_shadow_daemon::EnrollmentManager;

#[test]
fn ack_command_copies_trace_into_corpus_dir() {
    let dir = tempfile::tempdir().unwrap();
    let trace = dir.path().join("trace.jsonl");
    std::fs::write(&trace, "{\"kind\":\"schema_header\"}\n").unwrap();
    let manager = EnrollmentManager::new(EnrollmentConfig {
        target_corpus_dir: dir.path().join("corpus"),
        ..Default::default()
    });
    let action = manager.enroll(&trace, "manual ack", Some("abc123".into())).unwrap();
    assert!(action.enrolled_path.exists());
}

#[test]
fn auto_enroll_on_trust_drop_copies_without_ack_when_configured() {
    let dir = tempfile::tempdir().unwrap();
    let trace = dir.path().join("trace.jsonl");
    std::fs::write(&trace, "{\"kind\":\"schema_header\"}\n").unwrap();
    let manager = EnrollmentManager::new(EnrollmentConfig {
        target_corpus_dir: dir.path().join("corpus"),
        auto_enroll: false,
        auto_enroll_on_trust_drop: true,
        auto_enroll_on_budget_overrun: false,
    });
    let alert = Alert {
        ts_ms: 0,
        severity: AlertSeverity::Warning,
        kind: AlertKind::Dimension,
        agent: "refund_bot".into(),
        trace_path: trace.clone(),
        summary: "trust drop".into(),
        payload: serde_json::json!({ "dimension": "trust" }),
    };
    let action = manager.maybe_auto_enroll(&alert).unwrap().unwrap();
    assert!(action.enrolled_path.exists());
}

#[test]
fn ack_writes_sidecar_metadata_file() {
    let dir = tempfile::tempdir().unwrap();
    let trace = dir.path().join("trace.jsonl");
    std::fs::write(&trace, "{\"kind\":\"schema_header\"}\n").unwrap();
    let manager = EnrollmentManager::new(EnrollmentConfig {
        target_corpus_dir: dir.path().join("corpus"),
        ..Default::default()
    });
    let action = manager.enroll(&trace, "manual ack", Some("abc123".into())).unwrap();
    let sidecar = std::fs::read_to_string(action.ack_path).unwrap();
    assert!(sidecar.contains("manual ack"));
}

#[test]
fn manual_ack_required_by_default_does_not_auto_copy() {
    let dir = tempfile::tempdir().unwrap();
    let trace = dir.path().join("trace.jsonl");
    std::fs::write(&trace, "{\"kind\":\"schema_header\"}\n").unwrap();
    let manager = EnrollmentManager::new(EnrollmentConfig {
        target_corpus_dir: dir.path().join("corpus"),
        auto_enroll: false,
        auto_enroll_on_trust_drop: false,
        auto_enroll_on_budget_overrun: false,
    });
    let alert = Alert {
        ts_ms: 0,
        severity: AlertSeverity::Warning,
        kind: AlertKind::Dimension,
        agent: "refund_bot".into(),
        trace_path: trace.clone(),
        summary: "trust drop".into(),
        payload: serde_json::json!({ "dimension": "trust" }),
    };
    assert!(manager.maybe_auto_enroll(&alert).unwrap().is_none());
}
