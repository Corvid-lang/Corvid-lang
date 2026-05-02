//! Built-in `Approver` trait implementations.
//!
//! Two approvers ship: `StdinApprover` (interactive, the default
//! for `corvid run`) and `ProgrammaticApprover` (closure-wrapped,
//! used by tests, CI, and embedding hosts that want to plug
//! their own auth system in).
//!
//! `maybe_sleep_programmatic_approval` is the test-only latency
//! injector — when `CORVID_BENCH_APPROVAL_LATENCIES_MS` is set,
//! the programmatic approver sleeps for the queued duration
//! before returning. This lets the bench harness measure
//! approval-wait overhead without driving an actual interactive
//! session.

use std::collections::VecDeque;
use std::io::Write;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;

use crate::errors::RuntimeError;

use super::{
    emit_wait_profile, ApprovalDecision, ApprovalRequest, Approver, BENCH_APPROVAL_WAIT_NS,
};

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
            let card = req.card();
            let decision = tokio::task::spawn_blocking(move || -> Result<ApprovalDecision, RuntimeError> {
                let stdout = std::io::stdout();
                let mut handle = stdout.lock();
                writeln!(handle, "─── corvid: approval requested ────────────────")
                    .map_err(|e| RuntimeError::ApprovalFailed(e.to_string()))?;
                write!(handle, "{}", card.render_text())
                    .map_err(|e| RuntimeError::ApprovalFailed(e.to_string()))?;
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
