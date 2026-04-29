//! OAuth2 refresh resolver — slice 41K-C foundation.
//!
//! `OAuth2RefreshResolver` implements `BearerTokenResolver` for the
//! Gmail and Slack real clients. It consults a host-supplied
//! `OAuth2TokenStore` (which proxies to the Phase 37-G encrypted
//! token store), checks the access token's expiry against the
//! current clock with a configurable skew margin, and refreshes
//! against the provider's token endpoint when the cached access
//! token is too close to expiring.
//!
//! The HTTP refresh call is abstracted behind `OAuth2RefreshHook`
//! so tests can supply canned new tokens without spinning up a
//! mock token endpoint. The default `ReqwestRefreshHook` posts
//! `grant_type=refresh_token&refresh_token=...&client_id=...&client_secret=...`
//! to the configured token endpoint URL.

use crate::real_client::{BearerTokenError, BearerTokenResolver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Cached pair of OAuth2 tokens for a single (tenant, actor).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuth2Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at_ms: u64,
}

/// Host-supplied store proxy. The production implementation is
/// backed by the Phase 37-G encrypted token store; tests use the
/// `InMemoryOAuth2Store` shipped below.
pub trait OAuth2TokenStore: Send + Sync {
    fn load(&self, token_id: &str) -> Result<OAuth2Tokens, BearerTokenError>;
    fn save(&self, token_id: &str, tokens: OAuth2Tokens) -> Result<(), BearerTokenError>;
    fn mark_revoked(&self, token_id: &str) -> Result<(), BearerTokenError>;
}

/// Pluggable refresh callback. The production impl is
/// `ReqwestRefreshHook`; tests supply `StubRefreshHook` with canned
/// outcomes so the test does not need a network or a mock server.
pub trait OAuth2RefreshHook: Send + Sync {
    fn refresh(&self, refresh_token: &str) -> Result<OAuth2Tokens, BearerTokenError>;
}

/// In-memory `OAuth2TokenStore` for tests. Production uses the
/// Phase 37-G encrypted token store via a thin adapter (lands
/// alongside slice 41L when the CLI surface plumbs it through).
pub struct InMemoryOAuth2Store {
    inner: Mutex<std::collections::BTreeMap<String, OAuth2Entry>>,
}

#[derive(Debug, Clone)]
struct OAuth2Entry {
    tokens: OAuth2Tokens,
    revoked: bool,
}

impl InMemoryOAuth2Store {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(std::collections::BTreeMap::new()),
        }
    }

    pub fn seed(&self, token_id: impl Into<String>, tokens: OAuth2Tokens) {
        self.inner.lock().unwrap().insert(
            token_id.into(),
            OAuth2Entry {
                tokens,
                revoked: false,
            },
        );
    }

    pub fn snapshot(&self, token_id: &str) -> Option<OAuth2Tokens> {
        self.inner.lock().unwrap().get(token_id).map(|e| e.tokens.clone())
    }

    pub fn is_revoked(&self, token_id: &str) -> bool {
        self.inner
            .lock()
            .unwrap()
            .get(token_id)
            .map(|e| e.revoked)
            .unwrap_or(false)
    }
}

impl Default for InMemoryOAuth2Store {
    fn default() -> Self {
        Self::new()
    }
}

impl OAuth2TokenStore for InMemoryOAuth2Store {
    fn load(&self, token_id: &str) -> Result<OAuth2Tokens, BearerTokenError> {
        let guard = self.inner.lock().unwrap();
        let entry = guard
            .get(token_id)
            .ok_or_else(|| BearerTokenError::NotFound(token_id.to_string()))?;
        if entry.revoked {
            return Err(BearerTokenError::Revoked(token_id.to_string()));
        }
        Ok(entry.tokens.clone())
    }

    fn save(&self, token_id: &str, tokens: OAuth2Tokens) -> Result<(), BearerTokenError> {
        let mut guard = self.inner.lock().unwrap();
        let entry = guard
            .entry(token_id.to_string())
            .or_insert(OAuth2Entry {
                tokens: tokens.clone(),
                revoked: false,
            });
        entry.tokens = tokens;
        entry.revoked = false;
        Ok(())
    }

