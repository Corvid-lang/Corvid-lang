use std::path::{Path, PathBuf};
use std::process::Command;

use ed25519_dalek::SigningKey;

const SOURCE: &str = r#"
@budget($0.10)
pub extern "c"
agent classify(text: String) -> String:
    return text
"#;

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
    let signing_path = dir.join("sign.hex");
    std::fs::write(&signing_path, TEST_SEED_HEX).expect("write signing key");
    let verifying_path = dir.join("verify.hex");
    std::fs::write(
        &verifying_path,
        hex::encode(signing_key.verifying_key().as_bytes()),
    )
    .expect("write verifying key");
    (signing_path, verifying_path)
}

fn run_corvid(args: &[String], cwd: &Path) -> std::process::Output {
    Command::new(corvid_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run corvid")
}

fn build_signed_cdylib(project_dir: &Path, sign_key_path: &Path) -> (PathBuf, PathBuf) {
    let source_path = project_dir.join("src").join("classify.cor");
    std::fs::create_dir_all(project_dir.join("src")).expect("src dir");
    std::fs::write(&source_path, SOURCE).expect("write source");
    let args = vec![
        "build".to_string(),
        source_path.to_string_lossy().into_owned(),
        "--target=cdylib".to_string(),
        format!("--sign={}", sign_key_path.display()),
    ];
    let out = run_corvid(&args, project_dir);
    assert!(
        out.status.success(),
        "build failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (
        source_path,
        project_dir
            .join("target")
            .join("release")
            .join(shared_library_name("classify")),
    )
}

#[test]
fn claim_explain_reports_verified_signature_source_and_guarantees() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (sign_key, verify_key) = write_keys(dir.path());
    let (source, cdylib) = build_signed_cdylib(dir.path(), &sign_key);
    assert!(cdylib.exists(), "missing cdylib at {}", cdylib.display());

    let verified_args = vec![
        "claim".to_string(),
        "--explain".to_string(),
        cdylib.to_string_lossy().into_owned(),
        "--key".to_string(),
        verify_key.to_string_lossy().into_owned(),
        "--source".to_string(),
        source.to_string_lossy().into_owned(),
    ];
    let verified = run_corvid(&verified_args, dir.path());
    assert!(
        verified.status.success(),
        "claim --explain failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&verified.stdout),
        String::from_utf8_lossy(&verified.stderr)
    );
    let stdout = String::from_utf8_lossy(&verified.stdout);
    assert!(
        stdout.contains("Corvid cdylib claim explanation"),
        "{stdout}"
    );
    assert!(
        stdout.contains("attestation:\n  status: verified"),
        "{stdout}"
    );
    assert!(
        stdout.contains("signing_key_fingerprint: sha256:"),
        "{stdout}"
    );
    assert!(
        stdout.contains("source_descriptor_agreement:\n  status: verified"),
        "{stdout}"
    );
    assert!(
        stdout.contains("id: abi_attestation.envelope_signature; class: runtime_checked"),
        "{stdout}"
    );
    assert!(
        stdout.contains("id: abi_descriptor.bilateral_source_match; class: runtime_checked"),
        "{stdout}"
    );

    let unverified_args = vec![
        "claim".to_string(),
        "--explain".to_string(),
        cdylib.to_string_lossy().into_owned(),
    ];
    let unverified = run_corvid(&unverified_args, dir.path());
    assert!(
        unverified.status.success(),
        "claim without verification inputs should still explain honestly:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&unverified.stdout),
        String::from_utf8_lossy(&unverified.stderr)
    );
    let stdout = String::from_utf8_lossy(&unverified.stdout);
    assert!(
        stdout.contains("status: present_not_verified") && stdout.contains("pass `--key <pubkey>`"),
        "{stdout}"
    );
    assert!(
        stdout.contains("source_descriptor_agreement:\n  status: not_verified"),
        "{stdout}"
    );
}
