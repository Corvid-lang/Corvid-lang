//! `corvid connectors run` — drive a connector operation against
//! a chosen mode.
//!
//! Mock and Replay use the in-memory mock dictionary; Real builds
//! the per-connector real client from environment-supplied
//! credentials and dispatches through reqwest. Real mode gates on
//! `CORVID_PROVIDER_LIVE=1` plus the relevant per-provider
//! credential env vars — the same posture `ConnectorRuntime`
//! enforces in code.

use anyhow::{anyhow, Context, Result};
use corvid_connector_runtime::{
    github_pat_real_client, gmail_real_client, slack_real_client, ConnectorAuthState,
    ConnectorRealClient, ConnectorRequest, ConnectorRuntime, ConnectorRuntimeMode,
    OAuth2Tokens,
};
use serde_json::Value;
use std::sync::Arc;

use super::support::{
    map_runtime_error, shipped_connector_names, shipped_manifests,
};

#[derive(Debug, Clone, PartialEq)]
pub struct ConnectorRunOutput {
    pub connector: String,
    pub operation: String,
    pub mode: String,
    pub payload: Value,
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

#[cfg(test)]
mod tests {
    use super::*;

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