    fn mark_revoked(&self, token_id: &str) -> Result<(), BearerTokenError> {
        let mut guard = self.inner.lock().unwrap();
        if let Some(entry) = guard.get_mut(token_id) {
            entry.revoked = true;
        }
        Ok(())
    }
}

/// HTTP-backed refresh hook. POSTs the standard
/// `grant_type=refresh_token` form-encoded body to the configured
/// token endpoint and parses the response.
///
/// Provider-specific quirks (Slack's `oauth.v2.access` returns the
/// refresh response wrapped in `{ok, ...}`; Google returns plain
/// `{access_token, refresh_token?, expires_in}`) are handled by
/// each provider supplying its own token endpoint URL plus a
/// response shape. Default = the Google shape; Slack uses
/// `slack_refresh_hook` below.
pub struct ReqwestRefreshHook {
    client: reqwest::blocking::Client,
    token_endpoint: String,
    client_id: String,
    client_secret: String,
    now_ms: Box<dyn Fn() -> u64 + Send + Sync>,
    parser: TokenResponseParser,
}

#[derive(Clone, Copy)]
enum TokenResponseParser {
    Google,
    Slack,
}

impl ReqwestRefreshHook {
    /// Refresh hook for Google's OAuth2 token endpoint
    /// (`https://oauth2.googleapis.com/token`).
    pub fn google(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Result<Self, BearerTokenError> {
        Self::new(
            "https://oauth2.googleapis.com/token",
            client_id,
            client_secret,
            TokenResponseParser::Google,
        )
    }

    /// Refresh hook for Slack's OAuth2 token endpoint
    /// (`https://slack.com/api/oauth.v2.access`).
    pub fn slack(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Result<Self, BearerTokenError> {
        Self::new(
            "https://slack.com/api/oauth.v2.access",
            client_id,
            client_secret,
            TokenResponseParser::Slack,
        )
    }

    fn new(
        token_endpoint: impl Into<String>,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        parser: TokenResponseParser,
    ) -> Result<Self, BearerTokenError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("corvid-connector-runtime/41K-C")
            .build()
            .map_err(|e| BearerTokenError::Decryption(format!("reqwest init: {e}")))?;
        Ok(Self {
            client,
            token_endpoint: token_endpoint.into(),
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            now_ms: Box::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0)
            }),
            parser,
        })
    }
}

impl OAuth2RefreshHook for ReqwestRefreshHook {
    fn refresh(&self, refresh_token: &str) -> Result<OAuth2Tokens, BearerTokenError> {
        let now = (self.now_ms)();
        let form = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
        ];
        let resp = self
            .client
            .post(&self.token_endpoint)
            .form(&form)
            .send()
            .map_err(|e| BearerTokenError::Decryption(format!("refresh send: {e}")))?;

        let status = resp.status();
        let body: serde_json::Value = resp
            .json()
            .map_err(|e| BearerTokenError::Decryption(format!("refresh body: {e}")))?;
        match self.parser {
            TokenResponseParser::Google => parse_google_token_response(&body, refresh_token, now, status),
            TokenResponseParser::Slack => parse_slack_token_response(&body, refresh_token, now, status),
        }
    }
}

fn parse_google_token_response(
    body: &serde_json::Value,
    refresh_token_in: &str,
    now: u64,
    status: reqwest::StatusCode,
) -> Result<OAuth2Tokens, BearerTokenError> {
    if let Some(error) = body.get("error").and_then(|v| v.as_str()) {
        if error == "invalid_grant" {
            return Err(BearerTokenError::Revoked(refresh_token_in.to_string()));
        }
        return Err(BearerTokenError::Decryption(format!(
            "google refresh error `{error}`"
        )));
    }
    if !status.is_success() {
        return Err(BearerTokenError::Decryption(format!(
            "google refresh status {}",
            status.as_u16()
        )));
    }
    let access_token = body
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            BearerTokenError::Decryption("google response missing access_token".to_string())
        })?
        .to_string();
    let expires_in_s = body.get("expires_in").and_then(|v| v.as_u64()).unwrap_or(3600);
    let refresh_token = body
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| refresh_token_in.to_string());
    Ok(OAuth2Tokens {
        access_token,
        refresh_token,
        expires_at_ms: now.saturating_add(expires_in_s.saturating_mul(1_000)),
    })
}

