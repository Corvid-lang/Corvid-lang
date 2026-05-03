use super::*;
use crate::approvals::{ApprovalTokenScope, ProgrammaticApprover};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn call_tool_routes_through_registry() {
    let r = super::tests::rt();
    let v = r.call_tool("double", vec![json!(5)]).await.unwrap();
    assert_eq!(v, json!(10));
}

#[tokio::test]
async fn approval_gate_passes_when_approver_says_yes() {
    let r = super::tests::rt();
    r.approval_gate("Anything", vec![]).await.unwrap();
}

#[tokio::test]
async fn approval_gate_emits_scoped_token_for_approved_request() {
    let dir = tempfile::tempdir().unwrap();
    let trace_path = dir.path().join("approval.jsonl");
    let r = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .tracer(Tracer::open_path(&trace_path, "approval-run"))
        .build();

    r.approval_gate("IssueRefund", vec![json!("ord_1"), json!(12.5)])
        .await
        .unwrap();

    let events = corvid_trace_schema::read_events_from_path(&trace_path).unwrap();
    let token = events
        .iter()
        .find_map(|event| match event {
            TraceEvent::ApprovalTokenIssued {
                token_id,
                label,
                args,
                scope,
                issued_at_ms,
                expires_at_ms,
                ..
            } => Some((token_id, label, args, scope, *issued_at_ms, *expires_at_ms)),
            _ => None,
        })
        .expect("approval token event");

    assert!(token.0.starts_with("apr_"));
    assert_eq!(token.0.len(), 68);
    assert_eq!(token.1, "IssueRefund");
    assert_eq!(token.2, &vec![json!("ord_1"), json!(12.5)]);
    assert_eq!(token.3, "one_time");
    assert_eq!(token.5 - token.4, APPROVAL_TOKEN_TTL_MS);
}

#[test]
fn approval_scope_violation_is_trace_visible() {
    let dir = tempfile::tempdir().unwrap();
    let trace_path = dir.path().join("scope.jsonl");
    let r = Runtime::builder()
        .tracer(Tracer::open_path(&trace_path, "scope-run"))
        .build();
    let mut token = ApprovalToken {
        token_id: "apr_limit".into(),
        label: "ChargeCard".into(),
        args: vec![json!(100.0)],
        scope: ApprovalTokenScope::AmountLimited { max_amount: 100.0 },
        issued_at_ms: 0,
        expires_at_ms: u64::MAX,
        uses_remaining: 1,
    };

    let err = r
        .validate_approval_token_scope(&mut token, "ChargeCard", &[json!(125.0)], None)
        .unwrap_err();
    assert!(matches!(err, RuntimeError::ApprovalFailed(_)));

    let events = corvid_trace_schema::read_events_from_path(&trace_path).unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        TraceEvent::ApprovalScopeViolation {
            token_id,
            label,
            reason,
            ..
        } if token_id == "apr_limit"
            && label == "ChargeCard"
            && reason.contains("exceeds token limit")
    )));
}

#[tokio::test]
async fn approval_gate_blocks_when_approver_says_no() {
    let r = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_no()))
        .build();
    let err = r.approval_gate("IssueRefund", vec![]).await.unwrap_err();
    assert!(matches!(
        err,
        RuntimeError::ApprovalDenied { ref action } if action == "IssueRefund"
    ));
}
