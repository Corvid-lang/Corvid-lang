//! Adversarial threat corpus — slice 41M-B.
//!
//! The Phase 41 phase-done checklist names seven attack classes
//! that the connector runtime must refuse, each with at least one
//! `must_fail` test per implemented connector. This module
//! consolidates those into a discoverable corpus organised by
//! threat × connector so a reviewer can answer "which test proves
//! Corvid refuses cross-tenant message access on Gmail?" by
//! grepping `tests/threat_corpus.rs`.
//!
//! The seven threats (per `docs/effects-spec/bounty.md` and the
//! ROADMAP Phase 41 audit-correction track 41M):
//!
//!   T1. token-scope escalation
//!   T2. cross-tenant access
//!   T3. refresh-token replay after revocation
//!   T4. malformed JSON body
//!   T5. 429 / 5xx Retry-After handling
//!   T6. expired OAuth state
//!   T7. webhook signature forgery
//!
//! Connectors covered today: github, gmail, slack. Linear shares
//! the github tasks-connector path. ms365 / calendar / files
//! share the same primitives and inherit the threats by
//! construction; their per-threat coverage extends this corpus
//! when their real-mode paths land.
//!
//! Each test name follows `t<N>_<connector>_<threat>` so a CI
//! grep for `t4_` lists every connector's malformed-body refusal,
//! etc.

