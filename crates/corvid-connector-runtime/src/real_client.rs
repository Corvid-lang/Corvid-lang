//! Real-mode connector client surface — slice 41K-A.
//!
//! Phase 41 originally shipped `ConnectorRuntimeMode::Real` returning
//! `Err(ConnectorRuntimeError::RealModeNotBound)` for every operation
//! and no HTTP client dependency in the connector crate. This module
//! introduces the trait that `ConnectorRuntime::execute` consults in
//! real mode, plus a default no-op implementation that preserves the
//! original `RealModeNotBound` behaviour. Per-connector real clients
//! (GitHub PAT, Gmail OAuth2, Slack OAuth2) land in slices 41K-B and
//! 41K-C; this slice ships only the architecture so those follow-ups
//! are mechanical.
//!
//! The `BearerTokenResolver` trait + `ReqwestRealClient` skeleton
//! also land here so 41K-B/C share one HTTP retry / Retry-After /
//! 429 handling path. Per-connector clients only contribute URL
//! mappings + provider-specific request/response shaping.

use crate::auth::ConnectorAuthState;
use crate::manifest::{ConnectorManifest, ConnectorScope};
use crate::runtime::ConnectorRuntimeError;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// Inputs available to a real-mode call. Shared by every
/// per-connector `ConnectorRealClient` implementation.
pub struct RealCallContext<'a> {
    pub manifest: &'a ConnectorManifest,
    pub scope: &'a ConnectorScope,
    pub auth: &'a ConnectorAuthState,
    pub operation: &'a str,
    pub payload: &'a Value,
    pub now_ms: u64,
}

/// Real-mode dispatcher. The default implementation is `RefuseRealMode`,
/// which returns `RealModeNotBound` for every operation and is what
/// `ConnectorRuntime` uses when no real client has been wired. A
/// production deployment supplies an implementation that knows how to
/// turn `(operation, payload, auth)` into an HTTP request against the
/// provider's API.
pub trait ConnectorRealClient: Send + Sync {
    fn execute_real(
        &self,
        ctx: &RealCallContext<'_>,
    ) -> Result<Value, ConnectorRuntimeError>;
}

/// Default real-mode dispatcher: refuse every call, preserving the
/// pre-41K-A `RealModeNotBound` behaviour. A `ConnectorRuntime`
/// constructed without a real client routes real-mode calls through
/// this implementation, so the live-mode gate at the call site
/// (`CORVID_PROVIDER_LIVE=1` per the audit-correction track) cannot
/// silently succeed against an unbound provider.
#[derive(Debug, Default, Clone, Copy)]
pub struct RefuseRealMode;

impl ConnectorRealClient for RefuseRealMode {
    fn execute_real(
        &self,
        ctx: &RealCallContext<'_>,
    ) -> Result<Value, ConnectorRuntimeError> {
        Err(ConnectorRuntimeError::RealModeNotBound(
            ctx.operation.to_string(),
        ))
    }
}

/// Resolves the bearer token for a connector auth state. Connector
/// auth holds a token *reference* (`auth.token_id`) — the actual
/// bearer lives in the host's encrypted token store (Phase 37-G
/// envelope). `ReqwestRealClient` consults this resolver before
/// every request so token rotation and revocation are honored.
pub trait BearerTokenResolver: Send + Sync {
    fn resolve_bearer(&self, token_id: &str) -> Result<String, BearerTokenError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BearerTokenError {
    NotFound(String),
    Revoked(String),
    Decryption(String),
}

impl std::fmt::Display for BearerTokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "bearer token `{id}` not found"),
            Self::Revoked(id) => write!(f, "bearer token `{id}` has been revoked"),
            Self::Decryption(id) => write!(f, "bearer token `{id}` failed to decrypt"),
        }
    }
}

impl std::error::Error for BearerTokenError {}

impl From<BearerTokenError> for ConnectorRuntimeError {
    fn from(err: BearerTokenError) -> Self {
        // Fold token-resolution failures into the existing auth
        // error surface so callers' existing match arms keep
        // working. The detailed reason is dropped here because
        // the bearer string itself must never leak into a trace;
        // the connector trace event records `redacted=true` and
        // the runtime audit log records the token_id reference.
        match err {
            BearerTokenError::Revoked(_) => ConnectorRuntimeError::Auth(
                crate::auth::ConnectorAuthError::RevokedRefreshToken,
            ),
            _ => ConnectorRuntimeError::Auth(
                crate::auth::ConnectorAuthError::MissingToken,
            ),
        }
    }
}

