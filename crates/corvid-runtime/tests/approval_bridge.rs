use std::fs;
use std::path::Path;

use corvid_runtime::approver_bridge::{
    clear_registered_approver, evaluate_registered_approver, register_approver_from_source,
    simulate_approver_source, ApprovalSiteInput, CorvidApproverLoadStatus,
};
use serde_json::json;

fn write_source(dir: &Path, name: &str, source: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    fs::write(&path, source).expect("write approver source");
    path
}

fn sample_site(label: &str) -> ApprovalSiteInput {
    ApprovalSiteInput {
        site_name: label.to_string(),
        agent_context: "refund_bot".to_string(),
        declared_at_file: "examples/refund.cor".to_string(),
        declared_at_line: 42,
        budget_remaining_usd: 1.0,
        trace_run_id: "run-1".to_string(),
    }
}

#[test]
fn well_typed_approver_registers_and_evaluates() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_source(
        dir.path(),
        "approver.cor",
        r#"
@budget($0.05)
agent approve_site(site: ApprovalSite, args: ApprovalArgs, ctx: ApprovalContext) -> ApprovalDecision:
    if args.values[0] == "\"vip\"":
        return ApprovalDecision(true, "approved")
    return ApprovalDecision(false, "rejected")
"#,
    );

    register_approver_from_source(&path, 1.0).expect("register approver");
    let decision = evaluate_registered_approver(&sample_site("IssueRefund"), &[json!("vip")])
        .expect("evaluate approver")
        .expect("registered approver");
    assert!(decision.accepted);
    assert_eq!(decision.rationale.as_deref(), Some("approved"));

    clear_registered_approver();
}

#[test]
fn missing_approve_site_agent_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_source(
        dir.path(),
        "missing.cor",
        r#"
agent something_else() -> Bool:
    return true
"#,
    );

    let err = register_approver_from_source(&path, 1.0).expect_err("missing approve_site");
    assert_eq!(err.status, CorvidApproverLoadStatus::MissingAgent);
    assert!(err.message.contains("approve_site"));
}

#[test]
fn over_budget_approver_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_source(
        dir.path(),
        "over_budget.cor",
        r#"
@budget($2.00)
agent approve_site(site: ApprovalSite, args: ApprovalArgs, ctx: ApprovalContext) -> ApprovalDecision:
    return ApprovalDecision(true, "approved")
"#,
    );

    let err = register_approver_from_source(&path, 1.0).expect_err("over-budget approver");
    assert_eq!(err.status, CorvidApproverLoadStatus::OverBudget);
    assert!(err.message.contains("exceeds host ceiling"));
}

#[test]
fn simulate_returns_decision_and_rationale() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_source(
        dir.path(),
        "simulate.cor",
        r#"
agent approve_site(site: ApprovalSite, args: ApprovalArgs, ctx: ApprovalContext) -> ApprovalDecision:
    if site.label == "IssueRefund":
        return ApprovalDecision(false, "manual review")
    return ApprovalDecision(true, "approved")
"#,
    );

    let decision =
        simulate_approver_source(&path, "IssueRefund", "[1000]", 1.0).expect("simulate approver");
    assert!(!decision.accepted);
    assert_eq!(decision.rationale, "manual review");
}
