//! Approval-token state machine.
//!
//! `ApprovalToken` is the single-use receipt issued by the
//! approval gate when a request is granted. The next dangerous
//! call must present the token to the runtime; `validate`
//! enforces the scope (`OneTime`, `Session`, `AmountLimited`,
//! `TimeLimited`, `ArgumentBound`) and bumps the
//! `uses_remaining` counter on each successful validation.
//!
//! Tokens are inert structs across the wire — no embedded
//! cryptographic signing. The runtime trusts the in-process
//! issuer, so token forgery is bounded by Rust's memory safety
//! model. Cross-process token transfer would require a signed
//! envelope; that's filed as a future invention slice.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ApprovalTokenScope {
    OneTime,
    Session { session_id: String },
    AmountLimited { max_amount: f64 },
    TimeLimited { expires_at_ms: u64 },
    ArgumentBound { args: Vec<serde_json::Value> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalToken {
    pub token_id: String,
    pub label: String,
    pub args: Vec<serde_json::Value>,
    pub scope: ApprovalTokenScope,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    pub uses_remaining: u32,
}

impl ApprovalToken {
    pub fn validate(
        &mut self,
        label: &str,
        args: &[serde_json::Value],
        now_ms: u64,
        session_id: Option<&str>,
    ) -> Result<(), String> {
        if self.uses_remaining == 0 {
            return Err("token has no remaining uses".into());
        }
        if self.label != label {
            return Err(format!(
                "token label `{}` does not match requested label `{label}`",
                self.label
            ));
        }
        if now_ms > self.expires_at_ms {
            return Err("token expired".into());
        }
        match &self.scope {
            ApprovalTokenScope::OneTime => {}
            ApprovalTokenScope::Session {
                session_id: expected,
            } => {
                if session_id != Some(expected.as_str()) {
                    return Err("token session does not match current session".into());
                }
            }
            ApprovalTokenScope::AmountLimited { max_amount } => {
                let requested = largest_numeric_arg(args).ok_or_else(|| {
                    "amount-limited token requires at least one numeric argument".to_string()
                })?;
                if requested.abs() > *max_amount {
                    return Err(format!(
                        "requested amount {requested} exceeds token limit {max_amount}"
                    ));
                }
            }
            ApprovalTokenScope::TimeLimited {
                expires_at_ms: scoped_expires_at_ms,
            } => {
                if now_ms > *scoped_expires_at_ms {
                    return Err("token time-limited scope expired".into());
                }
            }
            ApprovalTokenScope::ArgumentBound {
                args: expected_args,
            } => {
                if expected_args != args {
                    return Err("token arguments do not match current arguments".into());
                }
            }
        }
        if matches!(self.scope, ApprovalTokenScope::ArgumentBound { .. }) {
            // Argument-bound scopes are still exact-match tokens.
        } else if self.args != args {
            return Err("token arguments do not match issued arguments".into());
        }
        self.uses_remaining = self.uses_remaining.saturating_sub(1);
        Ok(())
    }
}

fn largest_numeric_arg(args: &[serde_json::Value]) -> Option<f64> {
    args.iter()
        .filter_map(|value| value.as_f64())
        .max_by(|left, right| {
            left.abs()
                .partial_cmp(&right.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}