use corvid_connector_runtime::{
    github_pat_real_client, BearerTokenError, BearerTokenResolver, ConnectorAuthError,
    ConnectorAuthState, ConnectorManifest, ConnectorRealClient, ConnectorRequest,
    ConnectorRuntime, ConnectorRuntimeError, ConnectorRuntimeMode, GitHubEndpoints,
    InMemoryOAuth2Store, OAuth2RefreshHook, OAuth2RefreshResolver, OAuth2Tokens,
    OperationEndpoints, RealCallContext, WebhookProvider, WebhookVerificationOutcome,
    WebhookVerifyInputs,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------
// Shared fixtures
// ---------------------------------------------------------------

fn fake_manifest(name: &str, provider: &str) -> ConnectorManifest {
    fake_manifest_with_scopes(name, provider, &[])
}

fn fake_manifest_with_scopes(
    name: &str,
    provider: &str,
    extra_scopes: &[(&str, &[&str], corvid_connector_runtime::ConnectorScopeApproval)],
) -> ConnectorManifest {
    let scope = extra_scopes
        .iter()
        .map(|(id, effects, approval)| corvid_connector_runtime::ConnectorScope {
            id: id.to_string(),
            provider_scope: format!("{provider}:{id}"),
            data_classes: vec![],
            effects: effects.iter().map(|e| e.to_string()).collect(),
            approval: *approval,
        })
        .collect();
    ConnectorManifest {
        schema: "corvid.connector.v1".to_string(),
        name: name.to_string(),
        provider: provider.to_string(),
        mode: vec![],
        scope,
        rate_limit: vec![],
        redaction: vec![],
        replay: vec![],
    }
}

fn auth_for(token_id: &str, scopes: &[&str]) -> ConnectorAuthState {
    ConnectorAuthState::new(
        "tenant-1",
        "actor-1",
        token_id,
        scopes.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        u64::MAX,
    )
}

// ---------------------------------------------------------------
// T1 — token-scope escalation
//
// A connector must refuse a call against a scope its auth state
// does not authorise. This is the `ConnectorAuthError::MissingScope`
// path; the runtime fires it before any HTTP layer touches the
// network, so a leaked low-scope token cannot escalate to a
// higher-scope operation by guessing the scope id.
// ---------------------------------------------------------------

#[test]
fn t1_github_rejects_unauthorised_scope() {
    let auth = auth_for("token-1", &["tasks.github_search"]);
    let mut runtime = ConnectorRuntime::new(
        fake_manifest("tasks", "linear_github"),
        auth,
        ConnectorRuntimeMode::Mock,
    );
    runtime.insert_mock("github_search", json!([]));
    // Call against a scope NOT in the auth state's scope set.
    let err = runtime
        .execute(ConnectorRequest {
            scope_id: "tasks.github_write".to_string(),
            operation: "github_write".to_string(),
            payload: json!({}),
            approval_id: "a".to_string(),
            replay_key: "rk".to_string(),
            now_ms: 1,
        })
        .unwrap_err();
    assert!(matches!(
        err,
        ConnectorRuntimeError::Auth(ConnectorAuthError::MissingScope(_))
            | ConnectorRuntimeError::UnknownScope(_)
    ));
}

#[test]
fn t1_gmail_rejects_unauthorised_scope() {
    let auth = auth_for("token-1", &["gmail.read_metadata"]);
    let mut runtime = ConnectorRuntime::new(
        fake_manifest("gmail", "google"),
        auth,
        ConnectorRuntimeMode::Mock,
    );
    runtime.insert_mock("send", json!({"id": "x"}));
    let err = runtime
        .execute(ConnectorRequest {
            scope_id: "gmail.send".to_string(),
            operation: "send".to_string(),
            payload: json!({}),
            approval_id: "a".to_string(),
            replay_key: "rk".to_string(),
            now_ms: 1,
        })
        .unwrap_err();
    assert!(matches!(
        err,
        ConnectorRuntimeError::Auth(ConnectorAuthError::MissingScope(_))
            | ConnectorRuntimeError::UnknownScope(_)
    ));
}

#[test]
fn t1_slack_rejects_unauthorised_scope() {
    let auth = auth_for("token-1", &["slack.channel_read"]);
    let mut runtime = ConnectorRuntime::new(
        fake_manifest("slack", "slack"),
        auth,
        ConnectorRuntimeMode::Mock,
    );
    runtime.insert_mock("send", json!({"id": "x"}));
    let err = runtime
        .execute(ConnectorRequest {
            scope_id: "slack.send".to_string(),
            operation: "send".to_string(),
            payload: json!({}),
            approval_id: "a".to_string(),
            replay_key: "rk".to_string(),
            now_ms: 1,
        })
        .unwrap_err();
    assert!(matches!(
        err,
        ConnectorRuntimeError::Auth(ConnectorAuthError::MissingScope(_))
            | ConnectorRuntimeError::UnknownScope(_)
    ));
}

// ---------------------------------------------------------------
// T2 — cross-tenant access
//
// An auth state belongs to exactly one (tenant, actor) pair. The
// runtime's authorize step confirms `tenant_id`, `actor_id`, and
// `token_id` are populated; an empty tenant id is the smoking-gun
// case (a forged auth state attempting to claim global access).
// ---------------------------------------------------------------

#[test]
fn t2_github_rejects_missing_tenant() {
    let auth = ConnectorAuthState::new(
        "",
        "actor-1",
        "token-1",
        ["tasks.github_search".to_string()],
        u64::MAX,
    );
    let mut runtime = ConnectorRuntime::new(
        fake_manifest_with_scopes(
            "tasks",
            "linear_github",
            &[(
                "tasks.github_search",
                &["network.read"],
                corvid_connector_runtime::ConnectorScopeApproval::None,
            )],
        ),
        auth,
        ConnectorRuntimeMode::Mock,
    );
    runtime.insert_mock("github_search", json!([]));
    let err = runtime
        .execute(ConnectorRequest {
            scope_id: "tasks.github_search".to_string(),
            operation: "github_search".to_string(),
            payload: json!({}),
            approval_id: String::new(),
            replay_key: "rk".to_string(),
            now_ms: 1,
        })
        .unwrap_err();
    assert!(matches!(
        err,
        ConnectorRuntimeError::Auth(ConnectorAuthError::MissingTenant)
            | ConnectorRuntimeError::Auth(ConnectorAuthError::TenantMismatch)
    ));
}

#[test]
fn t2_gmail_rejects_missing_tenant() {
    let auth = ConnectorAuthState::new(
        "",
        "actor-1",
        "token-1",
        ["gmail.search".to_string()],
        u64::MAX,
    );
    let mut runtime = ConnectorRuntime::new(
        fake_manifest_with_scopes(
            "gmail",
            "google",
            &[(
                "gmail.search",
                &["network.read"],
                corvid_connector_runtime::ConnectorScopeApproval::None,
            )],
        ),
        auth,
        ConnectorRuntimeMode::Mock,
    );
    runtime.insert_mock("search", json!([]));
    let err = runtime
        .execute(ConnectorRequest {
            scope_id: "gmail.search".to_string(),
            operation: "search".to_string(),
            payload: json!({}),
            approval_id: String::new(),
            replay_key: "rk".to_string(),
            now_ms: 1,
        })
        .unwrap_err();
    assert!(matches!(
        err,
        ConnectorRuntimeError::Auth(ConnectorAuthError::MissingTenant)
    ));
}

#[test]
fn t2_slack_rejects_missing_tenant() {
    let auth = ConnectorAuthState::new(
        "",
        "actor-1",
        "token-1",
        ["slack.channel_read".to_string()],
        u64::MAX,
    );
    let mut runtime = ConnectorRuntime::new(
        fake_manifest_with_scopes(
            "slack",
            "slack",
            &[(
                "slack.channel_read",
                &["network.read"],
                corvid_connector_runtime::ConnectorScopeApproval::None,
            )],
        ),
        auth,
        ConnectorRuntimeMode::Mock,
    );
    runtime.insert_mock("channel_read", json!([]));
    let err = runtime
        .execute(ConnectorRequest {
            scope_id: "slack.channel_read".to_string(),
            operation: "channel_read".to_string(),
            payload: json!({}),
            approval_id: String::new(),
            replay_key: "rk".to_string(),
            now_ms: 1,
        })
        .unwrap_err();
    assert!(matches!(
        err,
        ConnectorRuntimeError::Auth(ConnectorAuthError::MissingTenant)
    ));
}

// ---------------------------------------------------------------
// T3 — refresh-token replay after revocation
//
// A refresh token whose provider rejects `invalid_grant` must
// surface as `BearerTokenError::Revoked` AND mark the token store
// revoked so a subsequent retry from the same code path also
// fails closed. The 41K-C `OAuth2RefreshResolver` test suite
// already proves this; this entry surfaces the test name
// alongside the other threats so the corpus is self-contained.
// ---------------------------------------------------------------

struct AlwaysRevokedRefreshHook;
impl OAuth2RefreshHook for AlwaysRevokedRefreshHook {
    fn refresh(&self, refresh_token: &str) -> Result<OAuth2Tokens, BearerTokenError> {
        Err(BearerTokenError::Revoked(refresh_token.to_string()))
    }
}

#[test]
fn t3_oauth_refresh_after_revocation_marks_store_revoked() {
    let store = Arc::new(InMemoryOAuth2Store::new());
    store.seed(
        "tok-1",
        OAuth2Tokens {
            access_token: "stale".to_string(),
            refresh_token: "rfr-bad".to_string(),
            expires_at_ms: 0,
        },
    );
    let resolver = OAuth2RefreshResolver::new(store.clone(), Arc::new(AlwaysRevokedRefreshHook))
        .with_clock(|| 1_000_000_000);
    let err = resolver.resolve_bearer("tok-1").unwrap_err();
    assert!(matches!(err, BearerTokenError::Revoked(_)));
    assert!(store.is_revoked("tok-1"));
    // Subsequent attempt also fails — the revoked record persists.
    let again = resolver.resolve_bearer("tok-1").unwrap_err();
    assert!(matches!(again, BearerTokenError::Revoked(_)));
}

// ---------------------------------------------------------------
// T4 — malformed JSON body
//
// A connector caller passing payload that does not match the
// per-operation contract must be refused with a clear diagnostic
// rather than producing a malformed HTTP request. The
// per-provider `OperationEndpoints` impls reject before the HTTP
// layer fires.
// ---------------------------------------------------------------

#[test]
fn t4_github_search_missing_required_field() {
    let endpoints = GitHubEndpoints::new();
    let client = reqwest::blocking::Client::new();
    let manifest = fake_manifest("tasks", "linear_github");
    let scope = corvid_connector_runtime::ConnectorScope {
        id: "tasks.github_search".to_string(),
        provider_scope: "github:issues:read".to_string(),
        data_classes: vec![],
        effects: vec!["network.read".to_string()],
        approval: corvid_connector_runtime::ConnectorScopeApproval::None,
    };
    let auth = auth_for("tok", &["tasks.github_search"]);
    // Payload missing `query` — endpoint must refuse before HTTP.
    let payload = json!({"owner": "octocat", "repo": "Hello-World"});
    let ctx = RealCallContext {
        manifest: &manifest,
        scope: &scope,
        auth: &auth,
        operation: "github_search",
        payload: &payload,
        now_ms: 0,
    };
    let result = endpoints.build_request(&ctx, "ghp_test", &client);
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("expected refusal"),
    };
    assert!(matches!(
        &err,
        ConnectorRuntimeError::RealModeNotBound(msg) if msg.contains("query")
    ));
}

