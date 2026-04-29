use crate::auth::{ConnectorAuthError, ConnectorAuthState};
use crate::manifest::{ConnectorManifest, ConnectorScope, ConnectorScopeApproval};
use crate::rate_limit::{ConnectorRateLimit, ConnectorRateLimiter};
use crate::trace::ConnectorTraceEvent;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorRuntimeMode {
    Mock,
    Replay,
    Real,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConnectorRequest {
    pub scope_id: String,
    pub operation: String,
    pub payload: Value,
    pub approval_id: String,
    pub replay_key: String,
    pub now_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConnectorResponse {
    pub payload: Value,
    pub trace: ConnectorTraceEvent,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectorRuntimeError {
    UnknownScope(String),
    Auth(ConnectorAuthError),
    RateLimited { retry_after_ms: u64 },
    MissingMock(String),
    ApprovalRequired(String),
    ReplayWriteQuarantined(String),
    RealModeNotBound(String),
}

impl std::fmt::Display for ConnectorRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownScope(scope) => write!(f, "unknown connector scope `{scope}`"),
            Self::Auth(err) => write!(f, "{err}"),
            Self::RateLimited { retry_after_ms } => {
                write!(f, "connector rate limited; retry after {retry_after_ms}ms")
            }
            Self::MissingMock(operation) => write!(f, "missing connector mock for `{operation}`"),
            Self::ApprovalRequired(scope) => {
                write!(f, "connector scope `{scope}` requires approval")
            }
            Self::ReplayWriteQuarantined(operation) => {
                write!(f, "replay mode quarantined write operation `{operation}`")
            }
            Self::RealModeNotBound(operation) => {
                write!(
                    f,
                    "real connector operation `{operation}` has no bound provider client"
                )
            }
        }
    }
}

impl std::error::Error for ConnectorRuntimeError {}

impl From<ConnectorAuthError> for ConnectorRuntimeError {
    fn from(value: ConnectorAuthError) -> Self {
        Self::Auth(value)
    }
}

#[derive(Debug, Clone)]
pub struct ConnectorRuntime {
    manifest: ConnectorManifest,
    auth: ConnectorAuthState,
    mode: ConnectorRuntimeMode,
    rate_limiter: ConnectorRateLimiter,
    mocks: BTreeMap<String, Value>,
}

impl ConnectorRuntime {
    pub fn new(
        manifest: ConnectorManifest,
        auth: ConnectorAuthState,
        mode: ConnectorRuntimeMode,
    ) -> Self {
        Self {
            manifest,
            auth,
            mode,
            rate_limiter: ConnectorRateLimiter::new(),
            mocks: BTreeMap::new(),
        }
    }

    pub fn insert_mock(&mut self, operation: impl Into<String>, payload: Value) {
        self.mocks.insert(operation.into(), payload);
    }

    pub fn execute(
        &mut self,
        request: ConnectorRequest,
    ) -> Result<ConnectorResponse, ConnectorRuntimeError> {
        let scope = self
            .manifest
            .scope
            .iter()
            .find(|scope| scope.id == request.scope_id)
            .ok_or_else(|| ConnectorRuntimeError::UnknownScope(request.scope_id.clone()))?
            .clone();
        self.auth.authorize(&scope.id, request.now_ms)?;
        if scope.approval == ConnectorScopeApproval::Required
            && request.approval_id.trim().is_empty()
        {
            return Err(ConnectorRuntimeError::ApprovalRequired(scope.id));
        }
        let decision = self.rate_limiter.check(
            &ConnectorRateLimit {
                key: format!("{}:{}", self.auth.tenant_id, self.auth.actor_id),
                limit: self
                    .manifest
                    .rate_limit
                    .first()
                    .map(|limit| limit.limit)
                    .unwrap_or(u64::MAX),
                window_ms: self
                    .manifest
                    .rate_limit
                    .first()
                    .map(|limit| limit.window_ms)
                    .unwrap_or(1),
            },
            request.now_ms,
        );
        if !decision.allowed {
            return Err(ConnectorRuntimeError::RateLimited {
                retry_after_ms: decision.retry_after_ms,
            });
        }

        let payload = match self.mode {
            ConnectorRuntimeMode::Mock => self
                .mocks
                .get(&request.operation)
                .cloned()
                .ok_or_else(|| ConnectorRuntimeError::MissingMock(request.operation.clone()))?,
            ConnectorRuntimeMode::Replay if scope_is_write(&scope) => {
                return Err(ConnectorRuntimeError::ReplayWriteQuarantined(
                    request.operation,
                ));
            }
            ConnectorRuntimeMode::Replay => self
                .mocks
                .get(&request.operation)
                .cloned()
                .ok_or_else(|| ConnectorRuntimeError::MissingMock(request.operation.clone()))?,
            ConnectorRuntimeMode::Real => {
                return Err(ConnectorRuntimeError::RealModeNotBound(request.operation));
            }
        };

        let trace = ConnectorTraceEvent {
            connector: self.manifest.name.clone(),
            operation: request.operation,
            tenant_id: self.auth.tenant_id.clone(),
            actor_id: self.auth.actor_id.clone(),
            mode: mode_name(self.mode).to_string(),
            status: "ok".to_string(),
            scope: scope.id,
            effect_ids: scope.effects,
            data_classes: scope.data_classes,
            approval_id: request.approval_id,
            replay_key: request.replay_key,
            latency_ms: 0,
            redacted: true,
        };
        Ok(ConnectorResponse { payload, trace })
    }
}

