use corvid_connector_runtime::{
    CalendarAvailabilityRequest, CalendarConnector, CalendarWriteRequest, ConnectorAuthState,
    ConnectorRuntimeMode, FileConnector, FileIndexRequest, GitHubIssueSearchRequest,
    GmailConnector, GmailDraftRequest, SlackConnector, SlackDraftRequest, TaskConnector,
    TaskWriteKind, TaskWriteRequest,
};

fn auth(scopes: &[&str]) -> ConnectorAuthState {
    ConnectorAuthState::new(
        "tenant-1",
        "actor-1",
        "token-1",
        scopes.iter().copied(),
        10_000,
    )
}

#[test]
fn executive_agent_uses_email_calendar_tasks_chat_and_files_through_connector_mocks() {
    let mut gmail = GmailConnector::new(
        auth(&["gmail.search", "gmail.draft", "gmail.send"]),
        ConnectorRuntimeMode::Mock,
    )
    .unwrap();
    gmail.insert_mock("search", serde_json::json!([]));
    gmail.insert_mock(
        "draft",
        serde_json::json!({
            "id": "draft-1",
            "thread_id": "thread-1",
            "approval_id": "approval-email",
            "replay_key": "gmail:draft:me:Follow-up"
        }),
    );

    let mut calendar = CalendarConnector::new(
        auth(&["calendar.availability", "calendar.create"]),
        ConnectorRuntimeMode::Mock,
    )
    .unwrap();
    calendar.insert_mock(
        "availability",
        serde_json::json!([{
            "calendar_id": "primary",
            "start_ms": 100,
            "end_ms": 200,
            "confidence": 100
        }]),
    );
    calendar.insert_mock(
        "create",
        serde_json::json!({
            "event_id": "evt-1",
            "calendar_id": "primary",
            "approval_id": "approval-calendar",
            "replay_key": "calendar:create:me:primary:100"
        }),
    );

    let mut tasks = TaskConnector::new(
        auth(&["tasks.github_search", "tasks.github_write"]),
        ConnectorRuntimeMode::Mock,
    )
    .unwrap();
    tasks.insert_mock("github_search", serde_json::json!([]));
    tasks.insert_mock(
        "github_write",
        serde_json::json!({
            "provider": "github",
            "id": "gh-1",
            "key": "#1",
            "approval_id": "approval-task",
            "replay_key": "tasks:github_write:corvid-lang/corvid:new:Create"
        }),
    );

    let mut slack = SlackConnector::new(
        auth(&["slack.channel_read", "slack.draft"]),
        ConnectorRuntimeMode::Mock,
    )
    .unwrap();
    slack.insert_mock("channel_read", serde_json::json!([]));
    slack.insert_mock(
        "draft",
        serde_json::json!({
            "id": "draft-1",
            "workspace_id": "workspace-1",
            "channel_id": "C1",
            "approval_id": "approval-chat",
            "replay_key": "slack:draft:workspace-1:C1:U1"
        }),
    );

    let mut files = FileConnector::new(
        auth(&["files.index", "files.read"]),
        ConnectorRuntimeMode::Mock,
    )
    .unwrap();
    files.insert_mock("index", serde_json::json!([]));

    assert!(gmail
        .search_metadata(
            corvid_connector_runtime::GmailSearchRequest {
                user_id: "me".to_string(),
                query: "is:unread".to_string(),
                max_results: 10,
            },
            1,
        )
        .unwrap()
        .is_empty());
    assert_eq!(
        gmail
            .draft_reply(
                GmailDraftRequest {
                    user_id: "me".to_string(),
                    to: vec!["external@example.com".to_string()],
                    subject: "Follow-up".to_string(),
                    body: "hello".to_string(),
                    thread_id: Some("thread-1".to_string()),
                    approval_id: "approval-email".to_string(),
                },
                2,
            )
            .unwrap()
            .approval_id,
        "approval-email"
    );

    assert_eq!(
        calendar
            .availability(
                CalendarAvailabilityRequest {
                    user_id: "me".to_string(),
                    calendar_ids: vec!["primary".to_string()],
                    start_ms: 0,
                    end_ms: 1000,
                    duration_ms: 100,
                },
                3,
            )
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        calendar
            .create_event(
                CalendarWriteRequest {
                    user_id: "me".to_string(),
                    calendar_id: "primary".to_string(),
                    event_id: None,
                    title: "Planning".to_string(),
                    start_ms: 100,
                    end_ms: 200,
                    attendees: vec!["external@example.com".to_string()],
                    approval_id: "approval-calendar".to_string(),
                },
                4,
            )
            .unwrap()
            .approval_id,
        "approval-calendar"
    );

    assert!(tasks
        .search_github(
            GitHubIssueSearchRequest {
                owner: "corvid-lang".to_string(),
                repo: "corvid".to_string(),
                query: "is:open".to_string(),
                limit: 10,
            },
            5,
        )
        .unwrap()
        .is_empty());
    assert_eq!(
        tasks
            .write_github(
                TaskWriteRequest {
                    provider: "github".to_string(),
                    workspace_or_repo: "corvid-lang/corvid".to_string(),
                    issue_id: None,
                    title: "Task".to_string(),
                    body: "body".to_string(),
                    kind: TaskWriteKind::Create,
                    approval_id: "approval-task".to_string(),
                },
                6,
            )
            .unwrap()
            .approval_id,
        "approval-task"
    );

    assert!(slack
        .read_channel(
            corvid_connector_runtime::SlackReadRequest {
                workspace_id: "workspace-1".to_string(),
                channel_id: "C1".to_string(),
                user_id: "U1".to_string(),
                since_ms: 0,
                limit: 10,
            },
            7,
        )
        .unwrap()
        .is_empty());
    assert_eq!(
        slack
            .draft_message(
                SlackDraftRequest {
                    workspace_id: "workspace-1".to_string(),
                    channel_id: "C1".to_string(),
                    user_id: "U1".to_string(),
                    text: "hello".to_string(),
                    thread_ts: None,
                    approval_id: "approval-chat".to_string(),
                },
                8,
            )
            .unwrap()
            .approval_id,
        "approval-chat"
    );

    assert!(files
        .index(
            FileIndexRequest {
                root_id: "docs".to_string(),
                glob: "**/*.md".to_string(),
            },
            9,
        )
        .unwrap()
        .is_empty());
}
