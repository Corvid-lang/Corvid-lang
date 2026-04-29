//! GitHub real-mode client — slice 41K-B.
//!
//! Plugs into the `OperationEndpoints` trait shipped by 41K-A so the
//! shared `ReqwestRealClient` can handle the HTTP layer (retry,
//! Retry-After, 5xx mapping). This module contributes only:
//!
//!   - URL templates for `github_search` and `github_write` (Create /
//!     Update / Comment),
//!   - the GitHub-specific request shaping (`Authorization: Bearer
//!     <pat>`, `Accept: application/vnd.github+json`,
//!     `X-GitHub-Api-Version: 2022-11-28`),
//!   - a `shape_response` for `github_search` that extracts the
//!     `items[]` array GitHub's `/search/issues` returns so it
//!     matches the `Vec<TaskIssue>` shape `TaskConnector`'s caller
//!     deserialises.
//!
//! GitHub Personal Access Tokens are static — there is no OAuth
//! refresh path. Slice 41K-B uses the simplest possible bearer
//! resolver: `StaticBearerResolver` returns the bearer string passed
//! at construction. Slice 41K-C adds `OAuth2RefreshResolver` for
//! Gmail and Slack.
//!
//! Live-mode integration tests against api.github.com are gated
//! behind `CORVID_PROVIDER_LIVE=1` and a `GITHUB_PAT` env var. The
//! default CI matrix runs only the unit-level URL/request shaping
//! tests below; the live test runs in an opt-in CI matrix.

use crate::real_client::{
    BearerTokenError, BearerTokenResolver, ConnectorRealClient, OperationEndpoints,
    RealCallContext, RealCallPlan, ReqwestRealClient,
};
use crate::runtime::ConnectorRuntimeError;
use serde_json::Value;
use std::sync::Arc;

/// GitHub REST API base. Production is `https://api.github.com`;
/// tests can override via `GitHubEndpoints::with_base_url`.
pub const GITHUB_API_BASE: &str = "https://api.github.com";

/// `OperationEndpoints` impl that maps the connector's
/// `github_search` and `github_write` operations to GitHub REST
/// URLs.
#[derive(Debug, Clone)]
pub struct GitHubEndpoints {
    base_url: String,
}

impl Default for GitHubEndpoints {
    fn default() -> Self {
        Self {
            base_url: GITHUB_API_BASE.to_string(),
        }
    }
}

impl GitHubEndpoints {
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the base URL (for local mock servers in tests).
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    fn search_url(&self, owner: &str, repo: &str, query: &str, limit: u32) -> String {
        // GitHub's `/search/issues` accepts `q=repo:{owner}/{repo}+{query}`.
        // We URL-encode the query value so spaces and operators survive.
        let q = format!("repo:{owner}/{repo} {query}");
        let q_encoded = url_encode(&q);
        let limit = limit.clamp(1, 100);
        format!(
            "{}/search/issues?q={q_encoded}&per_page={limit}",
            self.base_url
        )
    }

    fn create_issue_url(&self, owner: &str, repo: &str) -> String {
        format!("{}/repos/{owner}/{repo}/issues", self.base_url)
    }

    fn update_issue_url(&self, owner: &str, repo: &str, issue: &str) -> String {
        format!("{}/repos/{owner}/{repo}/issues/{issue}", self.base_url)
    }

    fn issue_comment_url(&self, owner: &str, repo: &str, issue: &str) -> String {
        format!(
            "{}/repos/{owner}/{repo}/issues/{issue}/comments",
            self.base_url
        )
    }
}