fn parse_slack_token_response(
    body: &serde_json::Value,
    refresh_token_in: &str,
    now: u64,
    status: reqwest::StatusCode,
) -> Result<OAuth2Tokens, BearerTokenError> {
    let ok = body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    if !ok {
        let error = body.get("error").and_then(|v| v.as_str()).unwrap_or("unknown");
        if error == "invalid_grant" || error == "token_revoked" {
            return Err(BearerTokenError::Revoked(refresh_token_in.to_string()));
        }
        return Err(BearerTokenError::Decryption(format!(
            "slack refresh error `{error}`"
        )));
    }
    if !status.is_success() {
        return Err(BearerTokenError::Decryption(format!(
            "slack refresh status {}",
            status.as_u16()
        )));
    }
    // Slack v2 response uses `authed_user.access_token` for user-token refresh
    // and a top-level `access_token` for bot-token refresh. We accept both.
    let access_token = body
        .get("access_token")
        .and_then(|v| v.as_str())
        .or_else(|| {
            body.get("authed_user")
                .and_then(|u| u.get("access_token"))
                .and_then(|v| v.as_str())
        })
        .ok_or_else(|| {
            BearerTokenError::Decryption("slack response missing access_token".to_string())
        })?
        .to_string();
    let expires_in_s = body.get("expires_in").and_then(|v| v.as_u64()).unwrap_or(3600);
    let refresh_token = body
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| refresh_token_in.to_string());
    Ok(OAuth2Tokens {
        access_token,
        refresh_token,
        expires_at_ms: now.saturating_add(expires_in_s.saturating_mul(1_000)),
    })
}

/// Resolves a connector auth state's token reference to a fresh
/// access token, refreshing via the provider's OAuth2 token
/// endpoint when the cached value is too close to expiring.
pub struct OAuth2RefreshResolver {
    store: Arc<dyn OAuth2TokenStore>,
    refresh: Arc<dyn OAuth2RefreshHook>,
    skew_ms: u64,
    now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl OAuth2RefreshResolver {
    /// Construct a resolver. `skew_ms` is the safety margin (default
    /// 60_000 = 1 minute) so the resolver refreshes a token that
    /// would expire within the skew window rather than handing back
    /// a token a downstream call might find expired by the time it
    /// reaches the provider.
    pub fn new(
        store: Arc<dyn OAuth2TokenStore>,
        refresh: Arc<dyn OAuth2RefreshHook>,
    ) -> Self {
        Self {
            store,
            refresh,
            skew_ms: 60_000,
            now_ms: Arc::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0)
            }),
        }
    }

    pub fn with_skew_ms(mut self, skew_ms: u64) -> Self {
        self.skew_ms = skew_ms;
        self
    }

    pub fn with_clock(
        mut self,
        clock: impl Fn() -> u64 + Send + Sync + 'static,
    ) -> Self {
        self.now_ms = Arc::new(clock);
        self
    }
}