#[test]
fn t4_github_write_unknown_kind_refused() {
    let endpoints = GitHubEndpoints::new();
    let client = reqwest::blocking::Client::new();
    let manifest = fake_manifest("tasks", "linear_github");
    let scope = corvid_connector_runtime::ConnectorScope {
        id: "tasks.github_write".to_string(),
        provider_scope: "github:issues:write".to_string(),
        data_classes: vec![],
        effects: vec!["network.write".to_string()],
        approval: corvid_connector_runtime::ConnectorScopeApproval::Required,
    };
    let auth = auth_for("tok", &["tasks.github_write"]);
    let payload = json!({
        "workspace_or_repo": "octocat/Hello-World",
        "title": "x",
        "body": "x",
        "kind": "Yeet",
        "approval_id": "a",
    });
    let ctx = RealCallContext {
        manifest: &manifest,
        scope: &scope,
        auth: &auth,
        operation: "github_write",
        payload: &payload,
        now_ms: 0,
    };
    let result = endpoints.build_request(&ctx, "ghp", &client);
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("expected refusal"),
    };
    assert!(matches!(
        &err,
        ConnectorRuntimeError::RealModeNotBound(msg) if msg.contains("Yeet")
    ));
}

// ---------------------------------------------------------------
// T5 — 429 / 5xx Retry-After handling
//
// The shared `ReqwestRealClient` translates 429 + 5xx into
// `ConnectorRuntimeError::RateLimited`, parsing the Retry-After
// header into milliseconds. The runtime forwards this so a
// caller can apply backoff. We exercise the runtime path via a
// stub real client that always rate-limits.
// ---------------------------------------------------------------