impl OperationEndpoints for GitHubEndpoints {
    fn build_request(
        &self,
        ctx: &RealCallContext<'_>,
        bearer: &str,
        client: &reqwest::blocking::Client,
    ) -> Result<RealCallPlan, ConnectorRuntimeError> {
        match ctx.operation {
            "github_search" => {
                let owner = string_field(ctx.payload, "owner")?;
                let repo = string_field(ctx.payload, "repo")?;
                let query = string_field(ctx.payload, "query")?;
                let limit = ctx
                    .payload
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(20) as u32;
                let url = self.search_url(&owner, &repo, &query, limit);
                Ok(RealCallPlan::Http(github_request(client.get(&url), bearer)))
            }
            "github_write" => {
                let owner_repo = string_field(ctx.payload, "workspace_or_repo")?;
                let (owner, repo) = split_owner_repo(&owner_repo)?;
                let kind = string_field(ctx.payload, "kind")?;
                let title = string_field(ctx.payload, "title").unwrap_or_default();
                let body = string_field(ctx.payload, "body").unwrap_or_default();
                let issue_id = ctx
                    .payload
                    .get("issue_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                match kind.as_str() {
                    "Create" => {
                        let url = self.create_issue_url(&owner, &repo);
                        let body_json = serde_json::json!({"title": title, "body": body});
                        Ok(RealCallPlan::Http(github_request(
                            client.post(&url).json(&body_json),
                            bearer,
                        )))
                    }
                    "Update" => {
                        let issue = issue_id.ok_or_else(|| {
                            ConnectorRuntimeError::RealModeNotBound(
                                "github_write Update requires issue_id".to_string(),
                            )
                        })?;
                        let url = self.update_issue_url(&owner, &repo, &issue);
                        let body_json = serde_json::json!({"title": title, "body": body});
                        Ok(RealCallPlan::Http(github_request(
                            client.patch(&url).json(&body_json),
                            bearer,
                        )))
                    }
                    "Comment" => {
                        let issue = issue_id.ok_or_else(|| {
                            ConnectorRuntimeError::RealModeNotBound(
                                "github_write Comment requires issue_id".to_string(),
                            )
                        })?;
                        let url = self.issue_comment_url(&owner, &repo, &issue);
                        let body_json = serde_json::json!({"body": body});
                        Ok(RealCallPlan::Http(github_request(
                            client.post(&url).json(&body_json),
                            bearer,
                        )))
                    }
                    other => Err(ConnectorRuntimeError::RealModeNotBound(format!(
                        "github_write unknown kind `{other}`"
                    ))),
                }
            }
            other => Err(ConnectorRuntimeError::RealModeNotBound(format!(
                "github real client cannot handle operation `{other}`"
            ))),
        }
    }

    fn shape_response(&self, ctx: &RealCallContext<'_>, body: Value) -> Value {
        // `/search/issues` returns `{total_count, incomplete_results,
        // items: [...]}`. The runtime caller deserialises a
        // `Vec<TaskIssue>` from `response.payload`, so we lift
        // `items` out into the top-level array.
        if ctx.operation == "github_search" {
            if let Some(items) = body.get("items").cloned() {
                return items;
            }
        }
        body
    }
}

fn github_request(builder: reqwest::blocking::RequestBuilder, bearer: &str) -> reqwest::blocking::RequestBuilder {
    builder
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {bearer}"))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
}

fn string_field(payload: &Value, field: &str) -> Result<String, ConnectorRuntimeError> {
    payload
        .get(field)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            ConnectorRuntimeError::RealModeNotBound(format!(
                "github real client missing payload field `{field}`"
            ))
        })
}

fn split_owner_repo(value: &str) -> Result<(String, String), ConnectorRuntimeError> {
    let mut parts = value.splitn(2, '/');
    let owner = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    if owner.is_empty() || repo.is_empty() {
        return Err(ConnectorRuntimeError::RealModeNotBound(format!(
            "github real client expected `<owner>/<repo>`, got `{value}`"
        )));
    }
    Ok((owner.to_string(), repo.to_string()))
}

/// Minimal RFC 3986 path-component encoder for `+` and ` ` so the
/// GitHub `q` query parameter survives. We avoid pulling in a full
/// percent-encoding crate for the four characters the query needs.
fn url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 8);
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `BearerTokenResolver` for cases where the bearer is static — i.e.
/// a GitHub Personal Access Token. The resolver receives the token
/// id (as stored in `ConnectorAuthState`) and returns the bearer
/// the host has previously associated with it.
///
/// Slice 41K-B uses this directly; slice 41K-C wraps a more complex
/// `OAuth2RefreshResolver` around the same trait.
#[derive(Debug, Clone)]
pub struct StaticBearerResolver {
    bearers: std::collections::BTreeMap<String, String>,
}

