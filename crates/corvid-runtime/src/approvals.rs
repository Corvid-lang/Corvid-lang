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
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

static BENCH_APPROVAL_WAIT_NS: AtomicU64 = AtomicU64::new(0);

pub fn bench_approval_wait_ns() -> u64 {
    BENCH_APPROVAL_WAIT_NS.load(Ordering::Relaxed)
}

fn profile_enabled() -> bool {
    std::env::var("CORVID_PROFILE_EVENTS").ok().as_deref() == Some("1")
}

fn emit_wait_profile(kind: &str, name: &str, nominal_ms: u64, actual_ms: f64) {
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

fn approval_latency_queue() -> &'static Mutex<VecDeque<u64>> {
    static LATENCIES: OnceLock<Mutex<VecDeque<u64>>> = OnceLock::new();
    LATENCIES.get_or_init(|| {
        let queue = std::env::var("CORVID_BENCH_APPROVAL_LATENCIES_MS")
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .map(|value| match value {
                serde_json::Value::Array(values) => values
                    .into_iter()
                    .filter_map(|v| v.as_u64())
                    .collect::<VecDeque<_>>(),
                other => other
                    .as_u64()
                    .map(|v| VecDeque::from([v]))
                    .unwrap_or_default(),
            })
            .unwrap_or_default();
        Mutex::new(queue)
    })
}

fn maybe_sleep_programmatic_approval(label: &str) {
    let latency_ms = {
        let mut queue = approval_latency_queue().lock().unwrap();
        queue.pop_front().unwrap_or(0)
    };
    if latency_ms == 0 {
        return;
    }
    let start = Instant::now();
    std::thread::sleep(Duration::from_millis(latency_ms));
    let actual_ms = start.elapsed().as_secs_f64() * 1000.0;
    BENCH_APPROVAL_WAIT_NS.fetch_add((actual_ms * 1_000_000.0) as u64, Ordering::Relaxed);
    emit_wait_profile("approval", label, latency_ms, actual_ms);
}

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
        maybe_sleep_programmatic_approval(&req.label);
        let decision = (self.decide)(req);
        Box::pin(async move { Ok(decision) })
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
