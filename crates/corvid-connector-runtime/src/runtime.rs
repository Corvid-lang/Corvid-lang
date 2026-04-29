use crate::auth::{ConnectorAuthError, ConnectorAuthState};
use crate::manifest::{ConnectorManifest, ConnectorScope, ConnectorScopeApproval};
use crate::rate_limit::{ConnectorRateLimit, ConnectorRateLimiter};
use crate::real_client::{ConnectorRealClient, RealCallContext, RefuseRealMode};
use crate::trace::ConnectorTraceEvent;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

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

#[derive(Clone)]
pub struct ConnectorRuntime {
    manifest: ConnectorManifest,
    auth: ConnectorAuthState,
    mode: ConnectorRuntimeMode,
    rate_limiter: ConnectorRateLimiter,
    mocks: BTreeMap<String, Value>,
    /// Real-mode dispatcher. Defaults to `RefuseRealMode`, which
    /// returns `RealModeNotBound` for every operation — preserving
    /// the pre-41K-A behaviour. A production deployment installs a
    /// concrete client via `with_real_client(...)` (per slice 41K-B
    /// for GitHub PAT, slice 41K-C for Gmail/Slack OAuth2).
    real_client: Arc<dyn ConnectorRealClient>,
}

impl std::fmt::Debug for ConnectorRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectorRuntime")
            .field("manifest", &self.manifest)
            .field("auth", &self.auth)
            .field("mode", &self.mode)
            .field("rate_limiter", &self.rate_limiter)
            .field("mocks", &self.mocks)
            .field("real_client", &"<dyn ConnectorRealClient>")
            .finish()
    }
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
            real_client: Arc::new(RefuseRealMode),
        }
    }

    pub fn insert_mock(&mut self, operation: impl Into<String>, payload: Value) {
        self.mocks.insert(operation.into(), payload);
    }

    /// Install a real-mode dispatcher. Without this call, real-mode
    /// requests return `RealModeNotBound` — matching pre-41K-A
    /// behaviour and preventing accidental live calls during local
    /// development. Production deployments behind
    /// `CORVID_PROVIDER_LIVE=1` (per the audit-correction track)
    /// supply a concrete `ConnectorRealClient` here.
    pub fn with_real_client(mut self, client: Arc<dyn ConnectorRealClient>) -> Self {
        self.real_client = client;
        self
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
                // Slice 41K-A: dispatch through `ConnectorRealClient`.
                // When no real client has been installed, the default
                // `RefuseRealMode` returns `RealModeNotBound`,
                // preserving the pre-41K-A behaviour. A production
                // deployment installs a concrete real client behind
                // the `CORVID_PROVIDER_LIVE=1` env-var gate (slice
                // 41L); per-connector clients land in 41K-B/C.
                let ctx = RealCallContext {
                    manifest: &self.manifest,
                    scope: &scope,
                    auth: &self.auth,
                    operation: &request.operation,
                    payload: &request.payload,
                    now_ms: request.now_ms,
                };
                self.real_client.execute_real(&ctx)?
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

    /// Slice 41K-A: a `ConnectorRuntime` constructed without a real
    /// client preserves the pre-41K-A behaviour — real-mode requests
    /// return `RealModeNotBound`. This is what guards against
    /// accidental live calls during local development.
    #[test]
    fn real_mode_default_returns_real_mode_not_bound() {
        let mut runtime = ConnectorRuntime::new(manifest(), auth(), ConnectorRuntimeMode::Real);
        let err = runtime
            .execute(ConnectorRequest {
                scope_id: "gmail.read_metadata".to_string(),
                operation: "read_metadata".to_string(),
                payload: json!({}),
                approval_id: String::new(),
                replay_key: "replay-real".to_string(),
                now_ms: 1,
            })
            .unwrap_err();
        assert!(matches!(
            err,
            ConnectorRuntimeError::RealModeNotBound(operation) if operation == "read_metadata"
        ));
    }

    /// Slice 41K-A: when a concrete real client is wired via
    /// `with_real_client`, real-mode requests dispatch through it
    /// and return its payload. This is the architecture seam that
    /// 41K-B (GitHub PAT) and 41K-C (Gmail/Slack OAuth2) plug into.
    #[test]
    fn real_mode_dispatches_to_bound_client() {
        struct StubRealClient {
            body: Value,
        }
        impl ConnectorRealClient for StubRealClient {
            fn execute_real(
                &self,
                _ctx: &RealCallContext<'_>,
            ) -> Result<Value, ConnectorRuntimeError> {
                Ok(self.body.clone())
            }
        }

        let stub = Arc::new(StubRealClient {
            body: json!({"messages": [{"id": "live-1"}]}),
        });
        let mut runtime = ConnectorRuntime::new(manifest(), auth(), ConnectorRuntimeMode::Real)
            .with_real_client(stub);
        let response = runtime
            .execute(ConnectorRequest {
                scope_id: "gmail.read_metadata".to_string(),
                operation: "read_metadata".to_string(),
                payload: json!({}),
                approval_id: String::new(),
                replay_key: "replay-real-bound".to_string(),
                now_ms: 1,
            })
            .unwrap();
        assert_eq!(response.payload["messages"][0]["id"], "live-1");
        assert_eq!(response.trace.mode, "real");
    }

    /// Slice 41K-A: the runtime forwards the rate-limit decision
    /// produced by a bound real client (when the provider returns
    /// 429 + Retry-After). The shared `ReqwestRealClient` translates
    /// those into `RateLimited`; here we exercise the runtime path
    /// from the bound client end.
    #[test]
    fn real_mode_propagates_rate_limited_from_bound_client() {
        struct AlwaysRateLimited;
        impl ConnectorRealClient for AlwaysRateLimited {
            fn execute_real(
                &self,
                _ctx: &RealCallContext<'_>,
            ) -> Result<Value, ConnectorRuntimeError> {
                Err(ConnectorRuntimeError::RateLimited {
                    retry_after_ms: 7_000,
                })
            }
        }

        let mut runtime = ConnectorRuntime::new(manifest(), auth(), ConnectorRuntimeMode::Real)
            .with_real_client(Arc::new(AlwaysRateLimited));
        let err = runtime
            .execute(ConnectorRequest {
                scope_id: "gmail.read_metadata".to_string(),
                operation: "read_metadata".to_string(),
                payload: json!({}),
                approval_id: String::new(),
                replay_key: "replay-rate".to_string(),
                now_ms: 1,
            })
            .unwrap_err();
        assert!(matches!(
            err,
            ConnectorRuntimeError::RateLimited { retry_after_ms: 7_000 }
        ));
    }
}
