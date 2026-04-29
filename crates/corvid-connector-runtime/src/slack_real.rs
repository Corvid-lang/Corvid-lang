//! Slack real-mode endpoints — slice 41K-C.
//!
//! Maps the Slack connector's `channel_read`, `dm_read`,
//! `thread_read`, `draft`, and `send` operations to the Slack Web
//! API. Slack drafts are local-only — there is no provider-side
//! draft API — so `draft` returns `RealCallPlan::Synthesized` with
//! a deterministic local draft id; the actual provider call
//! happens at `send` time via `chat.postMessage`. The connector's
//! caller stores the draft body and sends it on approve.
//!
//! Slack uses Unix timestamps in seconds for `oldest`/`thread_ts`,
//! and the connector models `since_ms` in milliseconds, so this
//! module performs the unit conversion.

use crate::oauth2_refresh::{InMemoryOAuth2Store, OAuth2RefreshResolver, OAuth2Tokens, ReqwestRefreshHook};
use crate::real_client::{
    ConnectorRealClient, OperationEndpoints, RealCallContext, RealCallPlan, ReqwestRealClient,
};
use crate::runtime::ConnectorRuntimeError;
use serde_json::Value;
use std::sync::Arc;

pub const SLACK_API_BASE: &str = "https://slack.com/api";

#[derive(Debug, Clone)]
pub struct SlackEndpoints {
    base_url: String,
}

impl Default for SlackEndpoints {
    fn default() -> Self {
        Self {
            base_url: SLACK_API_BASE.to_string(),
        }
    }
}

impl SlackEndpoints {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    fn conversations_history_url(&self, channel: &str, oldest_s: u64, limit: u32) -> String {
        let limit = limit.clamp(1, 1000);
        format!(
            "{}/conversations.history?channel={channel}&oldest={oldest_s}&limit={limit}",
            self.base_url
        )
    }

    fn conversations_replies_url(&self, channel: &str, thread_ts: &str) -> String {
        format!(
            "{}/conversations.replies?channel={channel}&ts={thread_ts}",
            self.base_url
        )
    }

    fn chat_post_message_url(&self) -> String {
        format!("{}/chat.postMessage", self.base_url)
    }
}

impl OperationEndpoints for SlackEndpoints {
    fn build_request(
        &self,
        ctx: &RealCallContext<'_>,
        bearer: &str,
        client: &reqwest::blocking::Client,
    ) -> Result<RealCallPlan, ConnectorRuntimeError> {
        match ctx.operation {
            "channel_read" | "dm_read" => {
                let channel = string_field(ctx.payload, "channel_id")?;
                let since_ms = ctx
                    .payload
                    .get("since_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let oldest_s = since_ms / 1_000;
                let limit = ctx
                    .payload
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(100) as u32;
                let url = self.conversations_history_url(&channel, oldest_s, limit);
                Ok(RealCallPlan::Http(slack_request(client.get(&url), bearer)))
            }
            "thread_read" => {
                let channel = string_field(ctx.payload, "channel_id")?;
                let thread_ts = string_field(ctx.payload, "thread_ts")?;
                let url = self.conversations_replies_url(&channel, &thread_ts);
                Ok(RealCallPlan::Http(slack_request(client.get(&url), bearer)))
            }
            "draft" => {
                // Slack has no server-side draft API. We synthesize a
                // local draft id from the channel + a stable hash of
                // the text so the same input produces the same id
                // (replay-friendly). The caller persists the body and
                // sends it on approval via `chat.postMessage`.
                let channel = string_field(ctx.payload, "channel_id")?;
                let text = ctx
                    .payload
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let approval_id = ctx
                    .payload
                    .get("approval_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let workspace_id = ctx
                    .payload
                    .get("workspace_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let id = format!(
                    "slack-draft-{}-{}",
                    channel,
                    stable_hash(text)
                );
                Ok(RealCallPlan::Synthesized(serde_json::json!({
                    "id": id,
                    "workspace_id": workspace_id,
                    "channel_id": channel,
                    "approval_id": approval_id,
                    "replay_key": format!("slack:draft:{workspace_id}:{channel}:{}", stable_hash(text)),
                })))
            }
            "send" => {
                let channel = string_field(ctx.payload, "channel_id")?;
                let text = ctx
                    .payload
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let thread_ts = ctx
                    .payload
                    .get("thread_ts")
                    .and_then(|v| v.as_str());
                let mut body = serde_json::json!({
                    "channel": channel,
                    "text": text,
                });
                if let Some(ts) = thread_ts {
                    body["thread_ts"] = Value::String(ts.to_string());
                }
                let url = self.chat_post_message_url();
                Ok(RealCallPlan::Http(
                    slack_request(client.post(&url).json(&body), bearer),
                ))
            }
            other => Err(ConnectorRuntimeError::RealModeNotBound(format!(
                "slack real client cannot handle operation `{other}`"
            ))),
        }
    }

    fn shape_response(&self, ctx: &RealCallContext<'_>, body: Value) -> Value {
        // `conversations.history` and `conversations.replies` both
        // return `{ok, messages: [...], ...}`. The connector caller
        // deserialises a `Vec<SlackMessage>`, so we lift `messages`
        // to the top.
        if matches!(ctx.operation, "channel_read" | "dm_read" | "thread_read") {
            if let Some(messages) = body.get("messages").cloned() {
                return messages;
            }
        }
        body
    }
}

fn slack_request(builder: reqwest::blocking::RequestBuilder, bearer: &str) -> reqwest::blocking::RequestBuilder {
    builder
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {bearer}"))
        .header(reqwest::header::ACCEPT, "application/json; charset=utf-8")
}

fn string_field(payload: &Value, field: &str) -> Result<String, ConnectorRuntimeError> {
    payload
        .get(field)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            ConnectorRuntimeError::RealModeNotBound(format!(
                "slack real client missing payload field `{field}`"
            ))
        })
}

