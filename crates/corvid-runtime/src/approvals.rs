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
use std::io::Write;
use std::sync::Arc;

/// What the runtime asks the approver to authorize.
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    /// Label from the `approve Label(args)` statement.
    pub label: String,
    /// Args from the `approve` statement, marshalled to JSON.
    pub args: Vec<serde_json::Value>,
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

/// Stdin approver. Prints the request, reads a y/n line.
pub struct StdinApprover;

impl StdinApprover {
    pub fn new() -> Self {
        Self
    }
}

impl Default for StdinApprover {
    fn default() -> Self {
        Self::new()
    }
}

impl Approver for StdinApprover {
    fn approve<'a>(
        &'a self,
        req: &'a ApprovalRequest,
    ) -> BoxFuture<'a, Result<ApprovalDecision, RuntimeError>> {
        Box::pin(async move {
            // Run the blocking IO on a Tokio blocking thread so we don't
            // park the async runtime while waiting on the user.
            let label = req.label.clone();
            let args = req.args.clone();
            let decision = tokio::task::spawn_blocking(move || -> Result<ApprovalDecision, RuntimeError> {
                let stdout = std::io::stdout();
                let mut handle = stdout.lock();
                writeln!(handle, "─── corvid: approval requested ────────────────")
                    .map_err(|e| RuntimeError::ApprovalFailed(e.to_string()))?;
                writeln!(handle, "  action: {label}")
                    .map_err(|e| RuntimeError::ApprovalFailed(e.to_string()))?;
                for (i, a) in args.iter().enumerate() {
                    writeln!(handle, "  arg {i}: {a}")
                        .map_err(|e| RuntimeError::ApprovalFailed(e.to_string()))?;
                }
                write!(handle, "  approve? [y/N]: ")
                    .map_err(|e| RuntimeError::ApprovalFailed(e.to_string()))?;
                handle.flush().ok();

                let mut line = String::new();
                std::io::stdin()
                    .read_line(&mut line)
                    .map_err(|e| RuntimeError::ApprovalFailed(e.to_string()))?;
                let trimmed = line.trim().to_ascii_lowercase();
                Ok(if trimmed == "y" || trimmed == "yes" {
                    ApprovalDecision::Approve
                } else {
                    ApprovalDecision::Deny
                })
            })
            .await
            .map_err(|e| RuntimeError::ApprovalFailed(format!("approver task failed: {e}")))??;
            Ok(decision)
        })
    }
}

/// Programmatic approver. Wraps a synchronous closure for tests and
/// embedding hosts.
pub struct ProgrammaticApprover {
    decide: Arc<dyn Fn(&ApprovalRequest) -> ApprovalDecision + Send + Sync>,
}

impl ProgrammaticApprover {
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&ApprovalRequest) -> ApprovalDecision + Send + Sync + 'static,
    {
        Self {
            decide: Arc::new(f),
        }
    }

    /// Always-approve approver. For tests and CI.
    pub fn always_yes() -> Self {
        Self::new(|_| ApprovalDecision::Approve)
    }

    /// Always-deny approver. For tests.
    pub fn always_no() -> Self {
        Self::new(|_| ApprovalDecision::Deny)
    }
}

impl Approver for ProgrammaticApprover {
    fn approve<'a>(
        &'a self,
        req: &'a ApprovalRequest,
    ) -> BoxFuture<'a, Result<ApprovalDecision, RuntimeError>> {
        let decide = self.decide.clone();
        let req = req.clone();
        Box::pin(async move { Ok(decide(&req)) })
    }
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
}
