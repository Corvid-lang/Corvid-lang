use crate::{
    parse_connector_manifest, ConnectorAuthState, ConnectorManifest, ConnectorRequest,
    ConnectorRuntime, ConnectorRuntimeError, ConnectorRuntimeMode,
};
use serde::{Deserialize, Serialize};

pub const GMAIL_CONNECTOR_MANIFEST: &str = r#"
schema = "corvid.connector.v1"
name = "gmail"
provider = "google"
mode = ["mock", "replay", "real"]

[[scope]]
id = "gmail.read_metadata"
provider_scope = "https://www.googleapis.com/auth/gmail.metadata"
data_classes = ["email_metadata"]
effects = ["network.read"]
approval = "none"

[[scope]]
id = "gmail.search"
provider_scope = "https://www.googleapis.com/auth/gmail.metadata"
data_classes = ["email_metadata"]
effects = ["network.read"]
approval = "none"

[[scope]]
id = "gmail.draft"
provider_scope = "https://www.googleapis.com/auth/gmail.compose"
data_classes = ["email_metadata", "email_body", "external_recipient"]
effects = ["network.write"]
approval = "required"

[[scope]]
id = "gmail.send"
provider_scope = "https://www.googleapis.com/auth/gmail.send"
data_classes = ["email_metadata", "email_body", "external_recipient"]
effects = ["network.write", "send_email"]
approval = "required"

[[rate_limit]]
key = "user_id"
limit = 250
window_ms = 1000
retry_after = "provider_header"

[[redaction]]
field = "message.snippet"
strategy = "hash_and_drop"

[[replay]]
operation = "read_metadata"
policy = "record_read"

[[replay]]
operation = "search"
policy = "record_read"

[[replay]]
operation = "draft"
policy = "quarantine_write"

