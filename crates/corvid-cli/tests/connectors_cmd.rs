//! Integration tests for `corvid connectors` — slice 41L.
//!
//! Exercise the user-facing CLI surface end-to-end: spawn the
//! built `corvid` binary, run a subcommand, assert the exit code
//! and structured output. Mirrors the `bench_cmd.rs` /
//! `audit_cmd.rs` patterns.

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

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
}

/// `corvid connectors list --json` returns a JSON array with one
/// row per shipped connector, including modes + scope_count +
/// write_scopes. The drift gate against this exit-0 contract is
/// what an operator's CI would enforce.
#[test]
fn list_json_lists_every_shipped_connector() {
    let output = run_corvid(&["connectors", "list", "--json"], repo_root());
    assert!(
        output.status.success(),
        "list failed: stderr={} stdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let arr = parsed.as_array().expect("array");
    let names: Vec<&str> = arr
        .iter()
        .filter_map(|e| e.get("name").and_then(|n| n.as_str()))
        .collect();
    assert!(names.contains(&"gmail"), "missing gmail: {names:?}");
    assert!(names.contains(&"slack"), "missing slack: {names:?}");
    // tasks manifest names itself differently; check by content.
    assert!(
        names
            .iter()
            .any(|n| n.contains("task") || n.contains("linear") || n.contains("github")),
        "missing tasks connector: {names:?}",
    );
}

/// `corvid connectors list` (human format) prints a table header
/// and at least one row, exits 0.
#[test]
fn list_human_format_renders_table() {
    let output = run_corvid(&["connectors", "list"], repo_root());
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("NAME"));
    assert!(stdout.contains("PROVIDER"));
    assert!(stdout.contains("MODES"));
    assert!(stdout.contains("gmail"));
}

/// `corvid connectors check` validates every manifest and exits 0
/// when all are valid (which is the default state of the shipped
/// manifests).
#[test]
fn check_passes_default_manifests() {
    let output = run_corvid(&["connectors", "check"], repo_root());
    assert!(
        output.status.success(),
        "check failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("VALID"));
    assert!(stdout.contains("✓"));
}

/// `corvid connectors check --live` without `CORVID_PROVIDER_LIVE=1`
/// refuses with an explicit message rather than silently
/// succeeding. This is the same posture the runtime takes against
/// accidental live HTTP calls.
#[test]
fn check_live_refuses_without_provider_live() {
    let output = run_corvid(&["connectors", "check", "--live"], repo_root());
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("CORVID_PROVIDER_LIVE")
            || stderr.contains("Live drift"),
        "{stderr}",
    );
}

/// `corvid connectors oauth init gmail --client-id=...` prints a
/// JSON object containing the Google OAuth2 v2 authorize endpoint
/// URL with PKCE parameters.
#[test]
fn oauth_init_gmail_prints_pkce_url() {
    let output = run_corvid(
        &[
            "connectors",
            "oauth",
            "init",
            "gmail",
            "--client-id",
            "client-1.apps.googleusercontent.com",
        ],
        repo_root(),
    );
    assert!(
        output.status.success(),
        "init failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let url = parsed
        .get("authorization_url")
        .and_then(|v| v.as_str())
        .expect("authorization_url");
    assert!(url.contains("https://accounts.google.com/o/oauth2/v2/auth"));
    assert!(url.contains("code_challenge_method=S256"));
    assert!(parsed.get("state").is_some());
    assert!(parsed.get("code_verifier").is_some());
    assert!(parsed.get("code_challenge").is_some());
}

/// `corvid connectors run --connector=gmail --mode=mock --mock=...`
/// drives the runtime against the supplied mock and prints the
/// payload back. Exits 0.
#[test]
fn run_mock_returns_supplied_mock_payload() {
    let temp = tempfile::tempdir().expect("temp");
    let payload_path = temp.path().join("payload.json");
    let mock_path = temp.path().join("mock.json");
    std::fs::write(
        &payload_path,
        r#"{"user_id":"u","query":"q","max_results":1}"#,
    )
    .unwrap();
    std::fs::write(&mock_path, r#"{"messages":[{"id":"m1"}]}"#).unwrap();
    let output = run_corvid(
        &[
            "connectors",
            "run",
            "--connector",
            "gmail",
            "--operation",
            "search",
            "--scope",
            "gmail.search",
            "--mode",
            "mock",
            "--payload",
            payload_path.to_str().unwrap(),
            "--mock",
            mock_path.to_str().unwrap(),
        ],
        repo_root(),
    );
    assert!(
        output.status.success(),
        "run failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(parsed["mode"], "mock");
    assert_eq!(parsed["payload"]["messages"][0]["id"], "m1");
}

/// `corvid connectors verify-webhook` exits 1 on a tampered body
/// and 0 when the signature matches the body content under the
/// declared secret.
#[test]
fn verify_webhook_round_trip() {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let temp = tempfile::tempdir().expect("temp");
    let body_path = temp.path().join("payload.json");
    let body = b"{\"event\":\"push\"}";
    std::fs::write(&body_path, body).unwrap();
    let secret = "shhh";
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    let expected: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    // Happy path: correct signature → exit 0, valid=true.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_corvid"));
    cmd.args([
        "connectors",
        "verify-webhook",
        "--signature",
        &format!("sha256={expected}"),
        "--secret-env",
        "CORVID_TEST_WEBHOOK_INTEGRATION",
        "--body-file",
        body_path.to_str().unwrap(),
    ]);
    cmd.env("CORVID_TEST_WEBHOOK_INTEGRATION", secret);
    cmd.current_dir(repo_root());
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "verify failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(parsed["valid"], serde_json::Value::Bool(true));

    // Tampered: wrong signature → exit 1, valid=false.
    let mut cmd2 = Command::new(env!("CARGO_BIN_EXE_corvid"));
    cmd2.args([
        "connectors",
        "verify-webhook",
        "--signature",
        "sha256=deadbeef00000000000000000000000000000000000000000000000000000000",
        "--secret-env",
        "CORVID_TEST_WEBHOOK_INTEGRATION_2",
        "--body-file",
        body_path.to_str().unwrap(),
    ]);
    cmd2.env("CORVID_TEST_WEBHOOK_INTEGRATION_2", secret);
    cmd2.current_dir(repo_root());
    let output2 = cmd2.output().unwrap();
    assert_eq!(output2.status.code(), Some(1));
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    let parsed2: serde_json::Value = serde_json::from_str(&stdout2).expect("json");
    assert_eq!(parsed2["valid"], serde_json::Value::Bool(false));
}
