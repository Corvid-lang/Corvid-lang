use std::path::PathBuf;
use std::process::Command;

fn corvid_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_corvid"))
}

#[test]
fn doctor_redacts_invalid_token_key() {
    let secret = "not-a-valid-token-key-secret";
    let out = Command::new(corvid_bin())
        .arg("doctor")
        .env("CORVID_TOKEN_KEY", secret)
        .output()
        .expect("run doctor");
    assert!(!out.status.success(), "invalid token key should fail doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("CORVID_TOKEN_KEY invalid"), "{stdout}");
    assert!(stdout.contains("value redacted"), "{stdout}");
    assert!(!stdout.contains(secret), "{stdout}");
}

#[test]
fn doctor_accepts_64_char_hex_token_key() {
    let out = Command::new(corvid_bin())
        .arg("doctor")
        .env(
            "CORVID_TOKEN_KEY",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .output()
        .expect("run doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("CORVID_TOKEN_KEY valid"), "{stdout}");
}