[[replay]]
operation = "send"
policy = "quarantine_write"
"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GmailMessageMetadata {
    pub id: String,
    pub thread_id: String,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub received_ms: u64,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GmailSearchRequest {
    pub user_id: String,
    pub query: String,
    pub max_results: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GmailDraftRequest {
    pub user_id: String,
    pub to: Vec<String>,
    pub subject: String,
    pub body: String,
    pub thread_id: Option<String>,
    pub approval_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GmailSendRequest {
    pub user_id: String,
    pub draft_id: String,
    pub approval_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GmailWriteReceipt {
    pub id: String,
    pub thread_id: String,
    pub approval_id: String,
    pub replay_key: String,
}

#[derive(Debug, Clone)]
pub struct GmailConnector {
    runtime: ConnectorRuntime,
}

impl GmailConnector {
    pub fn new(
        auth: ConnectorAuthState,
        mode: ConnectorRuntimeMode,
    ) -> Result<Self, toml::de::Error> {
        Ok(Self {
            runtime: ConnectorRuntime::new(gmail_manifest()?, auth, mode),
        })
    }

    pub fn insert_mock(&mut self, operation: impl Into<String>, payload: serde_json::Value) {
        self.runtime.insert_mock(operation, payload);
    }

    pub fn search_metadata(
        &mut self,
        request: GmailSearchRequest,
        now_ms: u64,
    ) -> Result<Vec<GmailMessageMetadata>, ConnectorRuntimeError> {
        let replay_key = format!(
            "gmail:search:{}:{}",
            request.user_id,
            stable_query(&request.query)
        );
        let response = self.runtime.execute(ConnectorRequest {
            scope_id: "gmail.search".to_string(),
            operation: "search".to_string(),
            payload: serde_json::to_value(&request).unwrap_or_default(),
            approval_id: String::new(),
            replay_key,
            now_ms,
        })?;
        Ok(serde_json::from_value(response.payload).unwrap_or_default())
    }

    pub fn read_metadata(
        &mut self,
        user_id: &str,
        message_id: &str,
        now_ms: u64,
    ) -> Result<GmailMessageMetadata, ConnectorRuntimeError> {
        let response = self.runtime.execute(ConnectorRequest {
            scope_id: "gmail.read_metadata".to_string(),
            operation: "read_metadata".to_string(),
            payload: serde_json::json!({ "user_id": user_id, "message_id": message_id }),
            approval_id: String::new(),
            replay_key: format!("gmail:message:{user_id}:{message_id}"),
            now_ms,
        })?;
        serde_json::from_value(response.payload)
            .map_err(|err| ConnectorRuntimeError::MissingMock(err.to_string()))
    }

    pub fn draft_reply(
        &mut self,
        request: GmailDraftRequest,
        now_ms: u64,
    ) -> Result<GmailWriteReceipt, ConnectorRuntimeError> {
        let replay_key = format!(
            "gmail:draft:{}:{}",
            request.user_id,
            stable_query(&request.subject)
        );
        let approval_id = request.approval_id.clone();
        let response = self.runtime.execute(ConnectorRequest {
            scope_id: "gmail.draft".to_string(),
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
        request: GmailSendRequest,
        now_ms: u64,
    ) -> Result<GmailWriteReceipt, ConnectorRuntimeError> {
        let replay_key = format!("gmail:send:{}:{}", request.user_id, request.draft_id);
        let approval_id = request.approval_id.clone();
        let response = self.runtime.execute(ConnectorRequest {
            scope_id: "gmail.send".to_string(),
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

pub fn gmail_manifest() -> Result<ConnectorManifest, toml::de::Error> {
    parse_connector_manifest(GMAIL_CONNECTOR_MANIFEST)
}

fn stable_query(query: &str) -> String {
    query
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{validate_connector_manifest, ConnectorRuntimeMode};

    fn auth() -> ConnectorAuthState {
        ConnectorAuthState::new(
            "tenant-1",
            "actor-1",
            "token-1",
            [
                "gmail.read_metadata",
                "gmail.search",
                "gmail.draft",
                "gmail.send",
            ],
            10_000,
        )
    }

    #[test]
    fn gmail_manifest_validates_read_search_contract() {
        let manifest = gmail_manifest().unwrap();
        let report = validate_connector_manifest(&manifest);
        assert!(report.valid, "{report:?}");
    }

    #[test]
    fn gmail_search_and_read_metadata_work_in_mock_mode() {
        let mut connector = GmailConnector::new(auth(), ConnectorRuntimeMode::Mock).unwrap();
        let message = GmailMessageMetadata {
            id: "m1".to_string(),
            thread_id: "t1".to_string(),
            from: "a@example.com".to_string(),
            to: "b@example.com".to_string(),
            subject: "Planning".to_string(),
            received_ms: 1_700_000_000_000,
            labels: vec!["INBOX".to_string()],
        };
        connector.insert_mock("search", serde_json::json!([message.clone()]));
        connector.insert_mock("read_metadata", serde_json::json!(message.clone()));

        let results = connector
            .search_metadata(
                GmailSearchRequest {
                    user_id: "me".to_string(),
                    query: "is:unread newer_than:1d".to_string(),
                    max_results: 10,
                },
                1,
            )
            .unwrap();
        assert_eq!(results, vec![message.clone()]);

        let read = connector.read_metadata("me", "m1", 2).unwrap();
        assert_eq!(read, message);
    }

    #[test]
    fn gmail_draft_and_send_require_approval_and_work_in_mock_mode() {
        let mut connector = GmailConnector::new(auth(), ConnectorRuntimeMode::Mock).unwrap();
        connector.insert_mock(
            "draft",
            serde_json::json!({
                "id": "draft-1",
                "thread_id": "t1",
                "approval_id": "approval-1",
                "replay_key": "gmail:draft:me:Planning"
            }),
        );
        connector.insert_mock(
            "send",
            serde_json::json!({
                "id": "sent-1",
                "thread_id": "t1",
                "approval_id": "approval-1",
                "replay_key": "gmail:send:me:draft-1"
            }),
        );

        let missing = connector
            .draft_reply(
                GmailDraftRequest {
                    user_id: "me".to_string(),
                    to: vec!["a@example.com".to_string()],
                    subject: "Planning".to_string(),
                    body: "hello".to_string(),
                    thread_id: Some("t1".to_string()),
                    approval_id: String::new(),
                },
                1,
            )
            .unwrap_err();
        assert!(
            matches!(missing, ConnectorRuntimeError::ApprovalRequired(scope) if scope == "gmail.draft")
        );

        let draft = connector
            .draft_reply(
                GmailDraftRequest {
                    user_id: "me".to_string(),
                    to: vec!["a@example.com".to_string()],
                    subject: "Planning".to_string(),
                    body: "hello".to_string(),
                    thread_id: Some("t1".to_string()),
                    approval_id: "approval-1".to_string(),
                },
                2,
            )
            .unwrap();
        assert_eq!(draft.id, "draft-1");

        let sent = connector
            .send_approved(
                GmailSendRequest {
                    user_id: "me".to_string(),
                    draft_id: "draft-1".to_string(),
                    approval_id: "approval-1".to_string(),
                },
                3,
            )
            .unwrap();
        assert_eq!(sent.id, "sent-1");
    }
}