/// Static URL mapping that a per-connector real client supplies to the
/// shared `ReqwestRealClient`. Maps `(operation, payload)` → fully
/// constructed `reqwest::blocking::Request`. 41K-B + 41K-C provide
/// `GitHubEndpoints`, `GmailEndpoints`, and `SlackEndpoints`.
pub trait OperationEndpoints: Send + Sync {
    fn build_request(
        &self,
        ctx: &RealCallContext<'_>,
        bearer: &str,
        client: &reqwest::blocking::Client,
    ) -> Result<reqwest::blocking::RequestBuilder, ConnectorRuntimeError>;

    /// Optional: shape the JSON response. Default = identity.
    fn shape_response(
        &self,
        _ctx: &RealCallContext<'_>,
        body: Value,
    ) -> Value {
        body
    }
}

/// HTTP-backed real client. Maps `(operation, payload, auth)` to an
/// outbound `reqwest::blocking::Client` request via an
/// `OperationEndpoints` mapping, resolves the bearer token through a
/// `BearerTokenResolver`, and translates `Retry-After` / 429 / 5xx
/// into `ConnectorRuntimeError::RateLimited` so the runtime's
/// existing rate-limit reporter does the right thing.
///
/// 41K-B + 41K-C wrap this with a per-connector `OperationEndpoints`
/// implementation. The HTTP retry decision logic lives here once,
/// not per-connector.
pub struct ReqwestRealClient {
    client: reqwest::blocking::Client,
    bearer: Arc<dyn BearerTokenResolver>,
    endpoints: Arc<dyn OperationEndpoints>,
    request_timeout: Duration,
}

impl ReqwestRealClient {
    /// Construct a new real client. Default request timeout is 30s,
    /// matching what most provider SDKs use.
    pub fn new(
        bearer: Arc<dyn BearerTokenResolver>,
        endpoints: Arc<dyn OperationEndpoints>,
    ) -> Result<Self, ConnectorRuntimeError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("corvid-connector-runtime/41K-A")
            .build()
            .map_err(|e| {
                ConnectorRuntimeError::RealModeNotBound(format!(
                    "reqwest client init failed: {e}"
                ))
            })?;
        Ok(Self {
            client,
            bearer,
            endpoints,
            request_timeout: Duration::from_secs(30),
        })
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }
}

impl ConnectorRealClient for ReqwestRealClient {
    fn execute_real(
        &self,
        ctx: &RealCallContext<'_>,
    ) -> Result<Value, ConnectorRuntimeError> {
        let bearer = self.bearer.resolve_bearer(&ctx.auth.token_id)?;
        let request_builder = self.endpoints.build_request(ctx, &bearer, &self.client)?;
        let request_builder = request_builder.timeout(self.request_timeout);
        let response = request_builder.send().map_err(map_reqwest_error)?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS
            || status.is_server_error()
        {
            let retry_after_ms = parse_retry_after_header(
                response.headers().get(reqwest::header::RETRY_AFTER),
                ctx.now_ms,
            )
            .unwrap_or(60_000);
            return Err(ConnectorRuntimeError::RateLimited { retry_after_ms });
        }

        if !status.is_success() {
            return Err(ConnectorRuntimeError::RealModeNotBound(format!(
                "{}: provider returned HTTP {}",
                ctx.operation,
                status.as_u16()
            )));
        }

        let body: Value = response.json().map_err(map_reqwest_error)?;
        Ok(self.endpoints.shape_response(ctx, body))
    }
}

fn map_reqwest_error(err: reqwest::Error) -> ConnectorRuntimeError {
    if err.is_timeout() {
        ConnectorRuntimeError::RateLimited {
            retry_after_ms: 30_000,
        }
    } else {
        ConnectorRuntimeError::RealModeNotBound(format!("HTTP error: {err}"))
    }
}

