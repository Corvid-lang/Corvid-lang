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

fn shared_library_name(stem: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("lib{stem}.dylib")
    } else if cfg!(windows) {
        format!("{stem}.dll")
    } else {
        format!("lib{stem}.so")
    }
}

fn static_library_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.lib")
    } else {
        format!("lib{stem}.a")
    }
}

const SCALAR_SRC: &str = r#"
pub extern "c"
agent refund_bot(ticket_id: String, amount: Float) -> Bool:
    return ticket_id == "vip" and amount > 10.0
"#;

const NON_SCALAR_SRC: &str = r#"
type Ticket:
    id: String

pub extern "c"
agent refund_bot(ticket: Ticket) -> Bool:
    return true
"#;

#[test]
fn cli_build_cdylib_target_succeeds_on_scalar_agent() {
    let (_dir, source_path) = write_project(SCALAR_SRC, "refund_bot");
    let output = run_corvid(
        &[
            "build",
            source_path.to_str().expect("utf8 source path"),
            "--target=cdylib",
        ],
        source_path.parent().unwrap(),
    );

    assert!(
        output.status.success(),
        "build failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let lib_path = source_path
        .parent()
        .and_then(Path::parent)
        .expect("project root")
        .join("target")
        .join("release")
        .join(shared_library_name("refund_bot"));
    assert!(lib_path.exists(), "missing shared library: {}", lib_path.display());
}

#[test]
fn cli_build_staticlib_target_succeeds_on_scalar_agent() {
    let (_dir, source_path) = write_project(SCALAR_SRC, "refund_bot");
    let output = run_corvid(
        &[
            "build",
            source_path.to_str().expect("utf8 source path"),
            "--target=staticlib",
        ],
        source_path.parent().unwrap(),
    );

    assert!(
        output.status.success(),
        "build failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let lib_path = source_path
        .parent()
        .and_then(Path::parent)
        .expect("project root")
        .join("target")
        .join("release")
        .join(static_library_name("refund_bot"));
    assert!(lib_path.exists(), "missing static library: {}", lib_path.display());
}

#[test]
fn cli_build_cdylib_with_header_flag_writes_header_alongside_lib() {
    let (_dir, source_path) = write_project(SCALAR_SRC, "refund_bot");
    let output = run_corvid(
        &[
            "build",
            source_path.to_str().expect("utf8 source path"),
            "--target=cdylib",
            "--header",
        ],
        source_path.parent().unwrap(),
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
    let lib_path = release_dir.join(shared_library_name("refund_bot"));
    let header_path = release_dir.join("lib_refund_bot.h");
    assert!(lib_path.exists(), "missing shared library: {}", lib_path.display());
    assert!(header_path.exists(), "missing header: {}", header_path.display());

    let header = std::fs::read_to_string(&header_path).expect("read header");
    assert!(header.contains("bool refund_bot(const char* ticket_id, double amount);"));
}

#[test]
fn cli_build_cdylib_fails_cleanly_on_non_scalar_signature() {
    let (_dir, source_path) = write_project(NON_SCALAR_SRC, "refund_bot");
    let output = run_corvid(
        &[
            "build",
            source_path.to_str().expect("utf8 source path"),
            "--target=cdylib",
        ],
        source_path.parent().unwrap(),
    );

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Phase 22-B"), "stderr missing 22-B hint: {stderr}");
    assert!(
        stderr.contains("unsupported ABI type") || stderr.contains("struct") || stderr.contains("Ticket"),
        "stderr missing offender detail: {stderr}"
    );
}