impl BearerTokenResolver for OAuth2RefreshResolver {
    fn resolve_bearer(&self, token_id: &str) -> Result<String, BearerTokenError> {
        let tokens = self.store.load(token_id)?;
        let now = (self.now_ms)();
        if tokens.expires_at_ms > now.saturating_add(self.skew_ms) {
            return Ok(tokens.access_token);
        }
        // Refresh.
        match self.refresh.refresh(&tokens.refresh_token) {
            Ok(new_tokens) => {
                self.store.save(token_id, new_tokens.clone())?;
                Ok(new_tokens.access_token)
            }
            Err(BearerTokenError::Revoked(_)) => {
                self.store.mark_revoked(token_id)?;
                Err(BearerTokenError::Revoked(token_id.to_string()))
            }
            Err(other) => Err(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test refresh hook: returns canned values without HTTP, lets
    /// the test assert which `refresh_token` value was supplied.
    struct StubRefreshHook {
        outcome: Result<OAuth2Tokens, BearerTokenError>,
        last_refresh_token: Mutex<Option<String>>,
    }

    impl StubRefreshHook {
        fn ok(tokens: OAuth2Tokens) -> Self {
            Self {
                outcome: Ok(tokens),
                last_refresh_token: Mutex::new(None),
            }
        }
        fn revoked(refresh_token: impl Into<String>) -> Self {
            Self {
                outcome: Err(BearerTokenError::Revoked(refresh_token.into())),
                last_refresh_token: Mutex::new(None),
            }
        }
    }

    impl OAuth2RefreshHook for StubRefreshHook {
        fn refresh(&self, refresh_token: &str) -> Result<OAuth2Tokens, BearerTokenError> {
            *self.last_refresh_token.lock().unwrap() = Some(refresh_token.to_string());
            self.outcome.clone()
        }
    }

    /// Slice 41K-C positive: a non-expired access token is returned
    /// directly without any refresh round-trip.
    #[test]
    fn returns_cached_access_when_not_expired() {
        let store = Arc::new(InMemoryOAuth2Store::new());
        store.seed(
            "tok-1",
            OAuth2Tokens {
                access_token: "ya29.fresh".to_string(),
                refresh_token: "rfr-1".to_string(),
                expires_at_ms: 10_000_000,
            },
        );
        let hook = Arc::new(StubRefreshHook::ok(OAuth2Tokens {
            access_token: "should-not-be-used".to_string(),
            refresh_token: "should-not-be-used".to_string(),
            expires_at_ms: 0,
        }));
        let resolver = OAuth2RefreshResolver::new(store, hook.clone())
            .with_clock(|| 5_000_000);
        let bearer = resolver.resolve_bearer("tok-1").expect("resolve");
        assert_eq!(bearer, "ya29.fresh");
        assert!(hook.last_refresh_token.lock().unwrap().is_none());
    }

    /// Slice 41K-C positive: when the access token is within the
    /// skew window, the resolver issues a refresh, persists the new
    /// pair, and returns the new access_token.
    #[test]
    fn refreshes_when_within_skew_window() {
        let store = Arc::new(InMemoryOAuth2Store::new());
        store.seed(
            "tok-1",
            OAuth2Tokens {
                access_token: "ya29.stale".to_string(),
                refresh_token: "rfr-1".to_string(),
                expires_at_ms: 1_000_030_000,
            },
        );
        let new_tokens = OAuth2Tokens {
            access_token: "ya29.fresh".to_string(),
            refresh_token: "rfr-2".to_string(),
            expires_at_ms: 1_000_000_000_000,
        };
        let hook = Arc::new(StubRefreshHook::ok(new_tokens.clone()));
        let resolver = OAuth2RefreshResolver::new(store.clone(), hook.clone())
            .with_skew_ms(60_000)
            .with_clock(|| 1_000_000_000);
        let bearer = resolver.resolve_bearer("tok-1").expect("resolve");
        assert_eq!(bearer, "ya29.fresh");
        assert_eq!(
            hook.last_refresh_token.lock().unwrap().clone().unwrap(),
            "rfr-1"
        );
        let saved = store.snapshot("tok-1").unwrap();
        assert_eq!(saved.access_token, "ya29.fresh");
        assert_eq!(saved.refresh_token, "rfr-2");
    }

    /// Slice 41K-C adversarial: a revoked refresh token surfaces as
    /// `BearerTokenError::Revoked` AND the store records the
    /// revocation so subsequent loads also fail.
    #[test]
    fn revoked_refresh_marks_store_revoked() {
        let store = Arc::new(InMemoryOAuth2Store::new());
        store.seed(
            "tok-1",
            OAuth2Tokens {
                access_token: "ya29.expired".to_string(),
                refresh_token: "rfr-bad".to_string(),
                expires_at_ms: 0,
            },
        );
        let hook = Arc::new(StubRefreshHook::revoked("rfr-bad"));
        let resolver = OAuth2RefreshResolver::new(store.clone(), hook)
            .with_clock(|| 1_000_000_000);
        let err = resolver.resolve_bearer("tok-1").unwrap_err();
        assert!(matches!(err, BearerTokenError::Revoked(_)));
        assert!(store.is_revoked("tok-1"));
        let again = resolver.resolve_bearer("tok-1").unwrap_err();
        assert!(matches!(again, BearerTokenError::Revoked(_)));
    }

    /// Slice 41K-C adversarial: an unknown token id surfaces
    /// `NotFound` from the store, no refresh attempted.
    #[test]
    fn unknown_token_id_returns_not_found() {
        let store = Arc::new(InMemoryOAuth2Store::new());
        let hook = Arc::new(StubRefreshHook::ok(OAuth2Tokens {
            access_token: "x".to_string(),
            refresh_token: "y".to_string(),
            expires_at_ms: 0,
        }));
        let resolver = OAuth2RefreshResolver::new(store, hook);
        let err = resolver.resolve_bearer("unknown").unwrap_err();
        assert!(matches!(err, BearerTokenError::NotFound(id) if id == "unknown"));
    }

    /// Slice 41K-C parser test: Google's `invalid_grant` error
    /// surfaces as `Revoked` so callers can mark the store revoked.
    #[test]
    fn parse_google_invalid_grant_is_revoked() {
        let body = serde_json::json!({"error": "invalid_grant"});
        let err = parse_google_token_response(
            &body,
            "rfr-bad",
            0,
            reqwest::StatusCode::BAD_REQUEST,
        )
        .unwrap_err();
        assert!(matches!(err, BearerTokenError::Revoked(_)));
    }

    /// Slice 41K-C parser test: Slack's `token_revoked` is also
    /// surfaced as `Revoked`.
    #[test]
    fn parse_slack_token_revoked_is_revoked() {
        let body = serde_json::json!({"ok": false, "error": "token_revoked"});
        let err = parse_slack_token_response(
            &body,
            "rfr-bad",
            0,
            reqwest::StatusCode::OK,
        )
        .unwrap_err();
        assert!(matches!(err, BearerTokenError::Revoked(_)));
    }

    /// Slice 41K-C parser test: a successful Google response yields
    /// new tokens with the expiry derived from the `expires_in`.
    #[test]
    fn parse_google_success_includes_expiry() {
        let body = serde_json::json!({
            "access_token": "ya29.new",
            "expires_in": 3600,
        });
        let tokens = parse_google_token_response(
            &body,
            "rfr-old",
            1_000_000,
            reqwest::StatusCode::OK,
        )
        .expect("parse");
        assert_eq!(tokens.access_token, "ya29.new");
        // Google didn't rotate refresh_token, so the previous one
        // sticks (real Google behaviour: refresh tokens persist).
        assert_eq!(tokens.refresh_token, "rfr-old");
        assert_eq!(tokens.expires_at_ms, 1_000_000 + 3_600_000);
    }

    /// Slice 41K-C parser test: Slack's nested `authed_user.access_token`
    /// is preferred over a missing top-level `access_token`.
    #[test]
    fn parse_slack_success_falls_back_to_authed_user() {
        let body = serde_json::json!({
            "ok": true,
            "authed_user": {"access_token": "xoxp-user-token"},
            "expires_in": 600,
        });
        let tokens = parse_slack_token_response(
            &body,
            "rfr-old",
            1_000_000,
            reqwest::StatusCode::OK,
        )
        .expect("parse");
        assert_eq!(tokens.access_token, "xoxp-user-token");
        assert_eq!(tokens.expires_at_ms, 1_000_000 + 600_000);
    }
}
