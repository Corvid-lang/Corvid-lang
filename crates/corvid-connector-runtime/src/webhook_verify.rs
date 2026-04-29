//! Per-provider webhook signature verifier — slice 41M-A.
//!
//! Inbound webhook payloads from third-party providers carry their
//! signature in a provider-specific header with a provider-specific
//! signing scheme. This module ships verifiers for the three
//! signing schemes Corvid's shipped connectors care about:
//!
//!   - **GitHub** (`X-Hub-Signature-256`): HMAC-SHA256 over the raw
//!     body, hex-encoded, prefixed with `sha256=`.
//!   - **Slack** (`X-Slack-Signature` + `X-Slack-Request-Timestamp`):
//!     HMAC-SHA256 over `v0:<timestamp>:<body>`, hex-encoded,
//!     prefixed with `v0=`. Includes replay protection — a
//!     timestamp older than 5 minutes is refused even if the
//!     signature matches.
//!   - **Linear** (`Linear-Signature`): HMAC-SHA256 over the raw
//!     body, hex-encoded, no prefix.
//!
//! Each verifier returns a structured outcome (`Verified` /
//! `BadSignature` / `Stale` / `Malformed`) so the caller can
//! produce the right HTTP response code and the right registry
//! row in the trace. All comparisons are constant-time.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookProvider {
    GitHub,
    Slack,
    Linear,
}

impl WebhookProvider {
    pub fn slug(&self) -> &'static str {
        match self {
            WebhookProvider::GitHub => "github",
            WebhookProvider::Slack => "slack",
            WebhookProvider::Linear => "linear",
        }
    }

    pub fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "github" => Some(WebhookProvider::GitHub),
            "slack" => Some(WebhookProvider::Slack),
            "linear" => Some(WebhookProvider::Linear),
            _ => None,
        }
    }
}

/// Outcome of a webhook verification. The HTTP layer maps each
/// variant to a status code: `Verified` → 200, `BadSignature` →
/// 401, `Stale` → 401, `Malformed` → 400.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookVerificationOutcome {
    Verified,
    BadSignature,
    /// Slack-only: signature matched but the timestamp was outside
    /// the freshness window (default 5 minutes). Replay protection.
    Stale {
        provided_ts_ms: u64,
        now_ms: u64,
        delta_ms: u64,
    },
    /// Required header missing or in an unparseable shape.
    Malformed {
        reason: String,
    },
}

impl WebhookVerificationOutcome {
    pub fn is_verified(&self) -> bool {
        matches!(self, WebhookVerificationOutcome::Verified)
    }
}

/// Inputs to a webhook verification. Headers are case-insensitive
/// per RFC 9110 — keys are normalised to lowercase before lookup.
#[derive(Debug, Clone)]
pub struct WebhookVerifyInputs<'a> {
    pub provider: WebhookProvider,
    pub headers: &'a BTreeMap<String, String>,
    pub body: &'a [u8],
    pub secret: &'a [u8],
    pub now_ms: u64,
    /// Slack-only: maximum allowed age between the
    /// `X-Slack-Request-Timestamp` value and `now_ms`. Default 5
    /// minutes (300_000 ms).
    pub max_age_ms: u64,
}

impl<'a> WebhookVerifyInputs<'a> {
    pub fn new(
        provider: WebhookProvider,
        headers: &'a BTreeMap<String, String>,
        body: &'a [u8],
        secret: &'a [u8],
        now_ms: u64,
    ) -> Self {
        Self {
            provider,
            headers,
            body,
            secret,
            now_ms,
            max_age_ms: 5 * 60 * 1_000,
        }
    }

    pub fn with_max_age_ms(mut self, max_age_ms: u64) -> Self {
        self.max_age_ms = max_age_ms;
        self
    }
}

/// Top-level entrypoint: dispatch by provider.
pub fn verify_webhook(inputs: WebhookVerifyInputs<'_>) -> WebhookVerificationOutcome {
    match inputs.provider {
        WebhookProvider::GitHub => verify_github(&inputs),
        WebhookProvider::Slack => verify_slack(&inputs),
        WebhookProvider::Linear => verify_linear(&inputs),
    }
}

fn header_lower<'a>(headers: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    let needle = name.to_ascii_lowercase();
    for (key, value) in headers {
        if key.to_ascii_lowercase() == needle {
            return Some(value.as_str());
        }
    }
    None
}

