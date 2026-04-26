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
fn bench_compare_renders_python_report() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root");
    let output = run_corvid(
        &[
            "bench",
            "compare",
            "python",
            "--session",
            "2026-04-17-marketable-session",
        ],
        repo,
    );
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("tool_loop"));
    assert!(stdout.contains("Corvid faster"));
}

#[test]
fn bench_compare_emits_json_for_js_alias() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root");
    let output = run_corvid(
        &[
            "bench",
            "compare",
            "js",
            "--session",
            "2026-04-17-corrected-session",
            "--json",
        ],
        repo,
    );
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json output");
    assert_eq!(json["target"], "typescript");
    assert_eq!(json["session"], "2026-04-17-corrected-session");
    let scenarios = json["scenarios"].as_array().expect("scenario array");
    assert!(scenarios.iter().any(|scenario| scenario["faster_than_competitor"] == false));
}
