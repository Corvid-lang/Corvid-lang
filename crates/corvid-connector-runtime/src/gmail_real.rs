//! Gmail real-mode endpoints — slice 41K-C.
//!
//! Maps the Gmail connector's `search`, `read_metadata`, `draft`,
//! and `send` operations to the Gmail REST v1 API. Bearer tokens
//! are resolved via the shared `OAuth2RefreshResolver` so an
//! expiring access token is rotated against
//! `https://oauth2.googleapis.com/token` before the call goes out.
//!
//! Live integration tests (gated by `CORVID_PROVIDER_LIVE=1` plus a
//! `GMAIL_REFRESH_TOKEN` / `GMAIL_CLIENT_ID` / `GMAIL_CLIENT_SECRET`
//! env-var triple) live alongside slice 41K-B's GitHub live test.
//! Default CI runs only the URL/request shaping tests.

use crate::oauth2_refresh::{InMemoryOAuth2Store, OAuth2RefreshResolver, OAuth2Tokens, ReqwestRefreshHook};
use crate::real_client::{
    ConnectorRealClient, OperationEndpoints, RealCallContext, RealCallPlan, ReqwestRealClient,
};
use crate::runtime::ConnectorRuntimeError;
use base64::Engine;
use serde_json::Value;
use std::sync::Arc;

pub const GMAIL_API_BASE: &str = "https://gmail.googleapis.com/gmail/v1";

#[derive(Debug, Clone)]
pub struct GmailEndpoints {
    base_url: String,
}

impl Default for GmailEndpoints {
    fn default() -> Self {
        Self {
            base_url: GMAIL_API_BASE.to_string(),
        }
    }
}

impl GmailEndpoints {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    fn search_url(&self, user_id: &str, query: &str, max_results: u32) -> String {
        let q = url_encode(query);
        let max = max_results.clamp(1, 500);
        format!(
            "{}/users/{}/messages?q={q}&maxResults={max}",
            self.base_url, user_id
        )
    }

    fn message_metadata_url(&self, user_id: &str, message_id: &str) -> String {
        format!(
            "{}/users/{}/messages/{}?format=metadata",
            self.base_url, user_id, message_id
        )
    }

    fn drafts_url(&self, user_id: &str) -> String {
        format!("{}/users/{}/drafts", self.base_url, user_id)
    }

    fn drafts_send_url(&self, user_id: &str) -> String {
        format!("{}/users/{}/drafts/send", self.base_url, user_id)
    }
}

impl OperationEndpoints for GmailEndpoints {
    fn build_request(
        &self,
        ctx: &RealCallContext<'_>,
        bearer: &str,
        client: &reqwest::blocking::Client,
    ) -> Result<RealCallPlan, ConnectorRuntimeError> {
        match ctx.operation {
            "search" => {
                let user_id = string_field(ctx.payload, "user_id")?;
                let query = string_field(ctx.payload, "query")?;
                let max_results = ctx
                    .payload
                    .get("max_results")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(20) as u32;
                let url = self.search_url(&user_id, &query, max_results);
                Ok(RealCallPlan::Http(google_request(client.get(&url), bearer)))
            }
            "read_metadata" => {
                let user_id = string_field(ctx.payload, "user_id")?;
                let message_id = string_field(ctx.payload, "message_id")?;
                let url = self.message_metadata_url(&user_id, &message_id);
                Ok(RealCallPlan::Http(google_request(client.get(&url), bearer)))
            }
            "draft" => {
                let user_id = string_field(ctx.payload, "user_id")?;
                let to = ctx
                    .payload
                    .get("to")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                let subject = ctx
                    .payload
                    .get("subject")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let body = ctx
                    .payload
                    .get("body")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let thread_id = ctx
                    .payload
                    .get("thread_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                let raw_message = build_rfc2822_message(&to, subject, body);
                let raw_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(raw_message.as_bytes());
                let mut payload = serde_json::json!({
                    "message": { "raw": raw_b64 },
                });
                if let Some(thread) = thread_id {
                    payload["message"]["threadId"] = Value::String(thread);
                }
                let url = self.drafts_url(&user_id);
                Ok(RealCallPlan::Http(
                    google_request(client.post(&url).json(&payload), bearer),
                ))
            }
            "send" => {
                let user_id = string_field(ctx.payload, "user_id")?;
                let draft_id = string_field(ctx.payload, "draft_id")?;
                let payload = serde_json::json!({"id": draft_id});
                let url = self.drafts_send_url(&user_id);
                Ok(RealCallPlan::Http(
                    google_request(client.post(&url).json(&payload), bearer),
                ))
            }
            other => Err(ConnectorRuntimeError::RealModeNotBound(format!(
                "gmail real client cannot handle operation `{other}`"
            ))),
        }
    }

    fn shape_response(&self, ctx: &RealCallContext<'_>, body: Value) -> Value {
        // Gmail's `users.messages.list` returns `{messages: [{id, threadId}], ...}`.
        // The connector's caller deserialises a `Vec<GmailMessageMetadata>`,
        // so we lift `messages` to the top.
        if ctx.operation == "search" {
            if let Some(messages) = body.get("messages").cloned() {
                return messages;
            }
        }
        body
    }
}

fn google_request(builder: reqwest::blocking::RequestBuilder, bearer: &str) -> reqwest::blocking::RequestBuilder {
    builder
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {bearer}"))
        .header(reqwest::header::ACCEPT, "application/json")
}

fn string_field(payload: &Value, field: &str) -> Result<String, ConnectorRuntimeError> {
    payload
        .get(field)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            ConnectorRuntimeError::RealModeNotBound(format!(
                "gmail real client missing payload field `{field}`"
            ))
        })
}

