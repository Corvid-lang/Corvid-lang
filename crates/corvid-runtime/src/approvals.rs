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
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::Write;
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

impl ApprovalRequest {
    pub fn card(&self) -> ApprovalCard {
        ApprovalCard::from_request(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalCard {
    pub label: String,
    pub title: String,
    pub risk: ApprovalRisk,
    pub arguments: Vec<ApprovalCardArgument>,
    pub context: Vec<String>,
    pub diff_preview: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRisk {
    Review,
    MoneyMovement,
    ExternalSideEffect,
    Irreversible,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalCardArgument {
    pub index: usize,
    pub json_type: String,
    pub value: serde_json::Value,
    pub redacted: bool,
}

impl ApprovalCard {
    pub fn from_request(req: &ApprovalRequest) -> Self {
        let arguments = req
            .args
            .iter()
            .enumerate()
            .map(|(index, value)| ApprovalCardArgument {
                index,
                json_type: json_type_name(value).into(),
                value: redact_if_sensitive(value),
                redacted: is_sensitive_value(value),
            })
            .collect::<Vec<_>>();
        let title = humanize_label(&req.label);
        let risk = infer_risk(&req.label, &req.args);
        Self {
            label: req.label.clone(),
            title: title.clone(),
            risk,
            context: build_context(&req.label, &arguments),
            diff_preview: Some(build_diff_preview(&title, risk, &arguments)),
            arguments,
        }
    }

    pub fn render_text(&self) -> String {
        let mut out = String::new();
        out.push_str("corvid approval card\n");
        out.push_str(&format!("  action: {}\n", self.title));
        out.push_str(&format!("  label: {}\n", self.label));
        out.push_str(&format!("  risk: {:?}\n", self.risk));
        for arg in &self.arguments {
            let suffix = if arg.redacted { " (redacted)" } else { "" };
            out.push_str(&format!(
                "  arg {} [{}]{}: {}\n",
                arg.index, arg.json_type, suffix, arg.value
            ));
        }
        for line in &self.context {
            out.push_str(&format!("  context: {line}\n"));
        }
        if let Some(diff) = &self.diff_preview {
            out.push_str("  preview:\n");
            out.push_str(diff);
            out.push('\n');
        }
        out
    }
}

fn build_context(label: &str, args: &[ApprovalCardArgument]) -> Vec<String> {
    let plural = if args.len() == 1 { "" } else { "s" };
    let types = args
        .iter()
        .map(|arg| format!("arg{}:{}", arg.index, arg.json_type))
        .collect::<Vec<_>>()
        .join(", ");
    vec![
        format!("why: program requested approval `{label}` before a protected action"),
        format!("scope: one decision covers this label and exact argument payload"),
        format!(
            "argument inspection: {} argument{}{}",
            args.len(),
            plural,
            if types.is_empty() {
                String::new()
            } else {
                format!(" ({types})")
            }
        ),
    ]
}

fn build_diff_preview(
    title: &str,
    risk: ApprovalRisk,
    args: &[ApprovalCardArgument],
) -> String {
    let mut out = String::new();
    out.push_str("    before: protected action is blocked\n");
    out.push_str(&format!(
        "    after: `{title}` may proceed as {}\n",
        risk_preview_label(risk)
    ));
    if args.is_empty() {
        out.push_str("    arguments: <none>");
    } else {
        out.push_str("    arguments:\n");
        for arg in args {
            let suffix = if arg.redacted { " (redacted)" } else { "" };
            out.push_str(&format!(
                "      - arg {} [{}]{} = {}\n",
                arg.index, arg.json_type, suffix, arg.value
            ));
        }
        while out.ends_with('\n') {
            out.pop();
        }
    }
    out
}

fn risk_preview_label(risk: ApprovalRisk) -> &'static str {
    match risk {
        ApprovalRisk::Review => "a reviewed operation",
        ApprovalRisk::MoneyMovement => "a money-moving operation",
        ApprovalRisk::ExternalSideEffect => "an external side effect",
        ApprovalRisk::Irreversible => "an irreversible operation",
    }
}

fn humanize_label(label: &str) -> String {
    let mut out = String::new();
    for (index, ch) in label.chars().enumerate() {
        if index > 0 && ch.is_ascii_uppercase() {
            out.push(' ');
        }
        if index == 0 {
            out.extend(ch.to_uppercase());
        } else {
            out.push(ch);
        }
    }
    out.replace(['_', '-'], " ")
}

fn infer_risk(label: &str, args: &[serde_json::Value]) -> ApprovalRisk {
    let lower = label.to_ascii_lowercase();
    if lower.contains("delete") || lower.contains("irreversible") || lower.contains("void") {
        ApprovalRisk::Irreversible
    } else if lower.contains("refund")
        || lower.contains("charge")
        || lower.contains("payment")
        || args
            .iter()
            .any(|value| value.as_f64().is_some_and(|n| n.abs() >= 100.0))
    {
        ApprovalRisk::MoneyMovement
    } else if lower.contains("send") || lower.contains("external") || lower.contains("email") {
        ApprovalRisk::ExternalSideEffect
    } else {
        ApprovalRisk::Review
    }
}

fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn redact_if_sensitive(value: &serde_json::Value) -> serde_json::Value {
    if is_sensitive_value(value) {
        serde_json::Value::String("<redacted>".into())
    } else {
        value.clone()
    }
}

fn is_sensitive_value(value: &serde_json::Value) -> bool {
    value.as_str().is_some_and(|text| {
        let lower = text.to_ascii_lowercase();
        lower.contains("secret")
            || lower.contains("token")
            || lower.contains("password")
            || lower.chars().filter(|ch| ch.is_ascii_digit()).count() >= 12
    })
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
    }
}
