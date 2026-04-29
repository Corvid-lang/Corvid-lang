use crate::{
    parse_connector_manifest, ConnectorAuthState, ConnectorManifest, ConnectorRequest,
    ConnectorRuntime, ConnectorRuntimeError, ConnectorRuntimeMode,
};
use serde::{Deserialize, Serialize};

pub const CALENDAR_CONNECTOR_MANIFEST: &str = r#"
schema = "corvid.connector.v1"
name = "calendar"
provider = "calendar"
mode = ["mock", "replay", "real"]

[[scope]]
id = "calendar.availability"
provider_scope = "calendar.read"
data_classes = ["calendar_metadata", "availability"]
effects = ["network.read"]
approval = "none"

[[scope]]
id = "calendar.events"
provider_scope = "calendar.read"
data_classes = ["calendar_metadata"]
effects = ["network.read"]
approval = "none"

[[scope]]
id = "calendar.create"
provider_scope = "calendar.write"
data_classes = ["calendar_metadata", "external_recipient"]
effects = ["network.write", "calendar.write"]
approval = "required"

[[scope]]
id = "calendar.update"
provider_scope = "calendar.write"
data_classes = ["calendar_metadata", "external_recipient"]
effects = ["network.write", "calendar.write"]
approval = "required"

[[scope]]
id = "calendar.cancel"
provider_scope = "calendar.write"
data_classes = ["calendar_metadata", "external_recipient"]
effects = ["network.write", "calendar.write"]
approval = "required"

[[rate_limit]]
key = "tenant_user"
limit = 100
window_ms = 1000
retry_after = "provider_header"

[[redaction]]
field = "event.description"
strategy = "hash_and_drop"

[[replay]]
operation = "availability"
policy = "record_read"

[[replay]]
operation = "events"
policy = "record_read"

[[replay]]
operation = "create"
policy = "quarantine_write"

[[replay]]
operation = "update"
policy = "quarantine_write"

