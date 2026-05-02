//! Approval flow.
//!
//! When the interpreter encounters a dangerous tool call after an `approve`
//! statement, it asks the runtime's `Approver` whether the action is
//! permitted. Two built-in approvers ship:
//!
//! * `StdinApprover` — interactive, the default for `corvid run`.
//! * `ProgrammaticApprover` — wraps a closure, used by tests, CI, and
//!   embedding hosts that want to plug their own auth system in.
//!
//! Programs select an approver via `Runtime::set_approver`. There is no
//! "default approve all" here — that would weaken the safety
//! story. Tests that need it construct `ProgrammaticApprover::always_yes`
//! explicitly so the intent is on the page.

use crate::errors::RuntimeError;
use futures::future::BoxFuture;
use std::sync::atomic::{AtomicU64, Ordering};

mod approver_impls;
mod card;
mod token;
pub use approver_impls::{ProgrammaticApprover, StdinApprover};
pub use card::{ApprovalCard, ApprovalCardArgument, ApprovalRisk};
pub use token::{ApprovalToken, ApprovalTokenScope};

pub(super) static BENCH_APPROVAL_WAIT_NS: AtomicU64 = AtomicU64::new(0);

pub fn bench_approval_wait_ns() -> u64 {
    BENCH_APPROVAL_WAIT_NS.load(Ordering::Relaxed)
}

fn profile_enabled() -> bool {
    std::env::var("CORVID_PROFILE_EVENTS").ok().as_deref() == Some("1")
}

pub(super) fn emit_wait_profile(kind: &str, name: &str, nominal_ms: u64, actual_ms: f64) {
    if !profile_enabled() {
        return;
    }
    let event = serde_json::json!({
        "kind": "wait",
        "source_kind": kind,
        "name": name,
        "nominal_ms": nominal_ms,
        "actual_ms": actual_ms,
    });
    eprintln!("CORVID_PROFILE_JSON={event}");
}


/// What the runtime asks the approver to authorize.
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    /// Label from the `approve Label(args)` statement.
    pub label: String,
    /// Args from the `approve` statement, marshalled to JSON.
    pub args: Vec<serde_json::Value>,
}

impl ApprovalRequest {
    pub fn card(&self) -> ApprovalCard {
        ApprovalCard::from_request(self)
    }
}





/// The approver's decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approve,
    Deny,
}

/// Trait every approver implements.
pub trait Approver: Send + Sync {
    fn approve<'a>(
        &'a self,
        req: &'a ApprovalRequest,
    ) -> BoxFuture<'a, Result<ApprovalDecision, RuntimeError>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn req() -> ApprovalRequest {
        ApprovalRequest {
            label: "IssueRefund".into(),
            args: vec![json!("ord_1"), json!(99.99)],
        }
    }

    #[tokio::test]
    async fn always_yes_approves() {
        let a = ProgrammaticApprover::always_yes();
        let r = req();
        assert_eq!(a.approve(&r).await.unwrap(), ApprovalDecision::Approve);
    }

    #[tokio::test]
    async fn always_no_denies() {
        let a = ProgrammaticApprover::always_no();
        let r = req();
        assert_eq!(a.approve(&r).await.unwrap(), ApprovalDecision::Deny);
    }

    #[tokio::test]
    async fn predicate_can_inspect_request() {
        let a = ProgrammaticApprover::new(|r| {
            if r.label == "IssueRefund" {
                ApprovalDecision::Approve
            } else {
                ApprovalDecision::Deny
            }
        });
        assert_eq!(a.approve(&req()).await.unwrap(), ApprovalDecision::Approve);
    }

    #[test]
    fn approval_card_humanizes_risk_and_redacts_sensitive_values() {
        let req = ApprovalRequest {
            label: "ChargeCard".into(),
            args: vec![json!("4242424242424242"), json!(125.00)],
        };

        let card = req.card();

        assert_eq!(card.title, "Charge Card");
        assert_eq!(card.risk, ApprovalRisk::MoneyMovement);
        assert_eq!(card.arguments[0].value, json!("<redacted>"));
        assert!(card.arguments[0].redacted);
        let rendered = card.render_text();
        assert!(rendered.contains("corvid approval card"));
        assert!(rendered.contains("why: program requested approval"));
        assert!(rendered.contains("preview:"));
        assert!(rendered.contains("money-moving operation"));
        assert!(rendered.contains("arg 0 [string] (redacted) = \"<redacted>\""));
        let html = card.render_html();
        assert!(html.contains("<!doctype html>"));
        assert!(html.contains("Charge Card"));
        assert!(html.contains("&lt;redacted&gt;"));
    }

    #[test]
    fn approval_token_scopes_fail_closed() {
        let mut token = ApprovalToken {
            token_id: "apr_test".into(),
            label: "ChargeCard".into(),
            args: vec![json!("ord_1"), json!(100.0)],
            scope: ApprovalTokenScope::AmountLimited { max_amount: 100.0 },
            issued_at_ms: 10,
            expires_at_ms: 1000,
            uses_remaining: 1,
        };
        assert!(token
            .validate("ChargeCard", &[json!("ord_1"), json!(101.0)], 20, None)
            .is_err());
        assert_eq!(token.uses_remaining, 1);

        token
            .validate("ChargeCard", &[json!("ord_1"), json!(100.0)], 20, None)
            .unwrap();
        assert_eq!(token.uses_remaining, 0);
        assert!(token
            .validate("ChargeCard", &[json!("ord_1"), json!(100.0)], 20, None)
            .is_err());
    }

    #[test]
    fn approval_token_session_time_and_argument_scopes_are_enforced() {
        let mut session_token = ApprovalToken {
            token_id: "apr_session".into(),
            label: "SendEmail".into(),
            args: vec![json!("user@example.com")],
            scope: ApprovalTokenScope::Session {
                session_id: "s-1".into(),
            },
            issued_at_ms: 10,
            expires_at_ms: 1000,
            uses_remaining: 1,
        };
        assert!(session_token
            .validate("SendEmail", &[json!("user@example.com")], 20, Some("s-2"))
            .is_err());

        let mut argument_token = ApprovalToken {
            token_id: "apr_args".into(),
            label: "SendEmail".into(),
            args: vec![json!("user@example.com")],
            scope: ApprovalTokenScope::ArgumentBound {
                args: vec![json!("user@example.com")],
            },
            issued_at_ms: 10,
            expires_at_ms: 1000,
            uses_remaining: 1,
        };
        assert!(argument_token
            .validate("SendEmail", &[json!("other@example.com")], 20, None)
            .is_err());

        let mut time_token = ApprovalToken {
            token_id: "apr_time".into(),
            label: "SendEmail".into(),
            args: vec![json!("user@example.com")],
            scope: ApprovalTokenScope::TimeLimited { expires_at_ms: 30 },
            issued_at_ms: 10,
            expires_at_ms: 1000,
            uses_remaining: 1,
        };
        assert!(time_token
            .validate("SendEmail", &[json!("user@example.com")], 31, None)
            .is_err());
    }
}
