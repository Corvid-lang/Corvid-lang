use std::path::Path;
use std::process::Command;

fn run_corvid(args: &[&str], cwd: &Path) -> std::process::Output {
    let exe = env!("CARGO_BIN_EXE_corvid");
    Command::new(exe)
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run corvid")
}

#[test]
fn audit_reports_launch_relevant_findings() {
    let dir = tempfile::tempdir().expect("tempdir");
    let main_path = dir.path().join("main.cor");
    let policy_path = dir.path().join("policy.cor");

    std::fs::write(
        &policy_path,
        r#"
effect paid:
    cost: $1.25
    trust: human_required

effect secrets:
    data: secret

public tool issue_refund(id: String) -> String uses paid
public tool read_secret() -> String uses secrets

agent refund(id: String) -> String:
    return issue_refund(id)
"#,
    )
    .expect("write policy");

    std::fs::write(
        &main_path,
        r#"
import "./policy" as p

agent main(id: String) -> String:
    return p.issue_refund(id)
"#,
    )
    .expect("write main");

    let output = run_corvid(
        &["audit", main_path.to_str().expect("utf8 path"), "--json"],
        dir.path(),
    );
    assert!(
        !output.status.success(),
        "audit should return findings: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("audit json output");
    assert_eq!(json["module_count"].as_u64(), Some(2));
    let findings = json["findings"].as_array().expect("findings array");
    assert!(findings.iter().any(|finding| finding["category"] == "approval-boundary"));
    assert!(findings.iter().any(|finding| finding["category"] == "money-moving-path"));
    assert!(findings.iter().any(|finding| finding["category"] == "secret-access"));
    assert!(findings.iter().any(|finding| finding["category"] == "replay-coverage"));
}

#[test]
fn audit_plaintext_reports_clean_project() {
    let dir = tempfile::tempdir().expect("tempdir");
    let main_path = dir.path().join("main.cor");
    std::fs::write(
        &main_path,
        r#"
public @replayable
agent main() -> String:
    return "ok"
"#,
    )
    .expect("write main");

    let output = run_corvid(&["audit", main_path.to_str().expect("utf8 path")], dir.path());
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("No launch-blocking findings"));
}
