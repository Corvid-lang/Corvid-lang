use crate::{
    parse_connector_manifest, ConnectorAuthState, ConnectorManifest, ConnectorRequest,
    ConnectorRuntime, ConnectorRuntimeError, ConnectorRuntimeMode,
};
use serde::{Deserialize, Serialize};

pub const SLACK_CONNECTOR_MANIFEST: &str = r#"
schema = "corvid.connector.v1"
name = "slack"
provider = "slack"
mode = ["mock", "replay", "real"]

[[scope]]
id = "slack.channel_read"
provider_scope = "channels:history"
data_classes = ["chat_metadata", "chat_message"]
effects = ["network.read"]
approval = "none"

[[scope]]
id = "slack.dm_read"
provider_scope = "im:history"
data_classes = ["chat_metadata", "chat_message"]
effects = ["network.read"]
approval = "none"

[[scope]]
id = "slack.thread_read"
provider_scope = "channels:history"
data_classes = ["chat_metadata", "chat_message"]
effects = ["network.read"]
approval = "none"

[[scope]]
id = "slack.draft"
provider_scope = "chat:write"
data_classes = ["chat_metadata", "chat_message", "external_recipient"]
effects = ["network.write"]
approval = "required"

[[scope]]
id = "slack.send"
provider_scope = "chat:write"
data_classes = ["chat_metadata", "chat_message", "external_recipient"]
effects = ["network.write", "send_message"]
approval = "required"

[[rate_limit]]
key = "workspace_user"
limit = 50
window_ms = 1000
retry_after = "provider_header"

[[redaction]]
field = "message.text"
strategy = "hash_and_drop"

[[replay]]
operation = "channel_read"
policy = "record_read"

[[replay]]
operation = "dm_read"
policy = "record_read"

[[replay]]
operation = "thread_read"
policy = "record_read"

[[replay]]
operation = "draft"
policy = "quarantine_write"

