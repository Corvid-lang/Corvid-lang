use crate::{
    parse_connector_manifest, ConnectorAuthState, ConnectorManifest, ConnectorRequest,
    ConnectorRuntime, ConnectorRuntimeError, ConnectorRuntimeMode,
};
use serde::{Deserialize, Serialize};

pub const MS365_CONNECTOR_MANIFEST: &str = r#"
schema = "corvid.connector.v1"
name = "microsoft365"
provider = "microsoft_graph"
mode = ["mock", "replay", "real"]

[[scope]]
id = "ms365.mail_search"
provider_scope = "Mail.Read"
data_classes = ["email_metadata", "email_body"]
effects = ["network.read"]
approval = "none"

[[scope]]
id = "ms365.calendar_events"
provider_scope = "Calendars.Read"
data_classes = ["calendar_metadata"]
effects = ["network.read"]
approval = "none"

[[rate_limit]]
key = "tenant_user"
limit = 100
window_ms = 1000
retry_after = "provider_header"

[[redaction]]
field = "message.body"
strategy = "hash_and_drop"

[[replay]]
operation = "mail_search"
policy = "record_read"

[[replay]]
operation = "calendar_events"
policy = "record_read"
"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ms365MailMessage {
    pub id: String,
    pub conversation_id: String,
    pub from: String,
    pub subject: String,
    pub received_ms: u64,
    pub has_attachments: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ms365CalendarEvent {
    pub id: String,
    pub subject: String,
    pub organizer: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub location: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ms365MailSearchRequest {
    pub user_id: String,
    pub query: String,
    pub top: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ms365CalendarEventsRequest {
    pub user_id: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Debug, Clone)]
pub struct Ms365Connector {
    runtime: ConnectorRuntime,
}

impl Ms365Connector {
    pub fn new(
        auth: ConnectorAuthState,
        mode: ConnectorRuntimeMode,
    ) -> Result<Self, toml::de::Error> {
        Ok(Self {
            runtime: ConnectorRuntime::new(ms365_manifest()?, auth, mode),
        })
    }

    pub fn insert_mock(&mut self, operation: impl Into<String>, payload: serde_json::Value) {
        self.runtime.insert_mock(operation, payload);
    }

    pub fn search_mail(
        &mut self,
        request: Ms365MailSearchRequest,
        now_ms: u64,
    ) -> Result<Vec<Ms365MailMessage>, ConnectorRuntimeError> {
        let replay_key = format!("ms365:mail:{}:{}", request.user_id, stable(&request.query));
        let response = self.runtime.execute(ConnectorRequest {
            scope_id: "ms365.mail_search".to_string(),
            operation: "mail_search".to_string(),
            payload: serde_json::to_value(&request).unwrap_or_default(),
            approval_id: String::new(),
            replay_key,
            now_ms,
        })?;
        Ok(serde_json::from_value(response.payload).unwrap_or_default())
    }

    pub fn calendar_events(
        &mut self,
        request: Ms365CalendarEventsRequest,
        now_ms: u64,
    ) -> Result<Vec<Ms365CalendarEvent>, ConnectorRuntimeError> {
        let replay_key = format!(
            "ms365:calendar:{}:{}:{}",
            request.user_id, request.start_ms, request.end_ms
        );
        let response = self.runtime.execute(ConnectorRequest {
            scope_id: "ms365.calendar_events".to_string(),
            operation: "calendar_events".to_string(),
            payload: serde_json::to_value(&request).unwrap_or_default(),
            approval_id: String::new(),
            replay_key,
            now_ms,
        })?;
        Ok(serde_json::from_value(response.payload).unwrap_or_default())
    }
}

pub fn ms365_manifest() -> Result<ConnectorManifest, toml::de::Error> {
    parse_connector_manifest(MS365_CONNECTOR_MANIFEST)
}

fn stable(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate_connector_manifest;

    fn auth() -> ConnectorAuthState {
        ConnectorAuthState::new(
            "tenant-graph",
            "actor-1",
            "token-1",
            ["ms365.mail_search", "ms365.calendar_events"],
            10_000,
        )
    }

    #[test]
    fn ms365_manifest_validates_mail_calendar_contract() {
        let manifest = ms365_manifest().unwrap();
        let report = validate_connector_manifest(&manifest);
        assert!(report.valid, "{report:?}");
    }

    #[test]
    fn ms365_mail_and_calendar_work_in_mock_mode() {
        let mut connector = Ms365Connector::new(auth(), ConnectorRuntimeMode::Mock).unwrap();
        let mail = Ms365MailMessage {
            id: "m1".to_string(),
            conversation_id: "c1".to_string(),
            from: "a@example.com".to_string(),
            subject: "Planning".to_string(),
            received_ms: 1,
            has_attachments: false,
        };
        let event = Ms365CalendarEvent {
            id: "e1".to_string(),
            subject: "Weekly".to_string(),
            organizer: "a@example.com".to_string(),
            start_ms: 10,
            end_ms: 20,
            location: "Teams".to_string(),
        };
        connector.insert_mock("mail_search", serde_json::json!([mail.clone()]));
        connector.insert_mock("calendar_events", serde_json::json!([event.clone()]));

        let mail_results = connector
            .search_mail(
                Ms365MailSearchRequest {
                    user_id: "me".to_string(),
                    query: "isRead eq false".to_string(),
                    top: 10,
                },
                1,
            )
            .unwrap();
        assert_eq!(mail_results, vec![mail]);

        let calendar_results = connector
            .calendar_events(
                Ms365CalendarEventsRequest {
                    user_id: "me".to_string(),
                    start_ms: 0,
                    end_ms: 100,
                },
                2,
            )
            .unwrap();
        assert_eq!(calendar_results, vec![event]);
    }
}
