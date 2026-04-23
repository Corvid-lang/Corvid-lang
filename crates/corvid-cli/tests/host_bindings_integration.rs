use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

fn demo_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

fn shared_library_name(stem: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("lib{stem}.dylib")
    } else if cfg!(windows) {
        format!("{stem}.dll")
    } else {
        format!("lib{stem}.so")
    }
}

fn run_corvid(args: &[&str], cwd: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_corvid"))
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run corvid")
}

fn run_python(script: &Path, cwd: &Path, library: &Path) -> Option<std::process::Output> {
    let candidates = if cfg!(windows) {
        vec![
            vec!["py".to_string(), "-3".to_string()],
            vec!["python".to_string()],
        ]
    } else {
        vec![vec!["python3".to_string()], vec!["python".to_string()]]
    };
    for candidate in candidates {
        let mut probe = Command::new(&candidate[0]);
        for arg in &candidate[1..] {
            probe.arg(arg);
        }
        if !probe
            .arg("--version")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
        {
            continue;
        }
        let mut cmd = Command::new(&candidate[0]);
        for arg in &candidate[1..] {
            cmd.arg(arg);
        }
        return Some(
            cmd.arg(script)
                .arg(library)
                .current_dir(cwd)
                .env("CORVID_MODEL", "mock-1")
                .env("CORVID_TEST_MOCK_LLM", "1")
                .env(
                    "CORVID_TEST_MOCK_LLM_REPLIES",
                    r#"{"classify_prompt":"positive"}"#,
                )
                .output()
                .expect("run python host"),
        );
    }
    eprintln!("skipping python host binding checks: no python on PATH");
    None
}

fn test_tools_lib_path() -> PathBuf {
    let root = workspace_root();
    let status = Command::new("cargo")
        .arg("build")
        .arg("-p")
        .arg("corvid-test-tools")
        .arg("--release")
        .current_dir(&root)
        .status()
        .expect("build corvid-test-tools");
    assert!(status.success(), "building corvid-test-tools failed");
    if cfg!(windows) {
        root.join("target").join("release").join("corvid_test_tools.lib")
    } else {
        root.join("target").join("release").join("libcorvid_test_tools.a")
    }
}

fn build_demo_cdylib(source: &Path, cwd: &Path, tools_lib: &Path) -> PathBuf {
    let build_output = run_corvid(
        &[
            "build",
            source.to_str().expect("utf8 source"),
            "--target=cdylib",
            "--with-tools-lib",
            tools_lib.to_str().expect("utf8 tools lib"),
            "--all-artifacts",
        ],
        cwd,
    );
    assert!(
        build_output.status.success(),
        "cdylib build failed: stdout={} stderr={}",
        String::from_utf8_lossy(&build_output.stdout),
        String::from_utf8_lossy(&build_output.stderr)
    );
    source
        .parent()
        .and_then(Path::parent)
        .expect("project root")
        .join("target")
        .join("release")
}

fn write_rust_smoke(crate_dir: &Path) {
    let bin_dir = crate_dir.join("src").join("bin");
    std::fs::create_dir_all(&bin_dir).expect("create rust bin dir");
    std::fs::write(
        bin_dir.join("smoke.rs"),
        r#"use classify::{
    ApprovalDecision, ApprovalRequest, Approver, ClassifyApi, Client, GroundedTagApi,
    IssueTagApi, TrustTier,
};

struct AcceptApprover;

impl Approver for AcceptApprover {
    fn decide(&self, _request: &ApprovalRequest) -> ApprovalDecision {
        ApprovalDecision::Accept
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let library_path = std::env::args().nth(1).expect("usage: smoke <library>");
    let client = Client::load(&library_path)?;

    let (classification, observation) = client.classify("I loved the support experience")?;
    println!("classification={classification} exceeded={}", observation.exceeded_bound());

    let (issued, _) = client.issue_tag("approved", &AcceptApprover)?;
    println!("issue_tag={issued}");

    let (grounded, _) = client.grounded_tag("catalog-proof")?;
    let sources = grounded.provenance().sources()?;
    println!("grounded={} sources={}", grounded.payload(), sources.len());

    let filtered = client
        .catalog()
        .where_trust_tier_le(TrustTier::Autonomous)
        .not_dangerous()
        .find()?;
    println!("filtered={}", filtered.len());
    Ok(())
}
"#,
    )
    .expect("write rust smoke");
}