fn build_rfc2822_message(to: &str, subject: &str, body: &str) -> String {
    // Minimal RFC 2822: To, Subject, blank line, body. Production
    // code may add MIME-Version, Content-Type; the Gmail API
    // accepts plain-text bodies without those. UTF-8 is preserved
    // because base64url-encoding is byte-exact.
    let mut out = String::with_capacity(to.len() + subject.len() + body.len() + 32);
    out.push_str("To: ");
    out.push_str(to);
    out.push_str("\r\n");
    out.push_str("Subject: ");
    out.push_str(subject);
    out.push_str("\r\n\r\n");
    out.push_str(body);
    out
}

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

/// Convenience: build a Gmail real client backed by the shared
/// OAuth2 refresh resolver. The token store is the seeded in-memory
/// store; a production deployment supplies the Phase 37-G encrypted
/// token store via its own `OAuth2TokenStore` impl.
pub fn gmail_real_client(
    token_id: impl Into<String>,
    initial_tokens: OAuth2Tokens,
    client_id: impl Into<String>,
    client_secret: impl Into<String>,
) -> Result<Arc<dyn ConnectorRealClient>, ConnectorRuntimeError> {
    let store = Arc::new(InMemoryOAuth2Store::new());
    store.seed(token_id.into(), initial_tokens);
    let hook =
        Arc::new(ReqwestRefreshHook::google(client_id, client_secret).map_err(|e| {
            ConnectorRuntimeError::RealModeNotBound(format!("oauth2 init failed: {e}"))
        })?);
    let resolver = Arc::new(OAuth2RefreshResolver::new(store, hook));
    let endpoints = Arc::new(GmailEndpoints::new());
    Ok(Arc::new(ReqwestRealClient::new(resolver, endpoints)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::ConnectorAuthState;
    use crate::manifest::{ConnectorManifest, ConnectorScope, ConnectorScopeApproval};
    use serde_json::json;

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
            name: "gmail".to_string(),
            provider: "google".to_string(),
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
            provider_scope: "https://googleapis.com/auth/gmail.metadata".to_string(),
            data_classes: vec!["email_metadata".to_string()],
            effects: effects.iter().map(|s| s.to_string()).collect(),
            approval: ConnectorScopeApproval::None,
        }
    }

    fn fake_auth() -> ConnectorAuthState {
        ConnectorAuthState::new(
            "tenant-1",
            "actor-1",
            "tok-1",
            ["gmail.search", "gmail.read_metadata", "gmail.draft", "gmail.send"],
            u64::MAX,
        )
    }

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

    /// Slice 41K-C: search builds a GET to
    /// `/users/{user_id}/messages?q=...&maxResults=...` with bearer
    /// auth.
    #[test]
    fn search_builds_get_with_query() {
        let endpoints = GmailEndpoints::new().with_base_url("https://example.test");
        let client = reqwest::blocking::Client::new();
        let manifest = fake_manifest();
        let scope = fake_scope("gmail.search", &["network.read"]);
        let auth = fake_auth();
        let payload = json!({
            "user_id": "user-1",
            "query": "is:unread newer_than:1d",
            "max_results": 5,
        });
        let ctx = ctx_for(&manifest, &scope, &auth, "search", &payload);
        let plan = endpoints
            .build_request(&ctx, "ya29.test", &client)
            .expect("build");
        let request = unwrap_http(plan).build().expect("finalise");
        assert_eq!(request.method(), reqwest::Method::GET);
        let url = request.url().to_string();
        assert!(url.starts_with("https://example.test/users/user-1/messages?q="), "{url}");
        let decoded = percent_decode(&url);
        assert!(decoded.contains("is:unread newer_than:1d"), "{decoded}");
        assert!(url.contains("maxResults=5"), "{url}");
        assert_eq!(
            request.headers().get(reqwest::header::AUTHORIZATION).unwrap(),
            "Bearer ya29.test"
        );
    }

    /// Slice 41K-C: search response shaping lifts the `messages`
    /// array so the connector caller's deserialiser keeps working.
    #[test]
    fn search_response_lifts_messages_array() {
        let endpoints = GmailEndpoints::new();
        let manifest = fake_manifest();
        let scope = fake_scope("gmail.search", &["network.read"]);
        let auth = fake_auth();
        let payload = json!({});
        let ctx = ctx_for(&manifest, &scope, &auth, "search", &payload);
        let body = json!({
            "messages": [{"id": "m1", "threadId": "t1"}],
            "resultSizeEstimate": 1,
        });
        let shaped = endpoints.shape_response(&ctx, body);
        assert_eq!(shaped.as_array().unwrap().len(), 1);
        assert_eq!(shaped[0]["id"], "m1");
    }

    /// Slice 41K-C: read_metadata builds a GET to
    /// `/users/{user_id}/messages/{message_id}?format=metadata`.
    #[test]
    fn read_metadata_builds_get() {
        let endpoints = GmailEndpoints::new().with_base_url("https://example.test");
        let client = reqwest::blocking::Client::new();
        let manifest = fake_manifest();
        let scope = fake_scope("gmail.read_metadata", &["network.read"]);
        let auth = fake_auth();
        let payload = json!({
            "user_id": "user-1",
            "message_id": "m-42",
        });
        let ctx = ctx_for(&manifest, &scope, &auth, "read_metadata", &payload);
        let request = unwrap_http(endpoints.build_request(&ctx, "ya29", &client).unwrap())
            .build()
            .unwrap();
        assert_eq!(request.method(), reqwest::Method::GET);
        assert_eq!(
            request.url().as_str(),
            "https://example.test/users/user-1/messages/m-42?format=metadata"
        );
    }

    /// Slice 41K-C: draft builds a POST to `/users/{user_id}/drafts`
    /// with the message base64url-encoded as RFC 2822.
    #[test]
    fn draft_builds_post_with_base64_rfc2822_message() {
        let endpoints = GmailEndpoints::new().with_base_url("https://example.test");
        let client = reqwest::blocking::Client::new();
        let manifest = fake_manifest();
        let scope = fake_scope("gmail.draft", &["network.write"]);
        let auth = fake_auth();
        let payload = json!({
            "user_id": "user-1",
            "to": ["alice@example.com", "bob@example.com"],
            "subject": "Hi",
            "body": "Test draft.",
            "thread_id": null,
            "approval_id": "approval-1",
        });
        let ctx = ctx_for(&manifest, &scope, &auth, "draft", &payload);
        let request = unwrap_http(endpoints.build_request(&ctx, "ya29", &client).unwrap())
            .build()
            .unwrap();
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(
            request.url().as_str(),
            "https://example.test/users/user-1/drafts"
        );
        let body_bytes = request.body().unwrap().as_bytes().unwrap();
        let body_json: Value = serde_json::from_slice(body_bytes).unwrap();
        let raw = body_json["message"]["raw"].as_str().unwrap();
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(raw)
            .unwrap();
        let decoded_str = std::str::from_utf8(&decoded).unwrap();
        assert!(decoded_str.contains("To: alice@example.com, bob@example.com"));
        assert!(decoded_str.contains("Subject: Hi"));
        assert!(decoded_str.contains("Test draft."));
    }

    /// Slice 41K-C: send builds a POST to
    /// `/users/{user_id}/drafts/send` with `{id: draft_id}`.
    #[test]
    fn send_builds_post_with_draft_id() {
        let endpoints = GmailEndpoints::new().with_base_url("https://example.test");
        let client = reqwest::blocking::Client::new();
        let manifest = fake_manifest();
        let scope = fake_scope("gmail.send", &["network.write"]);
        let auth = fake_auth();
        let payload = json!({
            "user_id": "user-1",
            "draft_id": "d-1",
            "approval_id": "approval-1",
        });
        let ctx = ctx_for(&manifest, &scope, &auth, "send", &payload);
        let request = unwrap_http(endpoints.build_request(&ctx, "ya29", &client).unwrap())
            .build()
            .unwrap();
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(
            request.url().as_str(),
            "https://example.test/users/user-1/drafts/send"
        );
        let body_str = std::str::from_utf8(request.body().unwrap().as_bytes().unwrap()).unwrap();
        assert!(body_str.contains("\"id\":\"d-1\""), "{body_str}");
    }
}