fn stable_hash(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Convenience: build a Slack real client backed by the shared
/// OAuth2 refresh resolver against Slack's token endpoint.
pub fn slack_real_client(
    token_id: impl Into<String>,
    initial_tokens: OAuth2Tokens,
    client_id: impl Into<String>,
    client_secret: impl Into<String>,
) -> Result<Arc<dyn ConnectorRealClient>, ConnectorRuntimeError> {
    let store = Arc::new(InMemoryOAuth2Store::new());
    store.seed(token_id.into(), initial_tokens);
    let hook =
        Arc::new(ReqwestRefreshHook::slack(client_id, client_secret).map_err(|e| {
            ConnectorRuntimeError::RealModeNotBound(format!("oauth2 init failed: {e}"))
        })?);
    let resolver = Arc::new(OAuth2RefreshResolver::new(store, hook));
    let endpoints = Arc::new(SlackEndpoints::new());
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
            name: "slack".to_string(),
            provider: "slack".to_string(),
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
            provider_scope: format!("slack:{id}"),
            data_classes: vec!["chat_metadata".to_string()],
            effects: effects.iter().map(|s| s.to_string()).collect(),
            approval: ConnectorScopeApproval::None,
        }
    }

    fn fake_auth() -> ConnectorAuthState {
        ConnectorAuthState::new(
            "tenant-1",
            "actor-1",
            "tok-1",
            ["slack.channel_read", "slack.dm_read", "slack.thread_read", "slack.draft", "slack.send"],
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

    /// Slice 41K-C: channel_read builds a GET to
    /// `conversations.history?channel=...&oldest=<seconds>&limit=...`,
    /// converting the connector's `since_ms` into Slack's seconds.
    #[test]
    fn channel_read_builds_get_with_seconds_conversion() {
        let endpoints = SlackEndpoints::new().with_base_url("https://example.test/api");
        let client = reqwest::blocking::Client::new();
        let manifest = fake_manifest();
        let scope = fake_scope("slack.channel_read", &["network.read"]);
        let auth = fake_auth();
        let payload = json!({
            "workspace_id": "T01",
            "channel_id": "C01",
            "user_id": "U01",
            "since_ms": 5_000_000,
            "limit": 25,
        });
        let ctx = ctx_for(&manifest, &scope, &auth, "channel_read", &payload);
        let request = unwrap_http(endpoints.build_request(&ctx, "xoxb-test", &client).unwrap())
            .build()
            .unwrap();
        assert_eq!(request.method(), reqwest::Method::GET);
        assert_eq!(
            request.url().as_str(),
            "https://example.test/api/conversations.history?channel=C01&oldest=5000&limit=25"
        );
        assert_eq!(
            request.headers().get(reqwest::header::AUTHORIZATION).unwrap(),
            "Bearer xoxb-test"
        );
    }

    /// Slice 41K-C: thread_read builds a GET to
    /// `conversations.replies?channel=...&ts=...`.
    #[test]
    fn thread_read_builds_get_to_replies() {
        let endpoints = SlackEndpoints::new().with_base_url("https://example.test/api");
        let client = reqwest::blocking::Client::new();
        let manifest = fake_manifest();
        let scope = fake_scope("slack.thread_read", &["network.read"]);
        let auth = fake_auth();
        let payload = json!({
            "workspace_id": "T01",
            "channel_id": "C01",
            "thread_ts": "1700000000.000100",
            "user_id": "U01",
        });
        let ctx = ctx_for(&manifest, &scope, &auth, "thread_read", &payload);
        let request = unwrap_http(endpoints.build_request(&ctx, "xoxb", &client).unwrap())
            .build()
            .unwrap();
        assert_eq!(
            request.url().as_str(),
            "https://example.test/api/conversations.replies?channel=C01&ts=1700000000.000100"
        );
    }

    /// Slice 41K-C: response shaping lifts `messages` for the read
    /// operations.
    #[test]
    fn read_response_lifts_messages_array() {
        let endpoints = SlackEndpoints::new();
        let manifest = fake_manifest();
        let scope = fake_scope("slack.channel_read", &["network.read"]);
        let auth = fake_auth();
        let payload = json!({});
        let ctx = ctx_for(&manifest, &scope, &auth, "channel_read", &payload);
        let body = json!({
            "ok": true,
            "messages": [{"text": "hi", "ts": "1.0"}],
        });
        let shaped = endpoints.shape_response(&ctx, body);
        assert_eq!(shaped.as_array().unwrap().len(), 1);
        assert_eq!(shaped[0]["text"], "hi");
    }

    /// Slice 41K-C: send builds a POST to `chat.postMessage` with
    /// `channel`, `text`, and optional `thread_ts`.
    #[test]
    fn send_builds_post_to_chat_post_message() {
        let endpoints = SlackEndpoints::new().with_base_url("https://example.test/api");
        let client = reqwest::blocking::Client::new();
        let manifest = fake_manifest();
        let scope = fake_scope("slack.send", &["network.write"]);
        let auth = fake_auth();
        let payload = json!({
            "workspace_id": "T01",
            "channel_id": "C01",
            "text": "hello world",
            "thread_ts": "1700000000.000100",
            "approval_id": "approval-1",
        });
        let ctx = ctx_for(&manifest, &scope, &auth, "send", &payload);
        let request = unwrap_http(endpoints.build_request(&ctx, "xoxb", &client).unwrap())
            .build()
            .unwrap();
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(
            request.url().as_str(),
            "https://example.test/api/chat.postMessage"
        );
        let body_str = std::str::from_utf8(request.body().unwrap().as_bytes().unwrap()).unwrap();
        assert!(body_str.contains("\"channel\":\"C01\""), "{body_str}");
        assert!(body_str.contains("\"text\":\"hello world\""), "{body_str}");
        assert!(body_str.contains("\"thread_ts\":\"1700000000.000100\""), "{body_str}");
    }

    /// Slice 41K-C: draft is synthesized — no HTTP call. The
    /// returned id is deterministic for the same `(channel, text)`
    /// pair so replay produces the same draft id.
    #[test]
    fn draft_is_synthesized_with_stable_id() {
        let endpoints = SlackEndpoints::new();
        let client = reqwest::blocking::Client::new();
        let manifest = fake_manifest();
        let scope = fake_scope("slack.draft", &["network.write"]);
        let auth = fake_auth();
        let payload = json!({
            "workspace_id": "T01",
            "channel_id": "C01",
            "user_id": "U01",
            "text": "draft body",
            "thread_ts": null,
            "approval_id": "approval-1",
        });
        let ctx = ctx_for(&manifest, &scope, &auth, "draft", &payload);
        let plan = endpoints.build_request(&ctx, "xoxb", &client).unwrap();
        let value = match plan {
            RealCallPlan::Synthesized(v) => v,
            RealCallPlan::Http(_) => panic!("expected synthesized"),
        };
        let id = value["id"].as_str().unwrap();
        assert!(id.starts_with("slack-draft-C01-"));
        // Stable across two calls with the same input.
        let plan2 = endpoints.build_request(&ctx, "xoxb", &client).unwrap();
        let value2 = match plan2 {
            RealCallPlan::Synthesized(v) => v,
            RealCallPlan::Http(_) => panic!("expected synthesized"),
        };
        assert_eq!(value["id"], value2["id"]);
    }
}
