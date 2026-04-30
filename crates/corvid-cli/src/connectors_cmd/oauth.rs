//! `corvid connectors oauth init|rotate` — OAuth2 lifecycle.
//!
//! `init` constructs a provider authorization URL with PKCE
//! parameters (state, code verifier, code challenge); the operator
//! opens the URL in a browser, completes the consent dance, and
//! the redirect handler exchanges the returned code for tokens.
//!
//! `rotate` force-refreshes an existing `(access, refresh)` pair
//! against the provider's token endpoint and emits the new pair so
//! the operator can persist it. Production deployments use the
//! encrypted token store; this CLI surface stays dev-friendly by
//! reading current tokens out of CLI args.

use anyhow::{anyhow, Result};
use corvid_connector_runtime::{
    BearerTokenResolver, InMemoryOAuth2Store, OAuth2RefreshResolver, OAuth2Tokens,
    ReqwestRefreshHook,
};
use std::sync::Arc;

use super::support::{pkce_code_challenge, random_b64url_bytes, url_encode};

#[derive(Debug, Clone, PartialEq)]
pub struct OauthInitOutput {
    pub provider: String,
    pub state: String,
    pub authorization_url: String,
    pub code_verifier: String,
    pub code_challenge: String,
}

#[derive(Debug, Clone)]
pub struct OauthInitArgs {
    pub provider: String,
    pub client_id: String,
    pub redirect_uri: Option<String>,
    pub scopes: Vec<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
