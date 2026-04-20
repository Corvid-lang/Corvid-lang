use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf()
}

fn write_project(src: &str, stem: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let src_dir = dir.path().join("src");
    std::fs::create_dir_all(&src_dir).expect("create src dir");
    let source_path = src_dir.join(format!("{stem}.cor"));
    std::fs::write(&source_path, src).expect("write source");
    (dir, source_path)
}

fn run_corvid(args: &[&str], target_dir: &Path) -> std::process::Output {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    Command::new(cargo)
        .args(["run", "-q", "-p", "corvid-cli", "--"])
        .args(args)
        .current_dir(repo_root())
        .env("CARGO_TARGET_DIR", target_dir)
        .output()
        .expect("run corvid")
}

const SCALAR_SRC: &str = r#"
pub extern "c"
agent refund_bot(ticket_id: String, amount: Float) -> Bool:
    return ticket_id == "vip" and amount > 10.0
"#;

#[test]
fn cdylib_with_abi_descriptor_flag_writes_json_alongside_library() {
    let (_dir, source_path) = write_project(SCALAR_SRC, "refund_bot");
    let target_dir = repo_root().join("target");
    let output = run_corvid(
        &[
            "build",
            source_path.to_str().expect("utf8 path"),
            "--target=cdylib",
            "--abi-descriptor",
        ],
        &target_dir,
    );

    assert!(
        output.status.success(),
        "build failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let release_dir = source_path
        .parent()
        .and_then(Path::parent)
        .expect("project root")
        .join("target")
        .join("release");
    assert!(release_dir.join("refund_bot.corvid-abi.json").exists());
}

#[test]
fn cdylib_without_abi_descriptor_flag_does_not_write_json() {
    let (_dir, source_path) = write_project(SCALAR_SRC, "refund_bot");
    let target_dir = repo_root().join("target");
    let output = run_corvid(
        &[
            "build",
            source_path.to_str().expect("utf8 path"),
            "--target=cdylib",
        ],
        &target_dir,
    );

    assert!(
        output.status.success(),
        "build failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let release_dir = source_path
        .parent()
        .and_then(Path::parent)
        .expect("project root")
        .join("target")
        .join("release");
    assert!(!release_dir.join("refund_bot.corvid-abi.json").exists());
}

#[test]
fn all_artifacts_flag_writes_lib_header_and_descriptor() {
    let (_dir, source_path) = write_project(SCALAR_SRC, "refund_bot");
    let target_dir = repo_root().join("target");
    let output = run_corvid(
        &[
            "build",
            source_path.to_str().expect("utf8 path"),
            "--target=cdylib",
            "--all-artifacts",
        ],
        &target_dir,
    );

    assert!(
        output.status.success(),
        "build failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let release_dir = source_path
        .parent()
        .and_then(Path::parent)
        .expect("project root")
        .join("target")
        .join("release");
    assert!(release_dir.join("refund_bot.corvid-abi.json").exists());
    assert!(release_dir.join("lib_refund_bot.h").exists());
}
