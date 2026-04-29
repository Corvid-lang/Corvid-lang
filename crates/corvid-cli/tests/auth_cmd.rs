//! Integration tests for `corvid auth` + `corvid approvals` —
//! slice 39L. Mirrors `connectors_cmd.rs` integration test
//! patterns: spawn the built `corvid` binary, run the subcommand,
//! assert the JSON output and exit code.

use std::path::Path;
use std::process::Command;

fn run_corvid(args: &[&str]) -> std::process::Output {
    let exe = env!("CARGO_BIN_EXE_corvid");
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root");
    Command::new(exe)
        .args(args)
        .current_dir(repo)
        .output()
        .expect("run corvid")
}

/// Slice 39L integration: `corvid auth migrate` opens both stores
/// and exits 0; the JSON output names the resolved paths.
#[test]
fn auth_migrate_initialises_both_stores() {
    let temp = tempfile::tempdir().expect("temp");
    let auth = temp.path().join("auth.db");
    let approvals = temp.path().join("approvals.db");
    let output = run_corvid(&[
        "auth",
        "migrate",
        "--auth-state",
        auth.to_str().unwrap(),
        "--approvals-state",
        approvals.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "migrate failed: stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(parsed["auth_initialised"], serde_json::Value::Bool(true));
    assert_eq!(parsed["approvals_initialised"], serde_json::Value::Bool(true));
    assert!(auth.exists());
    assert!(approvals.exists());
}

/// Slice 39L integration: `corvid auth keys issue` mints a key and
/// returns the raw value once. Exits 0.
#[test]
fn auth_keys_issue_returns_raw_key() {
    let temp = tempfile::tempdir().expect("temp");
    let auth = temp.path().join("auth.db");
    let approvals = temp.path().join("approvals.db");
    run_corvid(&[
        "auth",
        "migrate",
        "--auth-state",
        auth.to_str().unwrap(),
        "--approvals-state",
        approvals.to_str().unwrap(),
    ]);
    let output = run_corvid(&[
        "auth",
        "keys",
        "issue",
        "--auth-state",
        auth.to_str().unwrap(),
        "--key-id",
        "k-int-1",
        "--service-actor",
        "svc-int-1",
        "--tenant",
        "tenant-int",
        "--raw-key",
        "ck_test_integration_value",
        "--scope-fingerprint",
        "scope:read",
    ]);
    assert!(
        output.status.success(),
        "issue failed: stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(parsed["key_id"], "k-int-1");
    assert_eq!(parsed["raw_key"], "ck_test_integration_value");
    assert_eq!(parsed["scope_fingerprint"], "scope:read");
}

/// Slice 39L integration: `corvid approvals queue` empty tenant
/// emits an empty list, not an error.
#[test]
fn approvals_queue_empty_tenant_returns_empty_list() {
    let temp = tempfile::tempdir().expect("temp");
    let auth = temp.path().join("auth.db");
    let approvals = temp.path().join("approvals.db");
    run_corvid(&[
        "auth",
        "migrate",
        "--auth-state",
        auth.to_str().unwrap(),
        "--approvals-state",
        approvals.to_str().unwrap(),
    ]);
    let output = run_corvid(&[
        "approvals",
        "queue",
        "--approvals-state",
        approvals.to_str().unwrap(),
        "--tenant",
        "no-such-tenant",
    ]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(parsed["tenant_id"], "no-such-tenant");
    assert_eq!(parsed["approvals"].as_array().unwrap().len(), 0);
}

/// Slice 39L integration: `corvid approvals export --out FILE`
/// writes the export to disk and prints a summary to stdout.
#[test]
fn approvals_export_writes_to_file() {
    let temp = tempfile::tempdir().expect("temp");
    let auth = temp.path().join("auth.db");
    let approvals = temp.path().join("approvals.db");
    let out_path = temp.path().join("export.json");
    run_corvid(&[
        "auth",
        "migrate",
        "--auth-state",
        auth.to_str().unwrap(),
        "--approvals-state",
        approvals.to_str().unwrap(),
    ]);
    let output = run_corvid(&[
        "approvals",
        "export",
        "--approvals-state",
        approvals.to_str().unwrap(),
        "--tenant",
        "tenant-empty",
        "--out",
        out_path.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "export failed: stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let summary: serde_json::Value = serde_json::from_str(&stdout).expect("summary json");
    assert_eq!(summary["approval_count"], 0);
    let written = std::fs::read_to_string(&out_path).expect("export file");
    let parsed: serde_json::Value = serde_json::from_str(&written).expect("export json");
    assert_eq!(parsed["tenant_id"], "tenant-empty");
}
