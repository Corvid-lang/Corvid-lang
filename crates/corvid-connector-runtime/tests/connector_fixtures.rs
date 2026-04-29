use corvid_connector_runtime::{
    run_connector_fixture, CALENDAR_CONNECTOR_MANIFEST, FILE_CONNECTOR_MANIFEST,
    GMAIL_CONNECTOR_MANIFEST, MS365_CONNECTOR_MANIFEST, SLACK_CONNECTOR_MANIFEST,
    TASK_CONNECTOR_MANIFEST,
};

fn fixture(
    manifest: &str,
    scopes: &[&str],
    scope_id: &str,
    operation: &str,
    response: serde_json::Value,
    approval_id: &str,
) -> corvid_connector_runtime::ConnectorFixture {
    corvid_connector_runtime::ConnectorFixture {
        manifest: manifest.to_string(),
        tenant_id: "tenant-1".to_string(),
        actor_id: "actor-1".to_string(),
        token_id: "token-1".to_string(),
        scopes: scopes.iter().map(|scope| scope.to_string()).collect(),
        expires_at_ms: 10_000,
        scope_id: scope_id.to_string(),
        operation: operation.to_string(),
        request: serde_json::json!({}),
        response,
        approval_id: approval_id.to_string(),
        replay_key: format!("{operation}:fixture"),
        now_ms: 1,
    }
}

#[test]
fn every_connector_has_mock_replay_fixture_coverage() {
    let cases = vec![
        fixture(
            GMAIL_CONNECTOR_MANIFEST,
            &["gmail.search"],
            "gmail.search",
            "search",
            serde_json::json!([]),
            "",
        ),
        fixture(
            MS365_CONNECTOR_MANIFEST,
            &["ms365.mail_search"],
            "ms365.mail_search",
            "mail_search",
            serde_json::json!([]),
            "",
        ),
        fixture(
            CALENDAR_CONNECTOR_MANIFEST,
            &["calendar.availability"],
            "calendar.availability",
            "availability",
            serde_json::json!([]),
            "",
        ),
        fixture(
            SLACK_CONNECTOR_MANIFEST,
            &["slack.channel_read"],
            "slack.channel_read",
            "channel_read",
            serde_json::json!([]),
            "",
        ),
        fixture(
            TASK_CONNECTOR_MANIFEST,
            &["tasks.linear_search"],
            "tasks.linear_search",
            "linear_search",
            serde_json::json!([]),
            "",
        ),
        fixture(
            FILE_CONNECTOR_MANIFEST,
            &["files.index"],
            "files.index",
            "index",
            serde_json::json!([]),
            "",
        ),
    ];

    for case in cases {
        let report = run_connector_fixture(&case).unwrap();
        assert!(report.mock_ok, "{report:?}");
        assert!(report.replay_ok, "{report:?}");
    }
}

#[test]
fn write_connectors_quarantine_replay_fixtures() {
    let cases = vec![
        fixture(
            GMAIL_CONNECTOR_MANIFEST,
            &["gmail.send"],
            "gmail.send",
            "send",
            serde_json::json!({"id": "sent"}),
            "approval-1",
        ),
        fixture(
            CALENDAR_CONNECTOR_MANIFEST,
            &["calendar.create"],
            "calendar.create",
            "create",
            serde_json::json!({"event_id": "evt"}),
            "approval-1",
        ),
        fixture(
            SLACK_CONNECTOR_MANIFEST,
            &["slack.send"],
            "slack.send",
            "send",
            serde_json::json!({"id": "msg"}),
            "approval-1",
        ),
        fixture(
            TASK_CONNECTOR_MANIFEST,
            &["tasks.github_write"],
            "tasks.github_write",
            "github_write",
            serde_json::json!({"id": "issue"}),
            "approval-1",
        ),
        fixture(
            FILE_CONNECTOR_MANIFEST,
            &["files.write"],
            "files.write",
            "write",
            serde_json::json!({"path": "notes.md"}),
            "approval-1",
        ),
    ];

    for case in cases {
        let report = run_connector_fixture(&case).unwrap();
        assert!(report.mock_ok, "{report:?}");
        assert!(report.replay_write_quarantined, "{report:?}");
    }
}