[[replay]]
operation = "send"
policy = "quarantine_write"
"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackMessage {
    pub id: String,
    pub workspace_id: String,
    pub channel_id: String,
    pub user_id: String,
    pub thread_ts: String,
    pub text_fingerprint: String,
    pub ts_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackReadRequest {
    pub workspace_id: String,
    pub channel_id: String,
    pub user_id: String,
    pub since_ms: u64,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackThreadRequest {
    pub workspace_id: String,
    pub channel_id: String,
    pub thread_ts: String,
    pub user_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackDraftRequest {
    pub workspace_id: String,
    pub channel_id: String,
    pub user_id: String,
    pub text: String,
    pub thread_ts: Option<String>,
    pub approval_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackSendRequest {
    pub workspace_id: String,
    pub channel_id: String,
    pub draft_id: String,
    pub approval_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackWriteReceipt {
    pub id: String,
    pub workspace_id: String,
    pub channel_id: String,
    pub approval_id: String,
    pub replay_key: String,
}

#[derive(Debug, Clone)]
pub struct SlackConnector {
    runtime: ConnectorRuntime,
}

impl SlackConnector {
    pub fn new(
        auth: ConnectorAuthState,
        mode: ConnectorRuntimeMode,
    ) -> Result<Self, toml::de::Error> {
        Ok(Self {
            runtime: ConnectorRuntime::new(slack_manifest()?, auth, mode),
        })
    }

    pub fn insert_mock(&mut self, operation: impl Into<String>, payload: serde_json::Value) {
        self.runtime.insert_mock(operation, payload);
    }

    pub fn read_channel(
        &mut self,
        request: SlackReadRequest,
        now_ms: u64,
    ) -> Result<Vec<SlackMessage>, ConnectorRuntimeError> {
        self.read_messages("slack.channel_read", "channel_read", request, now_ms)
    }

    pub fn read_dm(
        &mut self,
        request: SlackReadRequest,
        now_ms: u64,
    ) -> Result<Vec<SlackMessage>, ConnectorRuntimeError> {
        self.read_messages("slack.dm_read", "dm_read", request, now_ms)
    }

    pub fn read_thread(
        &mut self,
        request: SlackThreadRequest,
        now_ms: u64,
    ) -> Result<Vec<SlackMessage>, ConnectorRuntimeError> {
        let replay_key = format!(
            "slack:thread:{}:{}:{}:{}",
            request.workspace_id, request.channel_id, request.thread_ts, request.user_id
        );
        let response = self.runtime.execute(ConnectorRequest {
            scope_id: "slack.thread_read".to_string(),
            operation: "thread_read".to_string(),
            payload: serde_json::to_value(&request).unwrap_or_default(),
            approval_id: String::new(),
            replay_key,
            now_ms,
        })?;
        Ok(serde_json::from_value(response.payload).unwrap_or_default())
    }

    fn read_messages(
        &mut self,
        scope_id: &str,
        operation: &str,
        request: SlackReadRequest,
        now_ms: u64,
    ) -> Result<Vec<SlackMessage>, ConnectorRuntimeError> {
        let replay_key = format!(
            "slack:{}:{}:{}:{}:{}",
            operation, request.workspace_id, request.channel_id, request.user_id, request.since_ms
        );
        let response = self.runtime.execute(ConnectorRequest {
            scope_id: scope_id.to_string(),
            operation: operation.to_string(),
            payload: serde_json::to_value(&request).unwrap_or_default(),
            approval_id: String::new(),
            replay_key,
            now_ms,
        })?;
        Ok(serde_json::from_value(response.payload).unwrap_or_default())
    }

    pub fn draft_message(
        &mut self,
        request: SlackDraftRequest,
        now_ms: u64,
    ) -> Result<SlackWriteReceipt, ConnectorRuntimeError> {
        let replay_key = format!(
            "slack:draft:{}:{}:{}",
            request.workspace_id, request.channel_id, request.user_id
        );
        let approval_id = request.approval_id.clone();
        let response = self.runtime.execute(ConnectorRequest {
            scope_id: "slack.draft".to_string(),
            operation: "draft".to_string(),
            payload: serde_json::to_value(&request).unwrap_or_default(),
            approval_id,
            replay_key,
            now_ms,
        })?;
        serde_json::from_value(response.payload)
            .map_err(|err| ConnectorRuntimeError::MissingMock(err.to_string()))
    }

    pub fn send_approved(
        &mut self,
        request: SlackSendRequest,
        now_ms: u64,
    ) -> Result<SlackWriteReceipt, ConnectorRuntimeError> {
        let replay_key = format!(
            "slack:send:{}:{}:{}",
            request.workspace_id, request.channel_id, request.draft_id
        );
        let approval_id = request.approval_id.clone();
        let response = self.runtime.execute(ConnectorRequest {
            scope_id: "slack.send".to_string(),
            operation: "send".to_string(),
            payload: serde_json::to_value(&request).unwrap_or_default(),
            approval_id,
            replay_key,
            now_ms,
        })?;
        serde_json::from_value(response.payload)
            .map_err(|err| ConnectorRuntimeError::MissingMock(err.to_string()))
    }
}

pub fn slack_manifest() -> Result<ConnectorManifest, toml::de::Error> {
    parse_connector_manifest(SLACK_CONNECTOR_MANIFEST)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate_connector_manifest;

    fn auth() -> ConnectorAuthState {
        ConnectorAuthState::new(
            "workspace-1",
            "user-1",
            "token-1",
            [
                "slack.channel_read",
                "slack.dm_read",
                "slack.thread_read",
                "slack.draft",
                "slack.send",
            ],
            10_000,
        )
    }

    #[test]
    fn slack_manifest_validates_read_contract() {
        let manifest = slack_manifest().unwrap();
        let report = validate_connector_manifest(&manifest);
        assert!(report.valid, "{report:?}");
    }

    #[test]
    fn slack_channel_dm_and_thread_reads_work_in_mock_mode() {
        let mut connector = SlackConnector::new(auth(), ConnectorRuntimeMode::Mock).unwrap();
        let message = SlackMessage {
            id: "msg-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            channel_id: "C1".to_string(),
            user_id: "U1".to_string(),
            thread_ts: "123.456".to_string(),
            text_fingerprint: "sha256:abc".to_string(),
            ts_ms: 100,
        };
        connector.insert_mock("channel_read", serde_json::json!([message.clone()]));
        connector.insert_mock("dm_read", serde_json::json!([message.clone()]));
        connector.insert_mock("thread_read", serde_json::json!([message.clone()]));

        let request = SlackReadRequest {
            workspace_id: "workspace-1".to_string(),
            channel_id: "C1".to_string(),
            user_id: "U1".to_string(),
            since_ms: 0,
            limit: 10,
        };
        assert_eq!(
            connector.read_channel(request.clone(), 1).unwrap(),
            vec![message.clone()]
        );
        assert_eq!(
            connector.read_dm(request, 2).unwrap(),
            vec![message.clone()]
        );
        assert_eq!(
            connector
                .read_thread(
                    SlackThreadRequest {
                        workspace_id: "workspace-1".to_string(),
                        channel_id: "C1".to_string(),
                        thread_ts: "123.456".to_string(),
                        user_id: "U1".to_string(),
                    },
                    3,
                )
                .unwrap(),
            vec![message]
        );
    }

    #[test]
    fn slack_draft_and_send_require_approval_and_work_in_mock_mode() {
        let mut connector = SlackConnector::new(auth(), ConnectorRuntimeMode::Mock).unwrap();
        connector.insert_mock(
            "draft",
            serde_json::json!({
                "id": "draft-1",
                "workspace_id": "workspace-1",
                "channel_id": "C1",
                "approval_id": "approval-1",
                "replay_key": "slack:draft:workspace-1:C1:U1"
            }),
        );
        connector.insert_mock(
            "send",
            serde_json::json!({
                "id": "msg-1",
                "workspace_id": "workspace-1",
                "channel_id": "C1",
                "approval_id": "approval-1",
                "replay_key": "slack:send:workspace-1:C1:draft-1"
            }),
        );

        let missing = connector
            .draft_message(
                SlackDraftRequest {
                    workspace_id: "workspace-1".to_string(),
                    channel_id: "C1".to_string(),
                    user_id: "U1".to_string(),
                    text: "hello".to_string(),
                    thread_ts: None,
                    approval_id: String::new(),
                },
                1,
            )
            .unwrap_err();
        assert!(
            matches!(missing, ConnectorRuntimeError::ApprovalRequired(scope) if scope == "slack.draft")
        );

        let draft = connector
            .draft_message(
                SlackDraftRequest {
                    workspace_id: "workspace-1".to_string(),
                    channel_id: "C1".to_string(),
                    user_id: "U1".to_string(),
                    text: "hello".to_string(),
                    thread_ts: None,
                    approval_id: "approval-1".to_string(),
                },
                2,
            )
            .unwrap();
        assert_eq!(draft.id, "draft-1");

        let sent = connector
            .send_approved(
                SlackSendRequest {
                    workspace_id: "workspace-1".to_string(),
                    channel_id: "C1".to_string(),
                    draft_id: "draft-1".to_string(),
                    approval_id: "approval-1".to_string(),
                },
                3,
            )
            .unwrap();
        assert_eq!(sent.id, "msg-1");
    }

    #[test]
    fn slack_replay_quarantines_writes() {
        let mut connector = SlackConnector::new(auth(), ConnectorRuntimeMode::Replay).unwrap();
        let err = connector
            .send_approved(
                SlackSendRequest {
                    workspace_id: "workspace-1".to_string(),
                    channel_id: "C1".to_string(),
                    draft_id: "draft-1".to_string(),
                    approval_id: "approval-1".to_string(),
                },
                1,
            )
            .unwrap_err();
        assert!(
            matches!(err, ConnectorRuntimeError::ReplayWriteQuarantined(operation) if operation == "send")
        );
    }
}