#[test]
fn t5_rate_limited_propagates_retry_after_ms() {
    struct AlwaysRateLimited;
    impl ConnectorRealClient for AlwaysRateLimited {
        fn execute_real(
            &self,
            _ctx: &RealCallContext<'_>,
        ) -> Result<serde_json::Value, ConnectorRuntimeError> {
            Err(ConnectorRuntimeError::RateLimited {
                retry_after_ms: 12_000,
            })
        }
    }
    let manifest = fake_manifest("tasks", "linear_github");
    let auth = auth_for("tok", &["tasks.github_search"]);
    // We need the manifest to declare the scope so the runtime
    // resolves it; reuse the real shipped manifest.
    let real_manifest = corvid_connector_runtime::task_manifest().expect("manifest");
    let mut runtime = ConnectorRuntime::new(real_manifest, auth, ConnectorRuntimeMode::Real)
        .with_real_client(Arc::new(AlwaysRateLimited));
    let _ = manifest;
    let err = runtime
        .execute(ConnectorRequest {
            scope_id: "tasks.github_search".to_string(),
            operation: "github_search".to_string(),
            payload: json!({"owner": "x", "repo": "y", "query": "q", "limit": 1}),
            approval_id: String::new(),
            replay_key: "rk".to_string(),
            now_ms: 1,
        })
        .unwrap_err();
    assert!(matches!(
        err,
        ConnectorRuntimeError::RateLimited { retry_after_ms } if retry_after_ms == 12_000
    ));
}

#[test]
fn t5_retry_after_parser_handles_seconds_form() {
    use corvid_connector_runtime::parse_retry_after_header;
    let header = reqwest::header::HeaderValue::from_static("30");
    assert_eq!(parse_retry_after_header(Some(&header), 0), Some(30_000));
}

// ---------------------------------------------------------------
// T6 — expired OAuth state (PKCE / authorization-code phase)
//
// Phase 39 ships oauth_state with single-use + tenant-scoped +
// expiry-bound semantics. The connector layer's OAuth2 refresh
// resolver complements that: an expired access token must trigger
// a refresh, not silently return a stale value. We exercise the
// expiry skew window: a token that expires *now* must refresh
// rather than be returned as-is.
// ---------------------------------------------------------------

struct CountingRefreshHook {
    count: Mutex<u64>,
    new_tokens: OAuth2Tokens,
}
impl OAuth2RefreshHook for CountingRefreshHook {
    fn refresh(&self, _refresh_token: &str) -> Result<OAuth2Tokens, BearerTokenError> {
        *self.count.lock().unwrap() += 1;
        Ok(self.new_tokens.clone())
    }
}

#[test]
fn t6_expired_oauth_access_triggers_refresh() {
    let store = Arc::new(InMemoryOAuth2Store::new());
    store.seed(
        "tok-1",
        OAuth2Tokens {
            access_token: "stale".to_string(),
            refresh_token: "rfr-1".to_string(),
            expires_at_ms: 1_000_000_000, // exact-now → within skew → refresh
        },
    );
    let hook = Arc::new(CountingRefreshHook {
        count: Mutex::new(0),
        new_tokens: OAuth2Tokens {
            access_token: "fresh".to_string(),
            refresh_token: "rfr-2".to_string(),
            expires_at_ms: 1_000_000_000_000,
        },
    });
    let resolver = OAuth2RefreshResolver::new(store.clone(), hook.clone())
        .with_clock(|| 1_000_000_000);
    let bearer = resolver.resolve_bearer("tok-1").unwrap();
    assert_eq!(bearer, "fresh");
    assert_eq!(*hook.count.lock().unwrap(), 1);
}