fn scope_is_write(scope: &ConnectorScope) -> bool {
    scope
        .effects
        .iter()
        .any(|effect| effect.contains(".write") || effect.starts_with("send_"))
}

fn mode_name(mode: ConnectorRuntimeMode) -> &'static str {
    match mode {
        ConnectorRuntimeMode::Mock => "mock",
        ConnectorRuntimeMode::Replay => "replay",
        ConnectorRuntimeMode::Real => "real",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{parse_connector_manifest, validate_connector_manifest};
    use serde_json::json;

    fn manifest() -> ConnectorManifest {
        let raw = r#"
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
limit = 1
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
"#;
        let manifest = parse_connector_manifest(raw).unwrap();
        assert!(validate_connector_manifest(&manifest).valid);
        manifest
    }

    fn auth() -> ConnectorAuthState {
        ConnectorAuthState::new(
            "tenant-1",
            "actor-1",
            "token-1",
            ["gmail.read_metadata", "gmail.send"],
            10_000,
        )
    }

    #[test]
    fn mock_mode_checks_auth_rate_limit_and_emits_trace() {
        let mut runtime = ConnectorRuntime::new(manifest(), auth(), ConnectorRuntimeMode::Mock);
        runtime.insert_mock("read_metadata", json!({"messages": [{"id": "m1"}]}));
        let response = runtime
            .execute(ConnectorRequest {
                scope_id: "gmail.read_metadata".to_string(),
                operation: "read_metadata".to_string(),
                payload: json!({}),
                approval_id: String::new(),
                replay_key: "replay-1".to_string(),
                now_ms: 1,
            })
            .unwrap();
        assert_eq!(response.payload["messages"][0]["id"], "m1");
        assert_eq!(response.trace.connector, "gmail");
        assert_eq!(response.trace.mode, "mock");
        assert_eq!(response.trace.effect_ids, vec!["network.read"]);

        let err = runtime
            .execute(ConnectorRequest {
                scope_id: "gmail.read_metadata".to_string(),
                operation: "read_metadata".to_string(),
                payload: json!({}),
                approval_id: String::new(),
                replay_key: "replay-2".to_string(),
                now_ms: 2,
            })
            .unwrap_err();
        assert!(matches!(err, ConnectorRuntimeError::RateLimited { .. }));
    }

    #[test]
    fn replay_mode_quarantines_writes() {
        let mut runtime = ConnectorRuntime::new(manifest(), auth(), ConnectorRuntimeMode::Replay);
        let err = runtime
            .execute(ConnectorRequest {
                scope_id: "gmail.send".to_string(),
                operation: "send".to_string(),
                payload: json!({"to": "a@example.com"}),
                approval_id: "approval-1".to_string(),
                replay_key: "replay-send".to_string(),
                now_ms: 1,
            })
            .unwrap_err();
        assert!(matches!(
            err,
            ConnectorRuntimeError::ReplayWriteQuarantined(operation) if operation == "send"
        ));
    }
}