fn write_python_smoke(package_dir: &Path) -> PathBuf {
    let script = package_dir.join("smoke.py");
    std::fs::write(
        &script,
        r#"from __future__ import annotations

import sys

from classify import ApprovalDecision, ApprovalRequest, Client, TrustTier


class AcceptApprover:
    def decide(self, _request: ApprovalRequest) -> ApprovalDecision:
        return ApprovalDecision.ACCEPT


def main() -> int:
    library_path = sys.argv[1]
    client = Client(library_path)

    classification, observation = client.classify("I loved the support experience")
    print(f"classification={classification} exceeded={observation.exceeded_bound()}")

    issued, _ = client.issue_tag("approved", approver=AcceptApprover())
    print(f"issue_tag={issued}")

    grounded, _ = client.grounded_tag("catalog-proof")
    print(f"grounded={grounded.payload()} sources={len(grounded.provenance().sources())}")

    filtered = (
        client.catalog()
        .where_trust_tier_le(TrustTier.AUTONOMOUS)
        .not_dangerous()
        .find()
    )
    print(f"filtered={len(filtered)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
"#,
    )
    .expect("write python smoke");
    script
}

#[test]
fn generated_host_bindings_compile_call_demo_and_fail_on_descriptor_drift() {
    let _guard = demo_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = workspace_root();
    let demo_dir = root.join("examples").join("cdylib_catalog_demo");
    let demo_source = demo_dir.join("src").join("classify.cor");
    let tools_lib = test_tools_lib_path();

    let release_dir = build_demo_cdylib(&demo_source, &root, &tools_lib);
    let descriptor = release_dir.join("classify.corvid-abi.json");
    let library = release_dir.join(shared_library_name("classify"));

    let temp = tempfile::tempdir().expect("tempdir");
    let rust_out = temp.path().join("rust_bindings");
    let python_out = temp.path().join("python_bindings");

    let rust_bind = run_corvid(
        &[
            "bind",
            "rust",
            descriptor.to_str().expect("utf8 descriptor"),
            "--out",
            rust_out.to_str().expect("utf8 rust out"),
        ],
        &root,
    );
    assert!(
        rust_bind.status.success(),
        "rust binding generation failed: stdout={} stderr={}",
        String::from_utf8_lossy(&rust_bind.stdout),
        String::from_utf8_lossy(&rust_bind.stderr)
    );

    let python_bind = run_corvid(
        &[
            "bind",
            "python",
            descriptor.to_str().expect("utf8 descriptor"),
            "--out",
            python_out.to_str().expect("utf8 python out"),
        ],
        &root,
    );
    assert!(
        python_bind.status.success(),
        "python binding generation failed: stdout={} stderr={}",
        String::from_utf8_lossy(&python_bind.stdout),
        String::from_utf8_lossy(&python_bind.stderr)
    );

    write_rust_smoke(&rust_out);
    let rust_target_dir = temp.path().join("rust_target");
    let rust_output = Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .arg("--bin")
        .arg("smoke")
        .arg("--")
        .arg(&library)
        .current_dir(&rust_out)
        .env("CARGO_TARGET_DIR", &rust_target_dir)
        .env("CORVID_MODEL", "mock-1")
        .env("CORVID_TEST_MOCK_LLM", "1")
        .env(
            "CORVID_TEST_MOCK_LLM_REPLIES",
            r#"{"classify_prompt":"positive"}"#,
        )
        .output()
        .expect("run rust smoke");
    assert!(
        rust_output.status.success(),
        "rust smoke failed: stdout={} stderr={}",
        String::from_utf8_lossy(&rust_output.stdout),
        String::from_utf8_lossy(&rust_output.stderr)
    );
    let rust_stdout = String::from_utf8_lossy(&rust_output.stdout);
    assert!(rust_stdout.contains("classification=positive"), "stdout was: {rust_stdout}");
    assert!(rust_stdout.contains("issue_tag=approved"), "stdout was: {rust_stdout}");
    assert!(rust_stdout.contains("grounded=catalog-proof sources=1"), "stdout was: {rust_stdout}");
    assert!(rust_stdout.contains("filtered="), "stdout was: {rust_stdout}");

    let python_script = write_python_smoke(&python_out);
    if let Some(python_output) = run_python(&python_script, &python_out, &library) {
        assert!(
            python_output.status.success(),
            "python smoke failed: stdout={} stderr={}",
            String::from_utf8_lossy(&python_output.stdout),
            String::from_utf8_lossy(&python_output.stderr)
        );
        let python_stdout = String::from_utf8_lossy(&python_output.stdout);
        assert!(
            python_stdout.contains("classification=positive"),
            "stdout was: {python_stdout}"
        );
        assert!(python_stdout.contains("issue_tag=approved"), "stdout was: {python_stdout}");
        assert!(
            python_stdout.contains("grounded=catalog-proof sources=1"),
            "stdout was: {python_stdout}"
        );
        assert!(python_stdout.contains("filtered="), "stdout was: {python_stdout}");
    }

    let drift_root = temp.path().join("drift_case");
    let drift_src_dir = drift_root.join("src");
    std::fs::create_dir_all(&drift_src_dir).expect("create drift src");
    let drift_source = drift_src_dir.join("classify.cor");
    let original = std::fs::read_to_string(&demo_source).expect("read demo source");
    let modified = original.replacen("@budget($0.01)", "@budget($0.03)", 1);
    std::fs::write(&drift_source, modified).expect("write drift source");
    let drift_release_dir = build_demo_cdylib(&drift_source, &drift_root, &tools_lib);
    let drift_library = drift_release_dir.join(shared_library_name("classify"));

    let rust_drift = Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .arg("--bin")
        .arg("smoke")
        .arg("--")
        .arg(&drift_library)
        .current_dir(&rust_out)
        .env("CARGO_TARGET_DIR", &rust_target_dir)
        .env("CORVID_MODEL", "mock-1")
        .env("CORVID_TEST_MOCK_LLM", "1")
        .env(
            "CORVID_TEST_MOCK_LLM_REPLIES",
            r#"{"classify_prompt":"positive"}"#,
        )
        .output()
        .expect("run rust drift smoke");
    assert!(
        !rust_drift.status.success(),
        "rust drift smoke unexpectedly succeeded: stdout={} stderr={}",
        String::from_utf8_lossy(&rust_drift.stdout),
        String::from_utf8_lossy(&rust_drift.stderr)
    );
    let rust_stderr = String::from_utf8_lossy(&rust_drift.stderr);
    assert!(
        rust_stderr.contains("DescriptorDrift"),
        "stderr was: {rust_stderr}"
    );

    if let Some(python_drift) = run_python(&python_script, &python_out, &drift_library) {
        assert!(
            !python_drift.status.success(),
            "python drift smoke unexpectedly succeeded: stdout={} stderr={}",
            String::from_utf8_lossy(&python_drift.stdout),
            String::from_utf8_lossy(&python_drift.stderr)
        );
        let python_stderr = String::from_utf8_lossy(&python_drift.stderr);
        assert!(
            python_stderr.contains("DescriptorDriftError"),
            "stderr was: {python_stderr}"
        );
    }
}
