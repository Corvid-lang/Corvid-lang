//! `corvid connectors` CLI subcommand surface — slice 41L.
//!
//! Wires the Phase 41 connector runtime into the `corvid` CLI.
//! Users get a top-level surface for inspecting available
//! connectors, validating their manifests, executing operations
//! against mock / replay / real modes, and managing OAuth2 token
//! lifecycle (PKCE init + force-refresh).
//!
//! Real-mode commands gate on `CORVID_PROVIDER_LIVE=1` so a
//! developer cannot accidentally fire a live request from a local
//! command — the same posture `ConnectorRuntime` enforces in code.
//! Webhook signature verification is implemented inline here as a
//! standalone primitive so a CI hook or `curl | corvid connectors
//! verify-webhook` pipeline can validate inbound payloads
//! independently of an HTTP server.

use anyhow::{anyhow, Context, Result};
use corvid_connector_runtime::{
    calendar_manifest, file_manifest, github_pat_real_client, gmail_manifest, gmail_real_client,
    ms365_manifest, slack_manifest, slack_real_client, task_manifest, validate_connector_manifest,
    verify_webhook, BearerTokenResolver, ConnectorAuthState, ConnectorManifest,
    ConnectorRealClient, ConnectorRequest, ConnectorRuntime, ConnectorRuntimeError,
    ConnectorRuntimeMode, InMemoryOAuth2Store, OAuth2RefreshResolver, OAuth2Tokens,
    ReqwestRefreshHook, WebhookProvider, WebhookVerificationOutcome, WebhookVerifyInputs,
};
use serde_json::Value;
use sha2::Sha256;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub struct ConnectorListEntry {
    pub name: String,
    pub provider: String,
    pub modes: Vec<String>,
    pub scope_count: usize,
    pub write_scopes: Vec<String>,
    pub rate_limit_summary: String,
    pub redaction_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConnectorCheckEntry {
    pub name: String,
    pub valid: bool,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConnectorRunOutput {
    pub connector: String,
    pub operation: String,
    pub mode: String,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OauthInitOutput {
    pub provider: String,
    pub state: String,
    pub authorization_url: String,
    pub code_verifier: String,
    pub code_challenge: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WebhookVerifyOutput {
    pub valid: bool,
    pub algorithm: String,
    pub outcome: String,
}

/// Returns the catalog of connectors built into Corvid. Each entry
/// is the parsed manifest summarised into a one-line table row.
pub fn run_list() -> Result<Vec<ConnectorListEntry>> {
    let manifests = shipped_manifests()?;
    let mut entries = Vec::with_capacity(manifests.len());
    for (_, manifest) in manifests {
        entries.push(summarise_manifest(&manifest));
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// Validates every shipped connector manifest and returns one
/// `ConnectorCheckEntry` per connector. With `live = true` the
/// caller indicates real-provider drift detection should run; this
/// slice flags it as a deferred bounty-extension behaviour and the
/// per-connector live drift narrator lands in slice 41M alongside
/// the webhook receive end-to-end.
pub fn run_check(live: bool) -> Result<Vec<ConnectorCheckEntry>> {
    let manifests = shipped_manifests()?;
    let mut entries = Vec::with_capacity(manifests.len());
    for (name, manifest) in manifests {
        let report = validate_connector_manifest(&manifest);
        let diagnostics = report
            .diagnostics
            .iter()
            .map(|d| format!("{d}"))
            .collect::<Vec<_>>();
        entries.push(ConnectorCheckEntry {
            name: name.to_string(),
            valid: report.valid,
            diagnostics,
        });
    }
    if live && std::env::var("CORVID_PROVIDER_LIVE").as_deref() != Ok("1") {
        return Err(anyhow!(
            "`--live` requires `CORVID_PROVIDER_LIVE=1` plus per-provider \
             credentials — refusing to issue live drift probes without \
             explicit opt-in. The default `corvid connectors check` \
             validates manifests without any network call."
        ));
    }
    // Live drift narration is deferred to slice 41M (per the
    // ROADMAP audit-correction track). Surfacing this honestly
    // rather than silently no-op'ing.
    if live {
        return Err(anyhow!(
            "Live drift narration is implemented end-to-end in slice 41M \
             (per `docs/effects-spec/bounty.md`). This slice ships \
             manifest-only validation; rerun without `--live` for the \
             validation report."
        ));
    }
    Ok(entries)
}

/// Drive a connector operation against a chosen mode. Mock /
/// Replay use the in-memory mock dictionary; Real builds the
/// per-connector real client and dispatches through reqwest. Real
/// mode gates on `CORVID_PROVIDER_LIVE=1` and the relevant
/// per-provider credential env vars.
pub fn run_run(args: ConnectorRunArgs) -> Result<ConnectorRunOutput> {
    let manifests = shipped_manifests()?;
    let manifest = manifests
        .iter()
        .find(|(name, _)| *name == args.connector.as_str())
        .map(|(_, m)| m.clone())
        .ok_or_else(|| {
            anyhow!(
                "unknown connector `{}`; one of {}",
                args.connector,
                shipped_connector_names().join(", ")
            )
        })?;

    let mode = match args.mode.as_str() {
        "mock" => ConnectorRuntimeMode::Mock,
        "replay" => ConnectorRuntimeMode::Replay,
        "real" => {
            if std::env::var("CORVID_PROVIDER_LIVE").as_deref() != Ok("1") {
                return Err(anyhow!(
                    "real mode requires `CORVID_PROVIDER_LIVE=1` to opt in \
                     to provider HTTP calls"
                ));
            }
            ConnectorRuntimeMode::Real
        }
        other => return Err(anyhow!("unknown mode `{other}`; expected mock|replay|real")),
    };

    let auth = ConnectorAuthState::new(
        args.tenant_id.clone(),
        args.actor_id.clone(),
        args.token_id.clone(),
        manifest
            .scope
            .iter()
            .map(|s| s.id.clone())
            .collect::<Vec<_>>(),
        args.now_ms.saturating_add(60_000_000),
    );
    let mut runtime = ConnectorRuntime::new(manifest.clone(), auth, mode);

    if matches!(mode, ConnectorRuntimeMode::Mock | ConnectorRuntimeMode::Replay) {
        if let Some(ref payload) = args.mock_payload {
            runtime.insert_mock(args.operation.clone(), payload.clone());
        }
    }

    if matches!(mode, ConnectorRuntimeMode::Real) {
        let client = build_real_client_for(&args.connector, &args.token_id)?;
        runtime = runtime.with_real_client(client);
    }

    let payload = args.payload.clone().unwrap_or(Value::Null);
    let response = runtime
        .execute(ConnectorRequest {
            scope_id: args.scope_id.clone(),
            operation: args.operation.clone(),
            payload,
            approval_id: args.approval_id.clone(),
            replay_key: args.replay_key.clone(),
            now_ms: args.now_ms,
        })
        .map_err(map_runtime_error)?;

    Ok(ConnectorRunOutput {
        connector: args.connector,
        operation: args.operation,
        mode: args.mode,
        payload: response.payload,
    })
}

#[derive(Debug, Clone)]
pub struct ConnectorRunArgs {
    pub connector: String,
    pub operation: String,
    pub scope_id: String,
    pub mode: String,
    pub payload: Option<Value>,
    pub mock_payload: Option<Value>,
    pub approval_id: String,
    pub replay_key: String,
    pub tenant_id: String,
    pub actor_id: String,
    pub token_id: String,
    pub now_ms: u64,
}

/// Build a per-connector real client from environment-supplied
/// credentials. GitHub uses a Personal Access Token; Gmail and
/// Slack use OAuth2 refresh.
fn build_real_client_for(
    connector: &str,
    token_id: &str,
) -> Result<Arc<dyn ConnectorRealClient>> {
    match connector {
        "tasks" => {
            // Tasks connector real-mode supports GitHub today (Linear
            // is bounty-extension). The PAT comes from `GITHUB_PAT`.
            let pat = std::env::var("GITHUB_PAT").context(
                "tasks real mode requires `GITHUB_PAT` env var (GitHub Personal Access Token)",
            )?;
            github_pat_real_client(token_id.to_string(), pat).map_err(map_runtime_error)
        }
        "gmail" => {
            let access = std::env::var("GMAIL_ACCESS_TOKEN")
                .context("gmail real mode requires `GMAIL_ACCESS_TOKEN`")?;
            let refresh = std::env::var("GMAIL_REFRESH_TOKEN")
                .context("gmail real mode requires `GMAIL_REFRESH_TOKEN`")?;
            let client_id = std::env::var("GMAIL_CLIENT_ID")
                .context("gmail real mode requires `GMAIL_CLIENT_ID`")?;
            let client_secret = std::env::var("GMAIL_CLIENT_SECRET")
                .context("gmail real mode requires `GMAIL_CLIENT_SECRET`")?;
            let initial = OAuth2Tokens {
                access_token: access,
                refresh_token: refresh,
                expires_at_ms: 0,
            };
            gmail_real_client(token_id.to_string(), initial, client_id, client_secret)
                .map_err(map_runtime_error)
        }
        "slack" => {
            let access = std::env::var("SLACK_ACCESS_TOKEN")
                .context("slack real mode requires `SLACK_ACCESS_TOKEN`")?;
            let refresh = std::env::var("SLACK_REFRESH_TOKEN")
                .context("slack real mode requires `SLACK_REFRESH_TOKEN`")?;
            let client_id = std::env::var("SLACK_CLIENT_ID")
                .context("slack real mode requires `SLACK_CLIENT_ID`")?;
            let client_secret = std::env::var("SLACK_CLIENT_SECRET")
                .context("slack real mode requires `SLACK_CLIENT_SECRET`")?;
            let initial = OAuth2Tokens {
                access_token: access,
                refresh_token: refresh,
                expires_at_ms: 0,
            };
            slack_real_client(token_id.to_string(), initial, client_id, client_secret)
                .map_err(map_runtime_error)
        }
        other => Err(anyhow!(
            "real mode for connector `{other}` is not yet wired \
             (slice 41M pending for ms365/calendar/files)"
        )),
    }
}

/// Initiate an OAuth2 PKCE authorization flow. Generates a state,
/// code verifier, and code challenge; constructs the provider's
/// authorization URL with the corvid-supplied callback. The state
/// is what the caller must persist (Phase 39 oauth_state path
/// records this); the URL is what the user opens in a browser.
pub fn run_oauth_init(args: OauthInitArgs) -> Result<OauthInitOutput> {
    let provider = args.provider.to_lowercase();
    let (auth_endpoint, default_scopes) = match provider.as_str() {
        "gmail" => (
            "https://accounts.google.com/o/oauth2/v2/auth",
            vec![
                "https://www.googleapis.com/auth/gmail.readonly",
                "https://www.googleapis.com/auth/gmail.compose",
                "https://www.googleapis.com/auth/gmail.send",
            ],
        ),
        "slack" => (
            "https://slack.com/oauth/v2/authorize",
            vec!["channels:history", "channels:read", "chat:write"],
        ),
        "ms365" => (
            "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
            vec!["Mail.Read", "Mail.Send", "Calendars.Read"],
        ),
        other => {
            return Err(anyhow!(
                "unknown provider `{other}`; expected gmail|slack|ms365"
            ))
        }
    };
    let scopes: Vec<String> = if args.scopes.is_empty() {
        default_scopes.iter().map(|s| s.to_string()).collect()
    } else {
        args.scopes
    };

    let state = random_b64url_bytes(16);
    let code_verifier = random_b64url_bytes(32);
    let code_challenge = pkce_code_challenge(&code_verifier);

    let redirect = args
        .redirect_uri
        .unwrap_or_else(|| "http://localhost:8765/oauth/callback".to_string());
    let scope_param = scopes.join(" ");
    let authorization_url = format!(
        "{auth}?response_type=code&client_id={cid}&redirect_uri={ru}&scope={sc}&state={st}&code_challenge={cc}&code_challenge_method=S256",
        auth = auth_endpoint,
        cid = url_encode(&args.client_id),
        ru = url_encode(&redirect),
        sc = url_encode(&scope_param),
        st = state,
        cc = code_challenge,
    );

    Ok(OauthInitOutput {
        provider,
        state,
        authorization_url,
        code_verifier,
        code_challenge,
    })
}

#[derive(Debug, Clone)]
pub struct OauthInitArgs {
    pub provider: String,
    pub client_id: String,
    pub redirect_uri: Option<String>,
    pub scopes: Vec<String>,
}

/// Force-rotate an OAuth2 token. Reads the current `(access,
/// refresh)` pair from env vars (the production deployment uses
/// the encrypted token store; this slice's CLI surface stays
/// dev-friendly), refreshes against the provider's token endpoint,
/// and prints the new pair so the operator can persist it.
pub fn run_oauth_rotate(args: OauthRotateArgs) -> Result<OauthRotateOutput> {
    let provider = args.provider.to_lowercase();
    let store = Arc::new(InMemoryOAuth2Store::new());
    store.seed(
        args.token_id.clone(),
        OAuth2Tokens {
            access_token: args.access_token.clone(),
            refresh_token: args.refresh_token.clone(),
            // Force expiry so the resolver MUST refresh.
            expires_at_ms: 0,
        },
    );
    let hook = match provider.as_str() {
        "gmail" => Arc::new(
            ReqwestRefreshHook::google(args.client_id.clone(), args.client_secret.clone())
                .map_err(|e| anyhow!("oauth refresh hook init failed: {e}"))?,
        ) as Arc<dyn corvid_connector_runtime::OAuth2RefreshHook>,
        "slack" => Arc::new(
            ReqwestRefreshHook::slack(args.client_id.clone(), args.client_secret.clone())
                .map_err(|e| anyhow!("oauth refresh hook init failed: {e}"))?,
        ) as Arc<dyn corvid_connector_runtime::OAuth2RefreshHook>,
        other => {
            return Err(anyhow!(
                "unknown provider `{other}`; expected gmail|slack"
            ))
        }
    };
    let resolver = OAuth2RefreshResolver::new(store.clone(), hook);
    let new_access = resolver
        .resolve_bearer(&args.token_id)
        .map_err(|e| anyhow!("refresh failed: {e}"))?;
    let snapshot = store
        .snapshot(&args.token_id)
        .ok_or_else(|| anyhow!("token gone after refresh"))?;
    Ok(OauthRotateOutput {
        provider,
        access_token: new_access,
        refresh_token: snapshot.refresh_token,
        expires_at_ms: snapshot.expires_at_ms,
    })
}

#[derive(Debug, Clone)]
pub struct OauthRotateArgs {
    pub provider: String,
    pub token_id: String,
    pub access_token: String,
    pub refresh_token: String,
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OauthRotateOutput {
    pub provider: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at_ms: u64,
}

/// Verify an inbound webhook signature. With `--provider` set to
/// `github`/`slack`/`linear`, dispatches to the per-provider
/// verifier in `corvid-connector-runtime::webhook_verify` (slice
/// 41M-A) which knows the provider-specific header conventions
/// (Slack's `v0:<ts>:<body>` basestring with replay protection,
/// GitHub's `sha256=` prefix, Linear's no-prefix hex). Without a
/// provider, falls back to the raw HMAC-SHA256 verifier suitable
/// for custom webhook integrations whose signing scheme is
/// "secret + body → hex digest" with optional `sha256=` prefix.
pub fn run_verify_webhook(args: WebhookVerifyArgs) -> Result<WebhookVerifyOutput> {
    let body = fs::read(&args.body_file).with_context(|| {
        format!("reading webhook body from `{}`", args.body_file.display())
    })?;
    let secret = std::env::var(&args.secret_env).with_context(|| {
        format!(
            "webhook verification requires `{}` env var to hold the manifest's webhook secret",
            args.secret_env
        )
    })?;

    if let Some(provider_slug) = args.provider.as_deref() {
        let provider = WebhookProvider::from_slug(provider_slug).ok_or_else(|| {
            anyhow!(
                "unknown webhook provider `{provider_slug}`; expected github|slack|linear"
            )
        })?;
        let mut headers = std::collections::BTreeMap::<String, String>::new();
        for (k, v) in &args.headers {
            headers.insert(k.clone(), v.clone());
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let inputs = WebhookVerifyInputs::new(
            provider,
            &headers,
            &body,
            secret.as_bytes(),
            now_ms,
        );
        let outcome = verify_webhook(inputs);
        return Ok(WebhookVerifyOutput {
            valid: outcome.is_verified(),
            algorithm: "hmac-sha256".to_string(),
            outcome: outcome_label(&outcome),
        });
    }

    // Provider-agnostic fallback: raw HMAC-SHA256 with the legacy
    // `sha256=` prefix tolerance.
    use hmac::{Hmac, Mac};
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes())
        .map_err(|e| anyhow!("hmac init: {e}"))?;
    mac.update(&body);
    let computed = mac.finalize().into_bytes();
    let computed_hex: String = computed.iter().map(|b| format!("{b:02x}")).collect();
    let expected_hex = args
        .signature
        .trim()
        .strip_prefix("sha256=")
        .unwrap_or(args.signature.trim());
    let valid = constant_time_eq(computed_hex.as_bytes(), expected_hex.as_bytes());
    Ok(WebhookVerifyOutput {
        valid,
        algorithm: "hmac-sha256".to_string(),
        outcome: if valid {
            "verified".to_string()
        } else {
            "bad_signature".to_string()
        },
    })
}

fn outcome_label(outcome: &WebhookVerificationOutcome) -> String {
    match outcome {
        WebhookVerificationOutcome::Verified => "verified".to_string(),
        WebhookVerificationOutcome::BadSignature => "bad_signature".to_string(),
        WebhookVerificationOutcome::Stale { delta_ms, .. } => {
            format!("stale (delta_ms={delta_ms})")
        }
        WebhookVerificationOutcome::Malformed { reason } => {
            format!("malformed ({reason})")
        }
    }
}

#[derive(Debug, Clone)]
pub struct WebhookVerifyArgs {
    pub signature: String,
    pub secret_env: String,
    pub body_file: PathBuf,
    pub provider: Option<String>,
    pub headers: Vec<(String, String)>,
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn shipped_manifests() -> Result<Vec<(&'static str, ConnectorManifest)>> {
    Ok(vec![
        ("calendar", calendar_manifest()?),
        ("files", file_manifest()?),
        ("gmail", gmail_manifest()?),
        ("ms365", ms365_manifest()?),
        ("slack", slack_manifest()?),
        ("tasks", task_manifest()?),
    ])
}

fn shipped_connector_names() -> Vec<&'static str> {
    vec!["calendar", "files", "gmail", "ms365", "slack", "tasks"]
}

fn summarise_manifest(manifest: &ConnectorManifest) -> ConnectorListEntry {
    let modes: Vec<String> = manifest
        .mode
        .iter()
        .map(|m| match m {
            corvid_connector_runtime::ConnectorMode::Mock => "mock".to_string(),
            corvid_connector_runtime::ConnectorMode::Replay => "replay".to_string(),
            corvid_connector_runtime::ConnectorMode::Real => "real".to_string(),
        })
        .collect();
    let write_scopes: Vec<String> = manifest
        .scope
        .iter()
        .filter(|s| {
            s.effects
                .iter()
                .any(|e| e.contains(".write") || e.starts_with("send_"))
        })
        .map(|s| s.id.clone())
        .collect();
    let rate_limit_summary = if manifest.rate_limit.is_empty() {
        "none".to_string()
    } else {
        manifest
            .rate_limit
            .iter()
            .map(|rl| format!("{}={}/{}ms", rl.key, rl.limit, rl.window_ms))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let _: BTreeSet<()> = BTreeSet::new();
    ConnectorListEntry {
        name: manifest.name.clone(),
        provider: manifest.provider.clone(),
        modes,
        scope_count: manifest.scope.len(),
        write_scopes,
        rate_limit_summary,
        redaction_count: manifest.redaction.len(),
    }
}

fn map_runtime_error(err: ConnectorRuntimeError) -> anyhow::Error {
    anyhow!("{err}")
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

fn random_b64url_bytes(n: usize) -> String {
    use base64::Engine;
    // Use a deterministic-but-unpredictable per-call source: the
    // process's nanoseconds + a hash of a fresh allocation address.
    // Production use should plumb an OS-supplied RNG; this keeps the
    // CLI command self-contained and doesn't pull in `rand`.
    let mut seed = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)) as u64;
    let addr_seed = (&seed as *const _ as usize) as u64;
    seed = seed.wrapping_add(addr_seed);
    let mut bytes = Vec::with_capacity(n);
    for _ in 0..n {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        bytes.push((seed >> 33) as u8);
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes)
}

fn pkce_code_challenge(verifier: &str) -> String {
    use base64::Engine;
    use sha2::Digest;
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Slice 41L: `corvid connectors list` returns one entry per
    /// shipped manifest with its mode set, write-scope list, and
    /// rate-limit summary.
    #[test]
    fn list_returns_every_shipped_connector() {
        let entries = run_list().expect("list");
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        // Manifest names are the public source of truth; ms365's
        // manifest spells itself `microsoft365` while the
        // shipped_manifests dictionary keys it as `ms365`.
        for required in ["gmail", "slack", "calendar", "files"] {
            assert!(names.contains(&required), "missing {required}: {names:?}");
        }
        // Tasks manifest uses "linear_github_tasks" as its name; we
        // assert at least one tasks-shaped entry exists.
        assert!(
            names.iter().any(|n| n.contains("task") || n.contains("linear")
                || n.contains("github")),
            "missing tasks-shaped connector: {names:?}",
        );
        // Microsoft 365 manifest names itself "microsoft365".
        assert!(
            names.iter().any(|n| n.contains("365")),
            "missing ms365-shaped connector: {names:?}",
        );
        let gmail = entries.iter().find(|e| e.name == "gmail").unwrap();
        assert!(gmail.modes.contains(&"real".to_string()));
        assert!(gmail.scope_count > 0);
        assert!(gmail.write_scopes.iter().any(|s| s.contains("send")));
    }

    /// Slice 41L: `corvid connectors check` flags every shipped
    /// manifest as valid (manifests are static and CI-tested
    /// elsewhere). With `--live` the command refuses without
    /// `CORVID_PROVIDER_LIVE=1`.
    #[test]
    fn check_passes_for_shipped_manifests() {
        let entries = run_check(false).expect("check");
        for entry in &entries {
            assert!(entry.valid, "{}: {:?}", entry.name, entry.diagnostics);
        }
    }

    /// Slice 41L: `connectors run --mode=mock` injects the
    /// supplied mock payload and returns it through the runtime.
    #[test]
    fn run_mock_mode_returns_inserted_payload() {
        let mock = serde_json::json!({"messages": [{"id": "m1"}]});
        let args = ConnectorRunArgs {
            connector: "gmail".to_string(),
            operation: "search".to_string(),
            scope_id: "gmail.search".to_string(),
            mode: "mock".to_string(),
            payload: Some(serde_json::json!({"user_id": "u", "query": "q", "max_results": 1})),
            mock_payload: Some(mock.clone()),
            approval_id: String::new(),
            replay_key: "rk".to_string(),
            tenant_id: "t".to_string(),
            actor_id: "a".to_string(),
            token_id: "tok".to_string(),
            now_ms: 1,
        };
        let output = run_run(args).expect("run");
        assert_eq!(output.payload, mock);
        assert_eq!(output.mode, "mock");
    }

    /// Slice 41L: `connectors run --mode=real` refuses without
    /// `CORVID_PROVIDER_LIVE=1`, preserving the runtime's default
    /// posture against accidental live calls.
    #[test]
    fn run_real_mode_refuses_without_provider_live() {
        // Ensure CORVID_PROVIDER_LIVE is unset for this test.
        std::env::remove_var("CORVID_PROVIDER_LIVE");
        let args = ConnectorRunArgs {
            connector: "tasks".to_string(),
            operation: "github_search".to_string(),
            scope_id: "tasks.github_search".to_string(),
            mode: "real".to_string(),
            payload: None,
            mock_payload: None,
            approval_id: String::new(),
            replay_key: "rk".to_string(),
            tenant_id: "t".to_string(),
            actor_id: "a".to_string(),
            token_id: "tok".to_string(),
            now_ms: 1,
        };
        let err = run_run(args).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("CORVID_PROVIDER_LIVE"), "{msg}");
    }

    /// Slice 41L: `connectors oauth init gmail` produces a valid
    /// authorization URL with PKCE parameters and a freshly
    /// generated state + code_verifier.
    #[test]
    fn oauth_init_gmail_emits_pkce_authorization_url() {
        let args = OauthInitArgs {
            provider: "gmail".to_string(),
            client_id: "client-1.apps.googleusercontent.com".to_string(),
            redirect_uri: Some("http://localhost:8765/cb".to_string()),
            scopes: vec![],
        };
        let output = run_oauth_init(args).expect("init");
        assert_eq!(output.provider, "gmail");
        assert!(output
            .authorization_url
            .starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
        assert!(output.authorization_url.contains("response_type=code"));
        assert!(output.authorization_url.contains("code_challenge_method=S256"));
        assert!(output
            .authorization_url
            .contains(&format!("state={}", output.state)));
        // PKCE verifier and challenge are non-empty and distinct.
        assert!(!output.code_verifier.is_empty());
        assert!(!output.code_challenge.is_empty());
        assert_ne!(output.code_verifier, output.code_challenge);
    }

    /// Slice 41L: `connectors oauth init slack` uses Slack's
    /// authorize endpoint and Slack's default scopes.
    #[test]
    fn oauth_init_slack_uses_slack_endpoint() {
        let args = OauthInitArgs {
            provider: "slack".to_string(),
            client_id: "slack-app".to_string(),
            redirect_uri: None,
            scopes: vec![],
        };
        let output = run_oauth_init(args).expect("init");
        assert!(output
            .authorization_url
            .starts_with("https://slack.com/oauth/v2/authorize?"));
        // Slack default scopes
        assert!(output.authorization_url.contains("channels"));
    }

    /// Slice 41L adversarial: an unknown provider yields a clear
    /// diagnostic, not a silent default.
    #[test]
    fn oauth_init_unknown_provider_refused() {
        let args = OauthInitArgs {
            provider: "discord".to_string(),
            client_id: "x".to_string(),
            redirect_uri: None,
            scopes: vec![],
        };
        let err = run_oauth_init(args).unwrap_err();
        assert!(err.to_string().contains("unknown provider"));
    }

    /// Slice 41L: `verify-webhook` validates a body against an
    /// HMAC-SHA256 signature computed with the given secret. The
    /// canonical happy path: the operator computes the signature
    /// the same way Slack/GitHub/Linear do, supplies the signature
    /// + the secret env var + the body file, and the command
    /// returns `valid=true`.
    #[test]
    fn webhook_verify_accepts_correct_signature() {
        let temp = tempfile::tempdir().unwrap();
        let body_path = temp.path().join("payload.json");
        let body = b"{\"event\":\"push\"}";
        std::fs::write(&body_path, body).unwrap();
        let secret = "shhhh";

        // Compute the expected signature with a fresh hmac.
        use hmac::{Hmac, Mac};
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let expected: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        std::env::set_var("CORVID_TEST_WEBHOOK_SECRET", secret);
        let output = run_verify_webhook(WebhookVerifyArgs {
            signature: format!("sha256={expected}"),
            secret_env: "CORVID_TEST_WEBHOOK_SECRET".to_string(),
            body_file: body_path,
            provider: None,
            headers: Vec::new(),
        })
        .expect("verify");
        std::env::remove_var("CORVID_TEST_WEBHOOK_SECRET");
        assert!(output.valid);
        assert_eq!(output.algorithm, "hmac-sha256");
        assert_eq!(output.outcome, "verified");
    }

    /// Slice 41L adversarial: a tampered body fails verification.
    /// The constant-time compare path makes the rejection
    /// unconditional rather than offering a length-leakage hint.
    #[test]
    fn webhook_verify_rejects_tampered_body() {
        let temp = tempfile::tempdir().unwrap();
        let body_path = temp.path().join("payload.json");
        let body = b"{\"event\":\"tampered\"}";
        std::fs::write(&body_path, body).unwrap();
        let secret = "shhhh";

        // Compute the signature for a DIFFERENT body so the verifier
        // sees a tampered body vs the supplied (genuine) signature.
        use hmac::{Hmac, Mac};
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(b"{\"event\":\"original\"}");
        let original_sig: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        std::env::set_var("CORVID_TEST_WEBHOOK_SECRET_2", secret);
        let output = run_verify_webhook(WebhookVerifyArgs {
            signature: original_sig,
            secret_env: "CORVID_TEST_WEBHOOK_SECRET_2".to_string(),
            body_file: body_path,
            provider: None,
            headers: Vec::new(),
        })
        .expect("verify runs");
        std::env::remove_var("CORVID_TEST_WEBHOOK_SECRET_2");
        assert!(!output.valid);
        assert_eq!(output.outcome, "bad_signature");
    }

    /// Slice 41M-A: provider-aware GitHub path uses the
    /// `X-Hub-Signature-256` header from `--header` rather than the
    /// generic `--signature` value.
    #[test]
    fn webhook_verify_dispatches_to_github_provider() {
        use hmac::{Hmac, Mac};

        let temp = tempfile::tempdir().unwrap();
        let body_path = temp.path().join("payload.json");
        let body = b"{\"event\":\"push\"}";
        std::fs::write(&body_path, body).unwrap();
        let secret = "github-secret";

        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let expected: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        std::env::set_var("CORVID_TEST_GITHUB_WEBHOOK", secret);
        let output = run_verify_webhook(WebhookVerifyArgs {
            signature: String::new(),
            secret_env: "CORVID_TEST_GITHUB_WEBHOOK".to_string(),
            body_file: body_path,
            provider: Some("github".to_string()),
            headers: vec![(
                "X-Hub-Signature-256".to_string(),
                format!("sha256={expected}"),
            )],
        })
        .expect("verify");
        std::env::remove_var("CORVID_TEST_GITHUB_WEBHOOK");
        assert!(output.valid);
        assert_eq!(output.outcome, "verified");
    }

    /// Slice 41M-A: an unknown provider yields a clear diagnostic.
    #[test]
    fn webhook_verify_unknown_provider_refused() {
        let temp = tempfile::tempdir().unwrap();
        let body_path = temp.path().join("payload.json");
        std::fs::write(&body_path, b"x").unwrap();
        std::env::set_var("CORVID_TEST_UNKNOWN_PROVIDER", "secret");
        let err = run_verify_webhook(WebhookVerifyArgs {
            signature: String::new(),
            secret_env: "CORVID_TEST_UNKNOWN_PROVIDER".to_string(),
            body_file: body_path,
            provider: Some("discord".to_string()),
            headers: Vec::new(),
        })
        .unwrap_err();
        std::env::remove_var("CORVID_TEST_UNKNOWN_PROVIDER");
        assert!(err.to_string().contains("unknown webhook provider"));
    }

    /// Slice 41L adversarial: `connectors run` with an unknown
    /// connector name yields a clear "one of <list>" diagnostic.
    #[test]
    fn run_unknown_connector_lists_known() {
        let args = ConnectorRunArgs {
            connector: "discord".to_string(),
            operation: "send".to_string(),
            scope_id: "discord.send".to_string(),
            mode: "mock".to_string(),
            payload: None,
            mock_payload: None,
            approval_id: String::new(),
            replay_key: "rk".to_string(),
            tenant_id: "t".to_string(),
            actor_id: "a".to_string(),
            token_id: "tok".to_string(),
            now_ms: 1,
        };
        let err = run_run(args).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown connector"), "{msg}");
        assert!(msg.contains("gmail"), "{msg}");
    }
}