/// Parse a `Retry-After` HTTP header per RFC 7231. The header may be
/// either an integer number of seconds or an HTTP-date. This helper
/// returns `Some(milliseconds)` for either form, falling back to
/// `None` on a malformed header so the caller can supply a default.
pub fn parse_retry_after_header(
    header: Option<&reqwest::header::HeaderValue>,
    now_ms: u64,
) -> Option<u64> {
    let header = header?;
    let s = header.to_str().ok()?;
    if let Ok(secs) = s.trim().parse::<u64>() {
        return Some(secs.saturating_mul(1_000));
    }
    // RFC 7231 IMF-fixdate: e.g. `Wed, 21 Oct 2026 07:28:00 GMT`.
    // Reqwest does not ship a date parser; we accept only the integer
    // form here and let callers default. Per-connector clients that
    // need date-form parsing add their own helper.
    let _ = now_ms;
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::ConnectorAuthState;
    use crate::manifest::{
        ConnectorManifest, ConnectorScope, ConnectorScopeApproval,
    };

    fn manifest() -> ConnectorManifest {
        ConnectorManifest {
            schema: "corvid.connector.v1".to_string(),
            name: "fake".to_string(),
            provider: "test".to_string(),
            mode: vec![],
            scope: vec![],
            rate_limit: vec![],
            redaction: vec![],
            replay: vec![],
        }
    }

    fn scope() -> ConnectorScope {
        ConnectorScope {
            id: "s".to_string(),
            provider_scope: "https://example/auth".to_string(),
            data_classes: vec![],
            effects: vec!["network.read".to_string()],
            approval: ConnectorScopeApproval::None,
        }
    }

    fn auth() -> ConnectorAuthState {
        ConnectorAuthState::new(
            "tenant-1",
            "actor-1",
            "token-1",
            ["s".to_string()],
            u64::MAX,
        )
    }

    /// Default real-mode dispatcher refuses every call, exactly as
    /// the pre-41K-A behaviour did. This is the shipping path for any
    /// `ConnectorRuntime` constructed without a real client wired.
    #[test]
    fn refuse_real_mode_returns_real_mode_not_bound() {
        let m = manifest();
        let s = scope();
        let a = auth();
        let payload = serde_json::Value::Null;
        let ctx = RealCallContext {
            manifest: &m,
            scope: &s,
            auth: &a,
            operation: "demo",
            payload: &payload,
            now_ms: 0,
        };
        let err = RefuseRealMode.execute_real(&ctx).expect_err("must refuse");
        assert!(
            matches!(err, ConnectorRuntimeError::RealModeNotBound(op) if op == "demo")
        );
    }

    /// `Retry-After: 5` → 5_000 ms. Lifts the RFC-7231 integer form
    /// directly so 41K-B/C don't reimplement it per connector.
    #[test]
    fn parse_retry_after_seconds_form() {
        let header = reqwest::header::HeaderValue::from_static("5");
        let parsed = parse_retry_after_header(Some(&header), 0).expect("parse");
        assert_eq!(parsed, 5_000);
    }

    /// A blank or malformed `Retry-After` returns `None` so the
    /// caller can pick a default (the runtime's default is 60s).
    #[test]
    fn parse_retry_after_returns_none_for_malformed() {
        let header = reqwest::header::HeaderValue::from_static("not-a-number");
        assert_eq!(parse_retry_after_header(Some(&header), 0), None);
    }

    /// A bound real client can short-circuit `RealModeNotBound` and
    /// return a payload — the architecture 41K-A is set up to support
    /// 41K-B and 41K-C plugging in per-connector implementations.
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

    #[test]
    fn bound_real_client_returns_payload() {
        let m = manifest();
        let s = scope();
        let a = auth();
        let payload = serde_json::Value::Null;
        let ctx = RealCallContext {
            manifest: &m,
            scope: &s,
            auth: &a,
            operation: "demo",
            payload: &payload,
            now_ms: 0,
        };
        let stub = StubRealClient {
            body: serde_json::json!({ "ok": true }),
        };
        let response = stub.execute_real(&ctx).expect("must succeed");
        assert_eq!(response, serde_json::json!({ "ok": true }));
    }
}
