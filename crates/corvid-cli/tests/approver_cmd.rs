use std::path::{Path, PathBuf};
use std::process::Command;

fn write_project(src: &str, stem: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let src_dir = dir.path().join("src");
    std::fs::create_dir_all(&src_dir).expect("create src dir");
    let source_path = src_dir.join(format!("{stem}.cor"));
    std::fs::write(&source_path, src).expect("write source");
    (dir, source_path)
}

fn run_corvid(args: &[&str], cwd: &Path) -> std::process::Output {
    let exe = env!("CARGO_BIN_EXE_corvid");
    Command::new(exe)
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run corvid")
}

#[test]
fn approver_check_succeeds_for_valid_source() {
    let (_dir, source_path) = write_project(
        r#"
@budget($0.05)
agent approve_site(site: ApprovalSite, args: ApprovalArgs, ctx: ApprovalContext) -> ApprovalDecision:
    return ApprovalDecision(true, "approved")
"#,
        "approver",
    );
    let output = run_corvid(
        &[
            "approver",
            "check",
            source_path.to_str().expect("utf8 source path"),
        ],
        source_path.parent().expect("src dir"),
    );
    assert!(
        output.status.success(),
        "check failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn approver_check_reports_unsafe_for_dangerous_approver() {
    let (_dir, source_path) = write_project(
        r#"
@dangerous
agent approve_site(site: ApprovalSite, args: ApprovalArgs, ctx: ApprovalContext) -> ApprovalDecision:
    return ApprovalDecision(true, "approved")
"#,
        "dangerous_approver",
    );
    let output = run_corvid(
        &[
            "approver",
            "check",
            source_path.to_str().expect("utf8 source path"),
        ],
        source_path.parent().expect("src dir"),
    );
    assert!(!output.status.success(), "unsafe approver unexpectedly passed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unsafe") || stderr.contains("dangerous"));
}

#[test]
fn approver_simulate_prints_decision_and_rationale() {
    let (_dir, source_path) = write_project(
        r#"
agent approve_site(site: ApprovalSite, args: ApprovalArgs, ctx: ApprovalContext) -> ApprovalDecision:
    if site.label == "IssueRefund":
        return ApprovalDecision(false, "manual review")
    return ApprovalDecision(true, "approved")
"#,
        "simulate_approver",
    );
    let output = run_corvid(
        &[
            "approver",
            "simulate",
            source_path.to_str().expect("utf8 source path"),
            "IssueRefund",
            "--args",
            "[1000]",
        ],
        source_path.parent().expect("src dir"),
    );
    assert!(
        output.status.success(),
        "simulate failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("\"accepted\": false"));
    assert!(stdout.contains("manual review"));
}

#[test]
fn approver_card_renders_same_card_as_json_and_html() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json_output = run_corvid(
        &[
            "approver",
            "card",
            "ChargeCard",
            "--args",
            "[\"4242424242424242\",125]",
            "--format",
            "json",
        ],
        dir.path(),
    );
    assert!(
        json_output.status.success(),
        "json card failed: stdout={} stderr={}",
        String::from_utf8_lossy(&json_output.stdout),
        String::from_utf8_lossy(&json_output.stderr)
    );
    let json_stdout = String::from_utf8(json_output.stdout).expect("stdout utf8");
    assert!(json_stdout.contains("\"label\": \"ChargeCard\""));
    assert!(json_stdout.contains("\"diff_preview\""));
    assert!(json_stdout.contains("<redacted>"));

    let html_output = run_corvid(
        &[
            "approver",
            "card",
            "ChargeCard",
            "--args",
            "[\"4242424242424242\",125]",
            "--format",
            "html",
        ],
        dir.path(),
    );
    assert!(
        html_output.status.success(),
        "html card failed: stdout={} stderr={}",
        String::from_utf8_lossy(&html_output.stdout),
        String::from_utf8_lossy(&html_output.stderr)
    );
    let html_stdout = String::from_utf8(html_output.stdout).expect("stdout utf8");
    assert!(html_stdout.contains("<!doctype html>"));
    assert!(html_stdout.contains("Charge Card"));
    assert!(html_stdout.contains("&lt;redacted&gt;"));
}
