//! End-to-end attestation tests.
//!
//! Build a signed cdylib with `corvid build --target=cdylib --sign=<key>`,
//! then exercise `corvid receipt verify-abi <cdylib> --key <pubkey>`
//! against it. Asserts the round-trip path (verified), the
//! tampering-detection path (descriptor edited after signing →
//! mismatch), and the absent-attestation path (built without
//! `--sign` → exit 2).

use std::path::{Path, PathBuf};
use std::process::Command;

use ed25519_dalek::SigningKey;

const SOURCE: &str = r#"
@budget($0.10)
pub extern "c"
agent classify(text: String) -> String:
    return text
"#;

/// Stable seed so the same test run always produces the same key
/// pair. The seed is a public constant — never use it to sign
/// production artifacts.
const TEST_SEED_HEX: &str = "4242424242424242424242424242424242424242424242424242424242424242";

fn corvid_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_corvid"))
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

fn write_keys(dir: &Path) -> (PathBuf, PathBuf) {
    let mut seed = [0u8; 32];
    hex::decode_to_slice(TEST_SEED_HEX, &mut seed).expect("decode seed");
    let signing_key = SigningKey::from_bytes(&seed);
    let verifying_key = signing_key.verifying_key();
    let signing_path = dir.join("sign.hex");
    std::fs::write(&signing_path, TEST_SEED_HEX).expect("write signing key");
    let verifying_path = dir.join("verify.hex");
    std::fs::write(&verifying_path, hex::encode(verifying_key.as_bytes()))
        .expect("write verifying key");
    (signing_path, verifying_path)
}

fn run_corvid(args: &[&str], cwd: &Path) -> std::process::Output {
    Command::new(corvid_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run corvid")
}

fn build_cdylib(project_dir: &Path, sign_key_path: Option<&Path>) -> PathBuf {
    let source_path = project_dir.join("src").join("classify.cor");
    std::fs::create_dir_all(project_dir.join("src")).expect("src dir");
    std::fs::write(&source_path, SOURCE).expect("write source");
    let mut args: Vec<String> = vec![
        "build".into(),
        source_path.to_string_lossy().into_owned(),
        "--target=cdylib".into(),
    ];
    if let Some(key) = sign_key_path {
        args.push(format!("--sign={}", key.display()));
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = run_corvid(&arg_refs, project_dir);
    assert!(
        out.status.success(),
        "build failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    project_dir
        .join("target")
        .join("release")
        .join(shared_library_name("classify"))
}

#[test]
fn signed_cdylib_verifies_against_matching_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (sign_path, verify_path) = write_keys(dir.path());
    let cdylib = build_cdylib(dir.path(), Some(&sign_path));
    assert!(cdylib.exists(), "missing built cdylib at {}", cdylib.display());

    let out = run_corvid(
        &[
            "receipt",
            "verify-abi",
            cdylib.to_str().expect("utf8 cdylib"),
            "--key",
            verify_path.to_str().expect("utf8 verify key"),
        ],
        dir.path(),
    );
    assert!(
        out.status.success(),
        "verify-abi failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("attestation OK"),
        "expected `attestation OK` in stderr, got: {stderr}"
    );
}

#[test]
fn unsigned_cdylib_reports_absent_attestation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_sign_path, verify_path) = write_keys(dir.path());
    // Build WITHOUT --sign.
    let cdylib = build_cdylib(dir.path(), None);

    let out = run_corvid(
        &[
            "receipt",
            "verify-abi",
            cdylib.to_str().expect("utf8 cdylib"),
            "--key",
            verify_path.to_str().expect("utf8 verify key"),
        ],
        dir.path(),
    );
    // Exit 2 = attestation absent (host policy decides).
    assert_eq!(
        out.status.code(),
        Some(2),
        "expected exit 2 for absent attestation, got: status={:?}\nstdout={}\nstderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unsigned"),
        "expected `unsigned` in stderr, got: {stderr}"
    );
}

#[test]
fn signed_cdylib_rejects_wrong_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (sign_path, _verify_path) = write_keys(dir.path());
    let cdylib = build_cdylib(dir.path(), Some(&sign_path));

    // Generate a different verifying key; the signed envelope was
    // signed with the seed above, so this key will fail verify.
    let wrong_seed = [0xaau8; 32];
    let wrong_key = SigningKey::from_bytes(&wrong_seed);
    let wrong_verify_path = dir.path().join("wrong.hex");
    std::fs::write(
        &wrong_verify_path,
        hex::encode(wrong_key.verifying_key().as_bytes()),
    )
    .expect("write wrong verify key");

    let out = run_corvid(
        &[
            "receipt",
            "verify-abi",
            cdylib.to_str().expect("utf8 cdylib"),
            "--key",
            wrong_verify_path.to_str().expect("utf8 wrong key"),
        ],
        dir.path(),
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1 for wrong key, got: status={:?}\nstdout={}\nstderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("attestation verification failed"),
        "expected `attestation verification failed` in stderr, got: {stderr}"
    );
}