impl StaticBearerResolver {
    pub fn new() -> Self {
        Self {
            bearers: std::collections::BTreeMap::new(),
        }
    }

    pub fn with_bearer(mut self, token_id: impl Into<String>, bearer: impl Into<String>) -> Self {
        self.bearers.insert(token_id.into(), bearer.into());
        self
    }
}

impl Default for StaticBearerResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl BearerTokenResolver for StaticBearerResolver {
    fn resolve_bearer(&self, token_id: &str) -> Result<String, BearerTokenError> {
        self.bearers
            .get(token_id)
            .cloned()
            .ok_or_else(|| BearerTokenError::NotFound(token_id.to_string()))
    }
}

/// Convenience: build a `ConnectorRealClient` for GitHub PAT auth
/// from a single `(token_id, pat)` pair. 41K-B's most common shape.
pub fn github_pat_real_client(
    token_id: impl Into<String>,
    pat: impl Into<String>,
) -> Result<Arc<dyn ConnectorRealClient>, ConnectorRuntimeError> {
    github_pat_real_client_with_base(token_id, pat, GITHUB_API_BASE)
}

/// Same as above but lets tests point at a local mock-server base
/// URL instead of `api.github.com`.
pub fn github_pat_real_client_with_base(
    token_id: impl Into<String>,
    pat: impl Into<String>,
    base_url: impl Into<String>,
) -> Result<Arc<dyn ConnectorRealClient>, ConnectorRuntimeError> {
    let resolver = Arc::new(
        StaticBearerResolver::new().with_bearer(token_id.into(), pat.into()),
    );
    let endpoints = Arc::new(GitHubEndpoints::new().with_base_url(base_url.into()));
    let client = ReqwestRealClient::new(resolver, endpoints)?;
    Ok(Arc::new(client))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::ConnectorAuthState;
    use crate::manifest::{ConnectorManifest, ConnectorScope, ConnectorScopeApproval};
    use serde_json::json;

    fn unwrap_http(plan: RealCallPlan) -> reqwest::blocking::RequestBuilder {
        match plan {
            RealCallPlan::Http(builder) => builder,
            RealCallPlan::Synthesized(value) => panic!(
                "expected RealCallPlan::Http but got Synthesized({value:?})"
            ),
        }
    }

    fn percent_decode(value: &str) -> String {
        let bytes = value.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                if let (Some(h), Some(l)) =
                    (hex_value(bytes[i + 1]), hex_value(bytes[i + 2]))
                {
                    out.push(h * 16 + l);
                    i += 3;
                    continue;
                }
            }
            out.push(bytes[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    fn hex_value(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            _ => None,
        }
    }

    fn ctx_for<'a>(
        manifest: &'a ConnectorManifest,
        scope: &'a ConnectorScope,
        auth: &'a ConnectorAuthState,
        operation: &'a str,
        payload: &'a Value,
    ) -> RealCallContext<'a> {
        RealCallContext {
            manifest,
            scope,
            auth,
            operation,
            payload,
            now_ms: 0,
        }
    }

    fn fake_manifest() -> ConnectorManifest {
        ConnectorManifest {
            schema: "corvid.connector.v1".to_string(),
            name: "tasks".to_string(),
            provider: "linear_github".to_string(),
            mode: vec![],
            scope: vec![],
            rate_limit: vec![],
            redaction: vec![],
            replay: vec![],
        }
    }

    fn fake_scope(id: &str, effects: &[&str]) -> ConnectorScope {
        ConnectorScope {
            id: id.to_string(),
            provider_scope: format!("github:{id}"),
            data_classes: vec!["task_metadata".to_string()],
            effects: effects.iter().map(|s| s.to_string()).collect(),
            approval: ConnectorScopeApproval::None,
        }
    }

    fn fake_auth() -> ConnectorAuthState {
        ConnectorAuthState::new(
            "tenant-1",
            "actor-1",
            "ghp-token-id",
            ["tasks.github_search", "tasks.github_write"],
            u64::MAX,
        )
    }

    /// Slice 41K-B positive: `github_search` produces a GET against
    /// the configured base + `/search/issues` with the query encoded
    /// and the `Authorization` / `Accept` / `X-GitHub-Api-Version`
    /// headers attached.
    #[test]
    fn search_builds_get_with_query_and_headers() {
        let endpoints = GitHubEndpoints::new().with_base_url("https://example.test");
        let client = reqwest::blocking::Client::new();
        let manifest = fake_manifest();
        let scope = fake_scope("tasks.github_search", &["network.read"]);
        let auth = fake_auth();
        let payload = json!({
            "owner": "octocat",
            "repo": "Hello-World",
            "query": "is:open label:bug",
            "limit": 5,
        });
        let ctx = ctx_for(&manifest, &scope, &auth, "github_search", &payload);
        let plan = endpoints
            .build_request(&ctx, "ghp_test", &client)
            .expect("build request");
        let request = unwrap_http(plan).build().expect("finalise");
        assert_eq!(request.method(), reqwest::Method::GET);
        let url = request.url().to_string();
        assert!(url.starts_with("https://example.test/search/issues?q="), "{url}");
        // reqwest's URL builder may further percent-encode the query
        // (`:` → `%3A`, `/` → `%2F`); match the URL-decoded form to
        // stay tolerant of that round-trip.
        let decoded = percent_decode(&url);
        assert!(decoded.contains("repo:octocat/Hello-World"), "{decoded}");
        assert!(url.contains("per_page=5"), "{url}");
        let headers = request.headers();
        assert_eq!(
            headers.get(reqwest::header::AUTHORIZATION).unwrap(),
            "Bearer ghp_test"
        );
        assert_eq!(
            headers.get(reqwest::header::ACCEPT).unwrap(),
            "application/vnd.github+json"
        );
        assert_eq!(
            headers.get("X-GitHub-Api-Version").unwrap(),
            "2022-11-28"
        );
    }

    /// Slice 41K-B positive: search responses get reshaped — the
    /// GitHub `/search/issues` envelope's `items` array is lifted to
    /// the top so the existing `Vec<TaskIssue>` deserialiser keeps
    /// working without changes.
    #[test]
    fn search_response_is_reshaped_to_items_array() {
        let endpoints = GitHubEndpoints::new();
        let manifest = fake_manifest();
        let scope = fake_scope("tasks.github_search", &["network.read"]);
        let auth = fake_auth();
        let payload = json!({});
        let ctx = ctx_for(&manifest, &scope, &auth, "github_search", &payload);
        let body = json!({
            "total_count": 2,
            "incomplete_results": false,
            "items": [
                {"number": 1, "title": "One"},
                {"number": 2, "title": "Two"},
            ],
        });
        let shaped = endpoints.shape_response(&ctx, body);
        assert_eq!(shaped.as_array().unwrap().len(), 2);
        assert_eq!(shaped[0]["number"], 1);
    }

    /// Slice 41K-B positive: `github_write` Create produces a POST
    /// to `/repos/{owner}/{repo}/issues` with a JSON body containing
    /// `title` + `body`.
    #[test]
    fn write_create_builds_post_to_issues_endpoint() {
        let endpoints = GitHubEndpoints::new().with_base_url("https://example.test");
        let client = reqwest::blocking::Client::new();
        let manifest = fake_manifest();
        let scope = fake_scope("tasks.github_write", &["network.write"]);
        let auth = fake_auth();
        let payload = json!({
            "provider": "github",
            "workspace_or_repo": "octocat/Hello-World",
            "issue_id": null,
            "title": "Bug",
            "body": "details",
            "kind": "Create",
            "approval_id": "approval-1",
        });
        let ctx = ctx_for(&manifest, &scope, &auth, "github_write", &payload);
        let plan = endpoints
            .build_request(&ctx, "ghp_write", &client)
            .expect("build request");
        let request = unwrap_http(plan).build().expect("finalise");
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(
            request.url().as_str(),
            "https://example.test/repos/octocat/Hello-World/issues"
        );
        let body = std::str::from_utf8(request.body().unwrap().as_bytes().unwrap()).unwrap();
        assert!(body.contains("\"title\":\"Bug\""), "{body}");
        assert!(body.contains("\"body\":\"details\""), "{body}");
    }

    /// Slice 41K-B positive: `github_write` Update produces a PATCH
    /// to `/repos/{owner}/{repo}/issues/{issue_id}`.
    #[test]
    fn write_update_builds_patch_with_issue_path() {
        let endpoints = GitHubEndpoints::new().with_base_url("https://example.test");
        let client = reqwest::blocking::Client::new();
        let manifest = fake_manifest();
        let scope = fake_scope("tasks.github_write", &["network.write"]);
        let auth = fake_auth();
        let payload = json!({
            "provider": "github",
            "workspace_or_repo": "octocat/Hello-World",
            "issue_id": "42",
            "title": "Bug2",
            "body": "more",
            "kind": "Update",
            "approval_id": "approval-1",
        });
        let ctx = ctx_for(&manifest, &scope, &auth, "github_write", &payload);
        let plan = endpoints
            .build_request(&ctx, "ghp_write", &client)
            .expect("build request");
        let request = unwrap_http(plan).build().expect("finalise");
        assert_eq!(request.method(), reqwest::Method::PATCH);
        assert_eq!(
            request.url().as_str(),
            "https://example.test/repos/octocat/Hello-World/issues/42"
        );
    }

    /// Slice 41K-B positive: `github_write` Comment produces a POST
    /// to `/repos/{owner}/{repo}/issues/{issue_id}/comments` with
    /// only `body` (no `title`).
    #[test]
    fn write_comment_builds_post_to_comments_endpoint() {
        let endpoints = GitHubEndpoints::new().with_base_url("https://example.test");
        let client = reqwest::blocking::Client::new();
        let manifest = fake_manifest();
        let scope = fake_scope("tasks.github_write", &["network.write"]);
        let auth = fake_auth();
        let payload = json!({
            "provider": "github",
            "workspace_or_repo": "octocat/Hello-World",
            "issue_id": "42",
            "title": "",
            "body": "thanks",
            "kind": "Comment",
            "approval_id": "approval-1",
        });
        let ctx = ctx_for(&manifest, &scope, &auth, "github_write", &payload);
        let plan = endpoints
            .build_request(&ctx, "ghp_write", &client)
            .expect("build request");
        let request = unwrap_http(plan).build().expect("finalise");
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(
            request.url().as_str(),
            "https://example.test/repos/octocat/Hello-World/issues/42/comments"
        );
        let body = std::str::from_utf8(request.body().unwrap().as_bytes().unwrap()).unwrap();
        assert!(body.contains("\"body\":\"thanks\""), "{body}");
        assert!(!body.contains("\"title\""), "{body}");
    }

    /// Slice 41K-B adversarial: `github_write` Update without an
    /// `issue_id` is refused before the HTTP request goes out, with
    /// a clear diagnostic.
    #[test]
    fn write_update_without_issue_id_is_refused() {
        let endpoints = GitHubEndpoints::new();
        let client = reqwest::blocking::Client::new();
        let manifest = fake_manifest();
        let scope = fake_scope("tasks.github_write", &["network.write"]);
        let auth = fake_auth();
        let payload = json!({
            "provider": "github",
            "workspace_or_repo": "octocat/Hello-World",
            "title": "no-id",
            "body": "x",
            "kind": "Update",
            "approval_id": "approval-1",
        });
        let ctx = ctx_for(&manifest, &scope, &auth, "github_write", &payload);
        let err = match endpoints.build_request(&ctx, "ghp", &client) {
            Err(e) => e,
            Ok(_) => panic!("expected refusal"),
        };
        assert!(
            matches!(&err, ConnectorRuntimeError::RealModeNotBound(msg) if msg.contains("issue_id")),
            "{err}"
        );
    }

    /// Slice 41K-B adversarial: a malformed `workspace_or_repo`
    /// without a `/` separator fails before the HTTP request.
    #[test]
    fn write_with_malformed_workspace_or_repo_is_refused() {
        let endpoints = GitHubEndpoints::new();
        let client = reqwest::blocking::Client::new();
        let manifest = fake_manifest();
        let scope = fake_scope("tasks.github_write", &["network.write"]);
        let auth = fake_auth();
        let payload = json!({
            "provider": "github",
            "workspace_or_repo": "missingslash",
            "title": "x",
            "body": "x",
            "kind": "Create",
            "approval_id": "approval-1",
        });
        let ctx = ctx_for(&manifest, &scope, &auth, "github_write", &payload);
        let err = match endpoints.build_request(&ctx, "ghp", &client) {
            Err(e) => e,
            Ok(_) => panic!("expected refusal"),
        };
        assert!(
            matches!(&err, ConnectorRuntimeError::RealModeNotBound(msg) if msg.contains("<owner>/<repo>")),
            "{err}"
        );
    }

    /// Slice 41K-B: `StaticBearerResolver` returns the bearer
    /// associated with a token id; unknown ids are
    /// `BearerTokenError::NotFound`.
    #[test]
    fn static_resolver_returns_or_refuses() {
        let resolver = StaticBearerResolver::new()
            .with_bearer("token-1", "ghp_secret_value");
        assert_eq!(resolver.resolve_bearer("token-1").unwrap(), "ghp_secret_value");
        assert!(matches!(
            resolver.resolve_bearer("token-2").unwrap_err(),
            BearerTokenError::NotFound(id) if id == "token-2"
        ));
    }

    /// Slice 41K-B convenience: `github_pat_real_client` produces a
    /// fully wired `ConnectorRealClient` from a single `(token_id,
    /// pat)` pair. Smoke-tests that the constructor doesn't fail.
    #[test]
    fn github_pat_real_client_constructs() {
        let client = github_pat_real_client("token-1", "ghp_test")
            .expect("construct");
        // The client implements ConnectorRealClient — i.e. the trait
        // object is dispatchable. We don't issue a live HTTP call;
        // those tests are gated behind CORVID_PROVIDER_LIVE=1.
        let _ = client;
    }

    /// Live integration test against `api.github.com`. Skipped by
    /// default; runs only when the `CORVID_PROVIDER_LIVE` env var
    /// is set and a `GITHUB_PAT` env var holds a valid token.
    ///
    /// The default CI matrix DOES NOT exercise this path. An
    /// opt-in matrix runs it. The test verifies the live HTTP
    /// path executes end-to-end and the response is shaped into
    /// the array the connector caller expects.
    #[test]
    #[ignore = "live network test; gated by CORVID_PROVIDER_LIVE=1"]
    fn live_search_against_api_github_com() {
        if std::env::var("CORVID_PROVIDER_LIVE").ok().as_deref() != Some("1") {
            eprintln!("skipping: CORVID_PROVIDER_LIVE != 1");
            return;
        }
        let pat = match std::env::var("GITHUB_PAT") {
            Ok(value) if !value.is_empty() => value,
            _ => {
                eprintln!("skipping: GITHUB_PAT not set");
                return;
            }
        };
        let client = github_pat_real_client("live-token", pat).expect("construct");
        let manifest = fake_manifest();
        let scope = fake_scope("tasks.github_search", &["network.read"]);
        let auth = fake_auth();
        let payload = serde_json::json!({
            "owner": "rust-lang",
            "repo": "rust",
            "query": "is:open label:E-easy",
            "limit": 1,
        });
        let ctx = ctx_for(&manifest, &scope, &auth, "github_search", &payload);
        let response = client.execute_real(&ctx).expect("github responds");
        let items = response.as_array().expect("array");
        assert!(items.len() <= 1);
    }
}