[[replay]]
operation = "cancel"
policy = "quarantine_write"
"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarAvailabilityRequest {
    pub user_id: String,
    pub calendar_ids: Vec<String>,
    pub start_ms: u64,
    pub end_ms: u64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarAvailabilitySlot {
    pub calendar_id: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub confidence: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarEventReadRequest {
    pub user_id: String,
    pub calendar_id: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarEvent {
    pub id: String,
    pub calendar_id: String,
    pub title: String,
    pub organizer: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub attendee_count: u32,
    pub external_attendee_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarWriteRequest {
    pub user_id: String,
    pub calendar_id: String,
    pub event_id: Option<String>,
    pub title: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub attendees: Vec<String>,
    pub approval_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarCancelRequest {
    pub user_id: String,
    pub calendar_id: String,
    pub event_id: String,
    pub reason: String,
    pub approval_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarWriteReceipt {
    pub event_id: String,
    pub calendar_id: String,
    pub approval_id: String,
    pub replay_key: String,
}

#[derive(Debug, Clone)]
pub struct CalendarConnector {
    runtime: ConnectorRuntime,
}

impl CalendarConnector {
    pub fn new(
        auth: ConnectorAuthState,
        mode: ConnectorRuntimeMode,
    ) -> Result<Self, toml::de::Error> {
        Ok(Self {
            runtime: ConnectorRuntime::new(calendar_manifest()?, auth, mode),
        })
    }

    pub fn insert_mock(&mut self, operation: impl Into<String>, payload: serde_json::Value) {
        self.runtime.insert_mock(operation, payload);
    }

    pub fn availability(
        &mut self,
        request: CalendarAvailabilityRequest,
        now_ms: u64,
    ) -> Result<Vec<CalendarAvailabilitySlot>, ConnectorRuntimeError> {
        let replay_key = format!(
            "calendar:availability:{}:{}:{}:{}",
            request.user_id, request.start_ms, request.end_ms, request.duration_ms
        );
        let response = self.runtime.execute(ConnectorRequest {
            scope_id: "calendar.availability".to_string(),
            operation: "availability".to_string(),
            payload: serde_json::to_value(&request).unwrap_or_default(),
            approval_id: String::new(),
            replay_key,
            now_ms,
        })?;
        Ok(serde_json::from_value(response.payload).unwrap_or_default())
    }

    pub fn events(
        &mut self,
        request: CalendarEventReadRequest,
        now_ms: u64,
    ) -> Result<Vec<CalendarEvent>, ConnectorRuntimeError> {
        let replay_key = format!(
            "calendar:events:{}:{}:{}:{}",
            request.user_id, request.calendar_id, request.start_ms, request.end_ms
        );
        let response = self.runtime.execute(ConnectorRequest {
            scope_id: "calendar.events".to_string(),
            operation: "events".to_string(),
            payload: serde_json::to_value(&request).unwrap_or_default(),
            approval_id: String::new(),
            replay_key,
            now_ms,
        })?;
        Ok(serde_json::from_value(response.payload).unwrap_or_default())
    }

    pub fn create_event(
        &mut self,
        request: CalendarWriteRequest,
        now_ms: u64,
    ) -> Result<CalendarWriteReceipt, ConnectorRuntimeError> {
        self.write_event("calendar.create", "create", request, now_ms)
    }

    pub fn update_event(
        &mut self,
        request: CalendarWriteRequest,
        now_ms: u64,
    ) -> Result<CalendarWriteReceipt, ConnectorRuntimeError> {
        self.write_event("calendar.update", "update", request, now_ms)
    }

    pub fn cancel_event(
        &mut self,
        request: CalendarCancelRequest,
        now_ms: u64,
    ) -> Result<CalendarWriteReceipt, ConnectorRuntimeError> {
        let replay_key = format!(
            "calendar:cancel:{}:{}:{}",
            request.user_id, request.calendar_id, request.event_id
        );
        let approval_id = request.approval_id.clone();
        let response = self.runtime.execute(ConnectorRequest {
            scope_id: "calendar.cancel".to_string(),
            operation: "cancel".to_string(),
            payload: serde_json::to_value(&request).unwrap_or_default(),
            approval_id,
            replay_key,
            now_ms,
        })?;
        serde_json::from_value(response.payload)
            .map_err(|err| ConnectorRuntimeError::MissingMock(err.to_string()))
    }

    fn write_event(
        &mut self,
        scope_id: &str,
        operation: &str,
        request: CalendarWriteRequest,
        now_ms: u64,
    ) -> Result<CalendarWriteReceipt, ConnectorRuntimeError> {
        let replay_key = format!(
            "calendar:{}:{}:{}:{}",
            operation, request.user_id, request.calendar_id, request.start_ms
        );
        let approval_id = request.approval_id.clone();
        let response = self.runtime.execute(ConnectorRequest {
            scope_id: scope_id.to_string(),
            operation: operation.to_string(),
            payload: serde_json::to_value(&request).unwrap_or_default(),
            approval_id,
            replay_key,
            now_ms,
        })?;
        serde_json::from_value(response.payload)
            .map_err(|err| ConnectorRuntimeError::MissingMock(err.to_string()))
    }
}

pub fn calendar_manifest() -> Result<ConnectorManifest, toml::de::Error> {
    parse_connector_manifest(CALENDAR_CONNECTOR_MANIFEST)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate_connector_manifest;

    fn auth() -> ConnectorAuthState {
        ConnectorAuthState::new(
            "tenant-1",
            "actor-1",
            "token-1",
            [
                "calendar.availability",
                "calendar.events",
                "calendar.create",
                "calendar.update",
                "calendar.cancel",
            ],
            10_000,
        )
    }

    #[test]
    fn calendar_manifest_validates_read_contract() {
        let manifest = calendar_manifest().unwrap();
        let report = validate_connector_manifest(&manifest);
        assert!(report.valid, "{report:?}");
    }

    #[test]
    fn calendar_availability_and_event_reads_work_in_mock_mode() {
        let mut connector = CalendarConnector::new(auth(), ConnectorRuntimeMode::Mock).unwrap();
        let slot = CalendarAvailabilitySlot {
            calendar_id: "primary".to_string(),
            start_ms: 100,
            end_ms: 200,
            confidence: 100,
        };
        let event = CalendarEvent {
            id: "evt-1".to_string(),
            calendar_id: "primary".to_string(),
            title: "Planning".to_string(),
            organizer: "a@example.com".to_string(),
            start_ms: 300,
            end_ms: 400,
            attendee_count: 3,
            external_attendee_count: 1,
        };
        connector.insert_mock("availability", serde_json::json!([slot.clone()]));
        connector.insert_mock("events", serde_json::json!([event.clone()]));

        let availability = connector
            .availability(
                CalendarAvailabilityRequest {
                    user_id: "me".to_string(),
                    calendar_ids: vec!["primary".to_string()],
                    start_ms: 0,
                    end_ms: 1000,
                    duration_ms: 100,
                },
                1,
            )
            .unwrap();
        assert_eq!(availability, vec![slot]);

        let events = connector
            .events(
                CalendarEventReadRequest {
                    user_id: "me".to_string(),
                    calendar_id: "primary".to_string(),
                    start_ms: 0,
                    end_ms: 1000,
                },
                2,
            )
            .unwrap();
        assert_eq!(events, vec![event]);
    }

    #[test]
    fn calendar_writes_require_approval_and_work_in_mock_mode() {
        let mut connector = CalendarConnector::new(auth(), ConnectorRuntimeMode::Mock).unwrap();
        connector.insert_mock(
            "create",
            serde_json::json!({
                "event_id": "evt-1",
                "calendar_id": "primary",
                "approval_id": "approval-1",
                "replay_key": "calendar:create:me:primary:100"
            }),
        );
        connector.insert_mock(
            "update",
            serde_json::json!({
                "event_id": "evt-1",
                "calendar_id": "primary",
                "approval_id": "approval-1",
                "replay_key": "calendar:update:me:primary:100"
            }),
        );
        connector.insert_mock(
            "cancel",
            serde_json::json!({
                "event_id": "evt-1",
                "calendar_id": "primary",
                "approval_id": "approval-1",
                "replay_key": "calendar:cancel:me:primary:evt-1"
            }),
        );

        let missing = connector
            .create_event(
                CalendarWriteRequest {
                    user_id: "me".to_string(),
                    calendar_id: "primary".to_string(),
                    event_id: None,
                    title: "Planning".to_string(),
                    start_ms: 100,
                    end_ms: 200,
                    attendees: vec!["external@example.com".to_string()],
                    approval_id: String::new(),
                },
                1,
            )
            .unwrap_err();
        assert!(
            matches!(missing, ConnectorRuntimeError::ApprovalRequired(scope) if scope == "calendar.create")
        );

        let create = connector
            .create_event(
                CalendarWriteRequest {
                    user_id: "me".to_string(),
                    calendar_id: "primary".to_string(),
                    event_id: None,
                    title: "Planning".to_string(),
                    start_ms: 100,
                    end_ms: 200,
                    attendees: vec!["external@example.com".to_string()],
                    approval_id: "approval-1".to_string(),
                },
                2,
            )
            .unwrap();
        assert_eq!(create.event_id, "evt-1");

        let update = connector
            .update_event(
                CalendarWriteRequest {
                    user_id: "me".to_string(),
                    calendar_id: "primary".to_string(),
                    event_id: Some("evt-1".to_string()),
                    title: "Planning updated".to_string(),
                    start_ms: 100,
                    end_ms: 300,
                    attendees: vec!["external@example.com".to_string()],
                    approval_id: "approval-1".to_string(),
                },
                3,
            )
            .unwrap();
        assert_eq!(update.event_id, "evt-1");

        let cancel = connector
            .cancel_event(
                CalendarCancelRequest {
                    user_id: "me".to_string(),
                    calendar_id: "primary".to_string(),
                    event_id: "evt-1".to_string(),
                    reason: "conflict".to_string(),
                    approval_id: "approval-1".to_string(),
                },
                4,
            )
            .unwrap();
        assert_eq!(cancel.event_id, "evt-1");
    }

    #[test]
    fn calendar_replay_quarantines_writes() {
        let mut connector = CalendarConnector::new(auth(), ConnectorRuntimeMode::Replay).unwrap();
        let err = connector
            .create_event(
                CalendarWriteRequest {
                    user_id: "me".to_string(),
                    calendar_id: "primary".to_string(),
                    event_id: None,
                    title: "Planning".to_string(),
                    start_ms: 100,
                    end_ms: 200,
                    attendees: vec!["external@example.com".to_string()],
                    approval_id: "approval-1".to_string(),
                },
                1,
            )
            .unwrap_err();
        assert!(
            matches!(err, ConnectorRuntimeError::ReplayWriteQuarantined(operation) if operation == "create")
        );
    }
}
