use corvid_abi::{hash_json_str, read_embedded_section_from_library};
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

const SRC: &str = r#"
@budget($0.01)
pub extern "c"
agent classify(text: String) -> String:
    return promptify(text)

prompt promptify(text: String) -> String:
    """Echo {text}."""
"#;

fn build_cdylib_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let (dir, source_path) = write_project(SRC, "classify");
    let output = run_corvid(
        &[
            "build",
            source_path.to_str().expect("utf8 source path"),
            "--target=cdylib",
            "--all-artifacts",
        ],
        source_path.parent().expect("src dir"),
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
    let lib_path = release_dir.join(shared_library_name("classify"));
    let descriptor_path = release_dir.join("classify.corvid-abi.json");
    assert!(lib_path.exists(), "missing library: {}", lib_path.display());
    assert!(
        descriptor_path.exists(),
        "missing descriptor: {}",
        descriptor_path.display()
    );
    (dir, source_path, lib_path)
}

#[test]
fn abi_dump_prints_embedded_descriptor_json() {
    let (_dir, _source_path, lib_path) = build_cdylib_fixture();
    let output = run_corvid(&["abi", "dump", lib_path.to_str().expect("utf8 lib path")], lib_path.parent().unwrap());
    assert!(
        output.status.success(),
        "abi dump failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let embedded = read_embedded_section_from_library(&lib_path).expect("embedded descriptor");
    let dumped: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("dump stdout json");
    let expected: serde_json::Value =
        serde_json::from_str(&embedded.json).expect("embedded json parse");
    assert_eq!(dumped, expected);
}

#[test]
fn abi_hash_matches_embedded_descriptor_hash() {
    let (_dir, source_path, lib_path) = build_cdylib_fixture();
    let output = run_corvid(
        &["abi", "hash", source_path.to_str().expect("utf8 source path")],
        source_path.parent().unwrap(),
    );
    assert!(
        output.status.success(),
        "abi hash failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let hash = String::from_utf8(output.stdout).expect("hash stdout utf8");
    let embedded = read_embedded_section_from_library(&lib_path).expect("embedded descriptor");
    assert_eq!(hash.trim(), encode_hex(&hash_json_str(&embedded.json)));
}

#[test]
fn abi_verify_exits_zero_on_match_and_two_on_mismatch() {
    let (_dir, source_path, lib_path) = build_cdylib_fixture();
    let hash_output = run_corvid(
        &["abi", "hash", source_path.to_str().expect("utf8 source path")],
        source_path.parent().unwrap(),
    );
    assert!(hash_output.status.success(), "abi hash failed");
    let expected_hash = String::from_utf8(hash_output.stdout).expect("utf8 hash");

    let ok = run_corvid(
        &[
            "abi",
            "verify",
            lib_path.to_str().expect("utf8 lib path"),
            "--expected-hash",
            expected_hash.trim(),
        ],
        lib_path.parent().unwrap(),
    );
    assert_eq!(ok.status.code(), Some(0));

    let bad = run_corvid(
        &[
            "abi",
            "verify",
            lib_path.to_str().expect("utf8 lib path"),
            "--expected-hash",
            "0000000000000000000000000000000000000000000000000000000000000000",
        ],
        lib_path.parent().unwrap(),
    );
    assert_eq!(bad.status.code(), Some(2));
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}