fn verify_github(inputs: &WebhookVerifyInputs<'_>) -> WebhookVerificationOutcome {
    let header = match header_lower(inputs.headers, "x-hub-signature-256") {
        Some(h) => h,
        None => {
            return WebhookVerificationOutcome::Malformed {
                reason: "missing X-Hub-Signature-256 header".to_string(),
            };
        }
    };
    let expected_hex = match header.strip_prefix("sha256=") {
        Some(rest) => rest,
        None => {
            return WebhookVerificationOutcome::Malformed {
                reason: "X-Hub-Signature-256 missing `sha256=` prefix".to_string(),
            };
        }
    };
    if hmac_sha256_hex_eq(inputs.secret, inputs.body, expected_hex) {
        WebhookVerificationOutcome::Verified
    } else {
        WebhookVerificationOutcome::BadSignature
    }
}

fn verify_linear(inputs: &WebhookVerifyInputs<'_>) -> WebhookVerificationOutcome {
    let header = match header_lower(inputs.headers, "linear-signature") {
        Some(h) => h,
        None => {
            return WebhookVerificationOutcome::Malformed {
                reason: "missing Linear-Signature header".to_string(),
            };
        }
    };
    if hmac_sha256_hex_eq(inputs.secret, inputs.body, header.trim()) {
        WebhookVerificationOutcome::Verified
    } else {
        WebhookVerificationOutcome::BadSignature
    }
}

fn verify_slack(inputs: &WebhookVerifyInputs<'_>) -> WebhookVerificationOutcome {
    let signature = match header_lower(inputs.headers, "x-slack-signature") {
        Some(h) => h,
        None => {
            return WebhookVerificationOutcome::Malformed {
                reason: "missing X-Slack-Signature header".to_string(),
            };
        }
    };
    let timestamp = match header_lower(inputs.headers, "x-slack-request-timestamp") {
        Some(h) => h,
        None => {
            return WebhookVerificationOutcome::Malformed {
                reason: "missing X-Slack-Request-Timestamp header".to_string(),
            };
        }
    };
    // Slack uses Unix timestamps in seconds in the header.
    let provided_ts_s: u64 = match timestamp.trim().parse() {
        Ok(v) => v,
        Err(_) => {
            return WebhookVerificationOutcome::Malformed {
                reason: format!("X-Slack-Request-Timestamp `{timestamp}` is not a u64"),
            };
        }
    };
    let provided_ts_ms = provided_ts_s.saturating_mul(1_000);
    // Replay protection: reject if the timestamp is more than
    // `max_age_ms` away from `now_ms` in either direction.
    let delta_ms = if inputs.now_ms >= provided_ts_ms {
        inputs.now_ms - provided_ts_ms
    } else {
        provided_ts_ms - inputs.now_ms
    };
    if delta_ms > inputs.max_age_ms {
        return WebhookVerificationOutcome::Stale {
            provided_ts_ms,
            now_ms: inputs.now_ms,
            delta_ms,
        };
    }
    let expected_hex = match signature.strip_prefix("v0=") {
        Some(rest) => rest,
        None => {
            return WebhookVerificationOutcome::Malformed {
                reason: "X-Slack-Signature missing `v0=` prefix".to_string(),
            };
        }
    };
    // Slack signs `v0:<timestamp>:<body>` (timestamp is the
    // header value, not a milliseconds rendering).
    let mut basestring = Vec::with_capacity(4 + timestamp.len() + 1 + inputs.body.len());
    basestring.extend_from_slice(b"v0:");
    basestring.extend_from_slice(timestamp.trim().as_bytes());
    basestring.push(b':');
    basestring.extend_from_slice(inputs.body);
    if hmac_sha256_hex_eq(inputs.secret, &basestring, expected_hex) {
        WebhookVerificationOutcome::Verified
    } else {
        WebhookVerificationOutcome::BadSignature
    }
}

fn hmac_sha256_hex_eq(secret: &[u8], body: &[u8], expected_hex: &str) -> bool {
    let computed = match hmac_sha256_hex(secret, body) {
        Some(c) => c,
        None => return false,
    };
    constant_time_eq(computed.as_bytes(), expected_hex.as_bytes())
}

