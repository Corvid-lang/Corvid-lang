//! `corvid connectors verify-webhook` — standalone webhook
//! signature verifier for inbound payloads.
//!
//! With `--provider github|slack|linear`, dispatches to the
//! per-provider verifier in `corvid-connector-runtime::webhook_verify`
//! (slice 41M-A) which knows the provider-specific header
//! conventions (Slack's `v0:<ts>:<body>` basestring with replay
//! protection, GitHub's `sha256=` prefix, Linear's no-prefix hex).
//!
//! Without a provider, falls back to a raw HMAC-SHA256 verifier
//! suitable for custom webhook integrations whose signing scheme
//! is "secret + body → hex digest" with optional `sha256=` prefix.

use anyhow::{anyhow, Context, Result};
use corvid_connector_runtime::{
    verify_webhook, WebhookProvider, WebhookVerificationOutcome, WebhookVerifyInputs,
};
use sha2::Sha256;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct WebhookVerifyOutput {
    pub valid: bool,
    pub algorithm: String,
    pub outcome: String,
}

#[derive(Debug, Clone)]
pub struct WebhookVerifyArgs {
    pub signature: String,
    pub secret_env: String,
    pub body_file: PathBuf,
    pub provider: Option<String>,
    pub headers: Vec<(String, String)>,
}

/// Verify an inbound webhook signature. With `--provider` set,
/// dispatches to the per-provider verifier; without, falls back to
/// raw HMAC-SHA256 verification.
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
