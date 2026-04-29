use crate::{
    parse_connector_manifest, validate_connector_manifest, ConnectorAuthState, ConnectorManifest,
    ConnectorRequest, ConnectorRuntime, ConnectorRuntimeError, ConnectorRuntimeMode,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectorFixture {
    pub manifest: String,
    pub tenant_id: String,
    pub actor_id: String,
    pub token_id: String,
    pub scopes: Vec<String>,
    pub expires_at_ms: u64,
    pub scope_id: String,
    pub operation: String,
    #[serde(default)]
    pub request: Value,
    #[serde(default)]
    pub response: Value,
    pub replay_key: String,
    pub now_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConnectorFixtureReport {
    pub connector: String,
    pub operation: String,
    pub mock_ok: bool,
    pub replay_ok: bool,
    pub replay_write_quarantined: bool,
}

pub fn parse_connector_fixture(source: &str) -> Result<ConnectorFixture, serde_json::Error> {
    serde_json::from_str(source)
}

pub fn run_connector_fixture(
    fixture: &ConnectorFixture,
) -> Result<ConnectorFixtureReport, ConnectorRuntimeError> {
    let manifest = parse_connector_manifest(&fixture.manifest)
        .map_err(|err| ConnectorRuntimeError::MissingMock(err.to_string()))?;
    let validation = validate_connector_manifest(&manifest);
    if !validation.valid {
        return Err(ConnectorRuntimeError::MissingMock(format!(
            "invalid manifest: {:?}",
            validation.diagnostics
        )));
    }
    let mock_ok = run_mode(fixture, &manifest, ConnectorRuntimeMode::Mock)?.is_some();
    let replay = run_mode(fixture, &manifest, ConnectorRuntimeMode::Replay);
    let (replay_ok, replay_write_quarantined) = match replay {
        Ok(Some(_)) => (true, false),
        Ok(None) => (false, false),
        Err(ConnectorRuntimeError::ReplayWriteQuarantined(_)) => (false, true),
        Err(err) => return Err(err),
    };

    Ok(ConnectorFixtureReport {
        connector: manifest.name,
        operation: fixture.operation.clone(),
        mock_ok,
        replay_ok,
        replay_write_quarantined,
    })
}

fn run_mode(
    fixture: &ConnectorFixture,
    manifest: &ConnectorManifest,
    mode: ConnectorRuntimeMode,
) -> Result<Option<Value>, ConnectorRuntimeError> {
    let mut runtime = ConnectorRuntime::new(manifest.clone(), fixture_auth(fixture), mode);
    runtime.insert_mock(&fixture.operation, fixture.response.clone());
    let response = runtime.execute(ConnectorRequest {
        scope_id: fixture.scope_id.clone(),
        operation: fixture.operation.clone(),
        payload: fixture.request.clone(),
        replay_key: fixture.replay_key.clone(),
        now_ms: fixture.now_ms,
    })?;
    Ok(Some(response.payload))
}

fn fixture_auth(fixture: &ConnectorFixture) -> ConnectorAuthState {
    ConnectorAuthState::new(
        &fixture.tenant_id,
        &fixture.actor_id,
        &fixture.token_id,
        fixture.scopes.iter().map(String::as_str),
        fixture.expires_at_ms,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> &'static str {
        r#"
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
id = "gmail.send"
provider_scope = "https://www.googleapis.com/auth/gmail.send"
data_classes = ["email_body"]
effects = ["network.write", "send_email"]
approval = "required"

[[rate_limit]]
key = "actor"
limit = 10
window_ms = 100
retry_after = "provider_header"

[[redaction]]
field = "message.body"
strategy = "hash_and_drop"

[[replay]]
operation = "read_metadata"
policy = "record_read"

[[replay]]
operation = "send"
policy = "quarantine_write"
"#
    }

    #[test]
    fn fixture_runs_mock_and_replay_read_paths() {
        let fixture = ConnectorFixture {
            manifest: manifest().to_string(),
            tenant_id: "tenant-1".to_string(),
            actor_id: "actor-1".to_string(),
            token_id: "token-1".to_string(),
            scopes: vec!["gmail.read_metadata".to_string()],
            expires_at_ms: 10_000,
            scope_id: "gmail.read_metadata".to_string(),
            operation: "read_metadata".to_string(),
            request: serde_json::json!({"q": "newer_than:1d"}),
            response: serde_json::json!({"messages": [{"id": "m1"}]}),
            replay_key: "gmail:read:m1".to_string(),
            now_ms: 1,
        };

        let report = run_connector_fixture(&fixture).unwrap();
        assert_eq!(report.connector, "gmail");
        assert!(report.mock_ok);
        assert!(report.replay_ok);
        assert!(!report.replay_write_quarantined);
    }

    #[test]
    fn fixture_proves_replay_write_quarantine() {
        let fixture = ConnectorFixture {
            manifest: manifest().to_string(),
            tenant_id: "tenant-1".to_string(),
            actor_id: "actor-1".to_string(),
            token_id: "token-1".to_string(),
            scopes: vec!["gmail.send".to_string()],
            expires_at_ms: 10_000,
            scope_id: "gmail.send".to_string(),
            operation: "send".to_string(),
            request: serde_json::json!({"to": "a@example.com"}),
            response: serde_json::json!({"id": "sent-1"}),
            replay_key: "gmail:send:sent-1".to_string(),
            now_ms: 1,
        };

        let report = run_connector_fixture(&fixture).unwrap();
        assert!(report.mock_ok);
        assert!(!report.replay_ok);
        assert!(report.replay_write_quarantined);
    }
}