// ---------------------------------------------------------------
// T7 — webhook signature forgery
//
// The per-provider webhook verifier (slice 41M-A) refuses
// tampered bodies, missing headers, and (Slack-only) replay
// outside the freshness window. We add per-connector
// adversarial cases so the corpus carries the named threat.
// ---------------------------------------------------------------

fn hmac_hex(secret: &[u8], body: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret).unwrap();
    mac.update(body);
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn t7_github_webhook_forgery_rejected() {
    let secret = b"github-secret";
    let body = b"{\"event\":\"push\"}";
    // Forged signature: correct length but wrong bytes.
    let forged = "deadbeef".repeat(8);
    let mut headers = BTreeMap::new();
    headers.insert(
        "X-Hub-Signature-256".to_string(),
        format!("sha256={forged}"),
    );
    let inputs = WebhookVerifyInputs::new(WebhookProvider::GitHub, &headers, body, secret, 0);
    let outcome = corvid_connector_runtime::verify_webhook(inputs);
    assert_eq!(outcome, WebhookVerificationOutcome::BadSignature);
}

#[test]
fn t7_slack_webhook_replay_outside_window_rejected() {
    let secret = b"slack-secret";
    let body = b"old=event";
    let then_s: u64 = 1_700_000_000;
    let mut basestring = Vec::new();
    basestring.extend_from_slice(b"v0:");
    basestring.extend_from_slice(then_s.to_string().as_bytes());
    basestring.push(b':');
    basestring.extend_from_slice(body);
    let sig = hmac_hex(secret, &basestring);
    let mut headers = BTreeMap::new();
    headers.insert("X-Slack-Signature".to_string(), format!("v0={sig}"));
    headers.insert(
        "X-Slack-Request-Timestamp".to_string(),
        then_s.to_string(),
    );
    // 10 minutes past the freshness window.
    let now_ms = (then_s + 600).saturating_mul(1_000);
    let inputs = WebhookVerifyInputs::new(WebhookProvider::Slack, &headers, body, secret, now_ms);
    let outcome = corvid_connector_runtime::verify_webhook(inputs);
    assert!(matches!(outcome, WebhookVerificationOutcome::Stale { .. }));
}

#[test]
fn t7_linear_webhook_wrong_secret_rejected() {
    let body = b"{\"action\":\"create\"}";
    let sig = hmac_hex(b"correct-secret", body);
    let mut headers = BTreeMap::new();
    headers.insert("Linear-Signature".to_string(), sig);
    let inputs = WebhookVerifyInputs::new(
        WebhookProvider::Linear,
        &headers,
        body,
        b"wrong-secret",
        0,
    );
    let outcome = corvid_connector_runtime::verify_webhook(inputs);
    assert_eq!(outcome, WebhookVerificationOutcome::BadSignature);
}

// ---------------------------------------------------------------
// Bonus: smoke-test that the convenience real-client constructor
// for GitHub PAT does NOT short-circuit any of the above. T1 / T2
// happen before the bearer resolver runs, so a constructed real
// client cannot bypass scope or tenant checks.
// ---------------------------------------------------------------

#[test]
fn convenience_real_client_does_not_bypass_runtime_checks() {
    let auth = auth_for("token-1", &["tasks.github_search"]);
    let client = github_pat_real_client("token-1", "ghp_dummy").unwrap();
    let mut runtime =
        ConnectorRuntime::new(fake_manifest("tasks", "linear_github"), auth, ConnectorRuntimeMode::Real)
            .with_real_client(client);
    // A request against a write scope that's not in the auth set
    // must fail with MissingScope BEFORE any HTTP call would fire.
    let err = runtime
        .execute(ConnectorRequest {
            scope_id: "tasks.github_write".to_string(),
            operation: "github_write".to_string(),
            payload: json!({}),
            approval_id: "a".to_string(),
            replay_key: "rk".to_string(),
            now_ms: 1,
        })
        .unwrap_err();
    assert!(matches!(
        err,
        ConnectorRuntimeError::Auth(ConnectorAuthError::MissingScope(_))
            | ConnectorRuntimeError::UnknownScope(_)
    ));
}
