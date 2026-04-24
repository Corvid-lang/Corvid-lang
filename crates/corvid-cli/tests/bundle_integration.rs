mod bundle_support;

use std::fs;
use std::process::Command;

use bundle_support::{create_fixture, run_corvid, workspace_root};

#[test]
fn bundle_diff_audit_explain_and_report_emit_structured_output() {
    let fixture = create_fixture();
    let other = create_fixture();

    let mut descriptor: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&other.descriptor_path).expect("read descriptor"),
    )
    .expect("parse descriptor");
    descriptor["agents"][0]["return_ownership"]["mode"] = serde_json::json!("borrowed");
    descriptor["agents"][0]["return_ownership"]["lifetime"] = serde_json::json!("call");
    fs::write(
        &other.descriptor_path,
        serde_json::to_string_pretty(&descriptor).expect("descriptor json"),
    )
    .expect("write descriptor");

    let diff = run_corvid(
        &[
            "bundle",
            "diff",
            fixture.root.to_str().expect("utf8 root"),
            other.root.to_str().expect("utf8 root"),
            "--json",
        ],
        &fixture.root,
    );
    assert!(diff.status.success(), "bundle diff failed: {}", String::from_utf8_lossy(&diff.stderr));
    let diff_stdout = String::from_utf8_lossy(&diff.stdout);
    assert!(diff_stdout.contains("ownership_changes"), "stdout was: {diff_stdout}");

    let audit = run_corvid(
        &[
            "bundle",
            "audit",
            fixture.root.to_str().expect("utf8 root"),
            "--question",
            "Which agents require approval?",
            "--json",
        ],
        &fixture.root,
    );
    assert!(audit.status.success(), "bundle audit failed: {}", String::from_utf8_lossy(&audit.stderr));
    let audit_stdout = String::from_utf8_lossy(&audit.stdout);
    assert!(audit_stdout.contains("approval-gated agents"), "stdout was: {audit_stdout}");

    let explain = run_corvid(
        &[
            "bundle",
            "explain",
            fixture.root.to_str().expect("utf8 root"),
            "--json",
        ],
        &fixture.root,
    );
    assert!(explain.status.success(), "bundle explain failed: {}", String::from_utf8_lossy(&explain.stderr));
    let explain_stdout = String::from_utf8_lossy(&explain.stdout);
    assert!(explain_stdout.contains("\"trace_count\""), "stdout was: {explain_stdout}");

    let report = run_corvid(
        &[
            "bundle",
            "report",
            fixture.root.to_str().expect("utf8 root"),
            "--format",
            "soc2",
            "--json",
        ],
        &fixture.root,
    );
    assert!(report.status.success(), "bundle report failed: {}", String::from_utf8_lossy(&report.stderr));
    let report_stdout = String::from_utf8_lossy(&report.stdout);
    assert!(report_stdout.contains("CC7.2"), "stdout was: {report_stdout}");
}

#[test]
fn committed_phase22_demo_bundle_script_passes_on_linux() {
    if !cfg!(target_os = "linux") {
        eprintln!("skipping Linux-only committed bundle script test on non-Linux host");
        return;
    }

    let root = workspace_root();
    let output = Command::new("bash")
        .arg(root.join("examples").join("phase22_demo").join("verify.sh"))
        .current_dir(&root)
        .output()
        .expect("run phase22 demo verify.sh");
    assert!(
        output.status.success(),
        "phase22 demo verify.sh failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn committed_failing_bundle_scripts_report_expected_errors_on_linux() {
    if !cfg!(target_os = "linux") {
        eprintln!("skipping Linux-only failing bundle script test on non-Linux host");
        return;
    }

    let root = workspace_root();
    for dir in [
        "failing_hash",
        "failing_signature",
        "failing_rebuild",
        "failing_lineage",
        "failing_adversarial",
    ] {
        let script = root.join("examples").join(dir).join("verify.sh");
        let output = Command::new("bash")
            .arg(&script)
            .current_dir(&root)
            .output()
            .unwrap_or_else(|err| panic!("run {}: {err}", script.display()));
        assert!(
            output.status.success(),
            "{} failed: stdout={} stderr={}",
            script.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