fn hmac_sha256_hex(secret: &[u8], body: &[u8]) -> Option<String> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret).ok()?;
    mac.update(body);
    let bytes = mac.finalize().into_bytes();
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
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

    fn compute_github_signature(secret: &[u8], body: &[u8]) -> String {
        let hex = hmac_sha256_hex(secret, body).expect("hmac");
        format!("sha256={hex}")
    }

    fn compute_slack_signature(secret: &[u8], ts_s: u64, body: &[u8]) -> String {
        let mut basestring = Vec::new();
        basestring.extend_from_slice(b"v0:");
        basestring.extend_from_slice(ts_s.to_string().as_bytes());
        basestring.push(b':');
        basestring.extend_from_slice(body);
        let hex = hmac_sha256_hex(secret, &basestring).unwrap();
        format!("v0={hex}")
    }

    fn compute_linear_signature(secret: &[u8], body: &[u8]) -> String {
        hmac_sha256_hex(secret, body).unwrap()
    }

    /// Slice 41M-A: GitHub happy-path. Computed `sha256=...` matches.
    #[test]
    fn github_verifies_correct_signature() {
        let secret = b"shhh";
        let body = b"{\"event\":\"push\"}";
        let sig = compute_github_signature(secret, body);
        let mut headers = BTreeMap::new();
        headers.insert("X-Hub-Signature-256".to_string(), sig);
        let inputs =
            WebhookVerifyInputs::new(WebhookProvider::GitHub, &headers, body, secret, 0);
        assert!(verify_webhook(inputs).is_verified());
    }

    /// Slice 41M-A adversarial: GitHub forgery — a single-byte body
    /// flip with the original signature must fail.
    #[test]
    fn github_rejects_tampered_body() {
        let secret = b"shhh";
        let body = b"{\"event\":\"push\"}";
        let sig = compute_github_signature(secret, body);
        let mut headers = BTreeMap::new();
        headers.insert("X-Hub-Signature-256".to_string(), sig);
        let tampered = b"{\"event\":\"PUSH\"}";
        let inputs =
            WebhookVerifyInputs::new(WebhookProvider::GitHub, &headers, tampered, secret, 0);
        assert_eq!(verify_webhook(inputs), WebhookVerificationOutcome::BadSignature);
    }

    /// Slice 41M-A adversarial: missing header → Malformed.
    #[test]
    fn github_missing_header_is_malformed() {
        let headers = BTreeMap::new();
        let inputs = WebhookVerifyInputs::new(
            WebhookProvider::GitHub,
            &headers,
            b"body",
            b"secret",
            0,
        );
        assert!(matches!(
            verify_webhook(inputs),
            WebhookVerificationOutcome::Malformed { .. }
        ));
    }

    /// Slice 41M-A adversarial: header without the `sha256=` prefix
    /// is malformed (production GitHub always includes it).
    #[test]
    fn github_missing_sha256_prefix_is_malformed() {
        let mut headers = BTreeMap::new();
        headers.insert(
            "X-Hub-Signature-256".to_string(),
            "deadbeef".to_string(),
        );
        let inputs = WebhookVerifyInputs::new(
            WebhookProvider::GitHub,
            &headers,
            b"body",
            b"secret",
            0,
        );
        assert!(matches!(
            verify_webhook(inputs),
            WebhookVerificationOutcome::Malformed { .. }
        ));
    }

    /// Slice 41M-A: Slack happy-path with a fresh timestamp inside
    /// the 5-minute window.
    #[test]
    fn slack_verifies_correct_signature_inside_window() {
        let secret = b"slack-secret";
        let body = b"token=xyz&team_id=T01";
        let now_s: u64 = 1_700_000_000;
        let sig = compute_slack_signature(secret, now_s, body);
        let mut headers = BTreeMap::new();
        headers.insert("X-Slack-Signature".to_string(), sig);
        headers.insert("X-Slack-Request-Timestamp".to_string(), now_s.to_string());
        let inputs = WebhookVerifyInputs::new(
            WebhookProvider::Slack,
            &headers,
            body,
            secret,
            now_s.saturating_mul(1_000),
        );
        assert!(verify_webhook(inputs).is_verified());
    }

    /// Slice 41M-A adversarial: Slack replay — a signature that was
    /// valid 10 minutes ago is `Stale`, even though the HMAC still
    /// matches.
    #[test]
    fn slack_rejects_stale_timestamp_outside_window() {
        let secret = b"slack-secret";
        let body = b"old=event";
        let then_s: u64 = 1_700_000_000;
        let sig = compute_slack_signature(secret, then_s, body);
        let mut headers = BTreeMap::new();
        headers.insert("X-Slack-Signature".to_string(), sig);
        headers.insert("X-Slack-Request-Timestamp".to_string(), then_s.to_string());
        // 10 minutes later in ms.
        let now_ms = (then_s + 600).saturating_mul(1_000);
        let inputs =
            WebhookVerifyInputs::new(WebhookProvider::Slack, &headers, body, secret, now_ms);
        assert!(matches!(
            verify_webhook(inputs),
            WebhookVerificationOutcome::Stale { .. }
        ));
    }

    /// Slice 41M-A adversarial: Slack signature with the basestring
    /// `v0:<timestamp>:<body>` does not survive a body tamper.
    #[test]
    fn slack_rejects_tampered_body() {
        let secret = b"slack-secret";
        let body = b"original";
        let now_s: u64 = 1_700_000_000;
        let sig = compute_slack_signature(secret, now_s, body);
        let mut headers = BTreeMap::new();
        headers.insert("X-Slack-Signature".to_string(), sig);
        headers.insert("X-Slack-Request-Timestamp".to_string(), now_s.to_string());
        let inputs = WebhookVerifyInputs::new(
            WebhookProvider::Slack,
            &headers,
            b"tampered",
            secret,
            now_s.saturating_mul(1_000),
        );
        assert_eq!(verify_webhook(inputs), WebhookVerificationOutcome::BadSignature);
    }

    /// Slice 41M-A adversarial: Slack with a missing timestamp header
    /// is malformed — without the timestamp the basestring cannot
    /// be reconstructed and the signature is not even comparable.
    #[test]
    fn slack_missing_timestamp_is_malformed() {
        let mut headers = BTreeMap::new();
        headers.insert("X-Slack-Signature".to_string(), "v0=deadbeef".to_string());
        let inputs = WebhookVerifyInputs::new(
            WebhookProvider::Slack,
            &headers,
            b"body",
            b"secret",
            0,
        );
        assert!(matches!(
            verify_webhook(inputs),
            WebhookVerificationOutcome::Malformed { .. }
        ));
    }

    /// Slice 41M-A: Linear happy-path with the no-prefix hex
    /// signature.
    #[test]
    fn linear_verifies_correct_signature() {
        let secret = b"linear-secret";
        let body = b"{\"data\":{\"action\":\"create\"}}";
        let sig = compute_linear_signature(secret, body);
        let mut headers = BTreeMap::new();
        headers.insert("Linear-Signature".to_string(), sig);
        let inputs =
            WebhookVerifyInputs::new(WebhookProvider::Linear, &headers, body, secret, 0);
        assert!(verify_webhook(inputs).is_verified());
    }

    /// Slice 41M-A adversarial: Linear with the wrong secret fails.
    #[test]
    fn linear_rejects_wrong_secret() {
        let body = b"{\"data\":{\"action\":\"create\"}}";
        let sig = compute_linear_signature(b"linear-secret", body);
        let mut headers = BTreeMap::new();
        headers.insert("Linear-Signature".to_string(), sig);
        let inputs = WebhookVerifyInputs::new(
            WebhookProvider::Linear,
            &headers,
            body,
            b"different-secret",
            0,
        );
        assert_eq!(verify_webhook(inputs), WebhookVerificationOutcome::BadSignature);
    }

    /// Slice 41M-A: header lookup is case-insensitive per RFC 9110
    /// — providers vary in case (`X-Hub-Signature-256` vs
    /// `x-hub-signature-256`).
    #[test]
    fn case_insensitive_header_lookup() {
        let secret = b"shhh";
        let body = b"{\"event\":\"push\"}";
        let sig = compute_github_signature(secret, body);
        let mut headers = BTreeMap::new();
        headers.insert("x-hub-signature-256".to_string(), sig);
        let inputs =
            WebhookVerifyInputs::new(WebhookProvider::GitHub, &headers, body, secret, 0);
        assert!(verify_webhook(inputs).is_verified());
    }

    /// Slice 41M-A: provider slug round-trip is stable.
    #[test]
    fn provider_slug_round_trip() {
        for p in [
            WebhookProvider::GitHub,
            WebhookProvider::Slack,
            WebhookProvider::Linear,
        ] {
            let slug = p.slug();
            assert_eq!(WebhookProvider::from_slug(slug).unwrap(), p);
        }
        assert!(WebhookProvider::from_slug("discord").is_none());
    }
}
