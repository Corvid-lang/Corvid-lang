//! End-to-end integration tests for `--format=in-toto` and
//! `--format=in-toto --sign=<key>`.
//!
//! Verifies the output is a valid in-toto Statement v1 with the
//! Corvid receipt as the predicate, the subject digest matches
//! the head source file's SHA-256, and the signed variant wraps
//! the Statement in a DSSE envelope with the in-toto payloadType.

use std::path::{Path, PathBuf};
use std::process::Command;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};

fn run_git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "corvid-test")
        .env("GIT_AUTHOR_EMAIL", "corvid-test@example.com")
        .env("GIT_COMMITTER_NAME", "corvid-test")
        .env("GIT_COMMITTER_EMAIL", "corvid-test@example.com")
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        output.status.success(),
        "git {args:?} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

fn corvid_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_corvid"))
}

const BASE_SOURCE: &str = r#"
pub extern "c" agent greet() -> Int:
    return 1
"#;

const HEAD_SOURCE: &str = r#"
pub extern "c" agent greet() -> Int:
    return 1

pub extern "c" agent summarize() -> Int:
    return 2
"#;

fn setup_repo() -> (tempfile::TempDir, String, String) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    run_git(repo, &["init", "--quiet", "-b", "main"]);
    let src = repo.join("agent.cor");
    write_file(&src, BASE_SOURCE);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "base"]);
    let base_sha = run_git(repo, &["rev-parse", "HEAD"]);
    write_file(&src, HEAD_SOURCE);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "head"]);
    let head_sha = run_git(repo, &["rev-parse", "HEAD"]);
    (tmp, base_sha, head_sha)
}

fn derive_verifying_key_hex(seed_hex: &str) -> String {
    use ed25519_dalek::SigningKey;
    let mut seed = [0u8; 32];
    hex::decode_to_slice(seed_hex, &mut seed).expect("hex seed");
    let sk = SigningKey::from_bytes(&seed);
    hex::encode(sk.verifying_key().to_bytes())
}

const TEST_SEED_HEX: &str =
    "4242424242424242424242424242424242424242424242424242424242424242";

fn with_cache_dir<F: FnOnce(&Path) -> R, R>(f: F) -> R {
    let tmp = tempfile::tempdir().unwrap();
    let old = std::env::var("CORVID_RECEIPT_CACHE_DIR").ok();
    std::env::set_var("CORVID_RECEIPT_CACHE_DIR", tmp.path());
    let result = f(tmp.path());
    if let Some(v) = old {
        std::env::set_var("CORVID_RECEIPT_CACHE_DIR", v);
    } else {
        std::env::remove_var("CORVID_RECEIPT_CACHE_DIR");
    }
    result
}

#[test]
fn unsigned_in_toto_emits_valid_statement_v1() {
    let (repo_tmp, base_sha, head_sha) = setup_repo();
    let repo = repo_tmp.path();

    let output = Command::new(corvid_bin())
        .args([
            "trace-diff",
            &base_sha,
            &head_sha,
            "agent.cor",
            "--narrative=off",
            "--format=in-toto",
        ])
        .current_dir(repo)
        .output()
        .expect("run corvid trace-diff --format=in-toto");

    assert!(
        output.status.success(),
        "exit={:?} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
    );

    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("output is valid JSON");

    assert_eq!(parsed["_type"], "https://in-toto.io/Statement/v1");
    assert_eq!(
        parsed["predicateType"],
        "https://corvid-lang.org/attestation/receipt/v1"
    );

    // Subject shape
    let subject = &parsed["subject"][0];
    assert_eq!(subject["name"], "agent.cor");
    let digest = subject["digest"]["sha256"].as_str().expect("sha256 hex");
    assert_eq!(digest.len(), 64, "sha256 must be 64 hex chars");

    // Predicate bundles verdict + receipt
    assert!(parsed["predicate"]["verdict"].is_object());
    assert!(parsed["predicate"]["receipt"].is_object());
    assert_eq!(parsed["predicate"]["receipt"]["schema_version"], 2);
}

#[test]
fn subject_digest_matches_head_source_bytes() {
    // The subject.digest.sha256 must be the SHA-256 of the head
    // source file's bytes — the attestation is *about* that
    // specific source, and consumers who have the source can
    // verify the digest matches.
    let (repo_tmp, base_sha, head_sha) = setup_repo();
    let repo = repo_tmp.path();

    let output = Command::new(corvid_bin())
        .args([
            "trace-diff",
            &base_sha,
            &head_sha,
            "agent.cor",
            "--narrative=off",
            "--format=in-toto",
        ])
        .current_dir(repo)
        .output()
        .expect("run trace-diff");
    assert!(output.status.success());

    let parsed: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("json");
    let digest_from_statement = parsed["subject"][0]["digest"]["sha256"]
        .as_str()
        .unwrap()
        .to_string();

    // Compute the digest ourselves from the head source bytes.
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(HEAD_SOURCE);
    let expected = hex::encode(hasher.finalize());
    assert_eq!(digest_from_statement, expected);
}

#[test]
fn signed_in_toto_wraps_statement_in_dsse_with_in_toto_payload_type() {
    let (repo_tmp, base_sha, head_sha) = setup_repo();
    let repo = repo_tmp.path();

    let keys_dir = tempfile::tempdir().unwrap();
    let sign_key = keys_dir.path().join("signing.hex");
    std::fs::write(&sign_key, TEST_SEED_HEX).unwrap();

    with_cache_dir(|_cache| {
        let output = Command::new(corvid_bin())
            .args([
                "trace-diff",
                &base_sha,
                &head_sha,
                "agent.cor",
                "--narrative=off",
                "--format=in-toto",
                "--sign",
                sign_key.to_str().unwrap(),
            ])
            .current_dir(repo)
            .output()
            .expect("run corvid trace-diff --format=in-toto --sign");

        assert!(
            output.status.success(),
            "exit={:?} stderr={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr),
        );

        let envelope: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("envelope json");

        // DSSE envelope with in-toto payloadType (not Corvid's
        // native one).
        assert_eq!(
            envelope["payloadType"],
            "application/vnd.in-toto+json",
            "signed in-toto output must declare the in-toto payloadType"
        );
        assert!(envelope["signatures"].as_array().map(|a| !a.is_empty()).unwrap_or(false));

        // Decode the payload and confirm it's the Statement.
        let payload_b64 = envelope["payload"].as_str().unwrap();
        let payload_bytes = B64.decode(payload_b64).unwrap();
        let statement: serde_json::Value =
            serde_json::from_slice(&payload_bytes).unwrap();
        assert_eq!(statement["_type"], "https://in-toto.io/Statement/v1");
        assert_eq!(
            statement["predicateType"],
            "https://corvid-lang.org/attestation/receipt/v1"
        );
    });
}

#[test]
fn receipt_verify_accepts_in_toto_envelope() {
    // `corvid receipt verify` must round-trip an in-toto-shaped
    // DSSE envelope the same way it handles the Corvid-native
    // payloadType — the verify path accepts either because
    // both are known-good payload types.
    let (repo_tmp, base_sha, head_sha) = setup_repo();
    let repo = repo_tmp.path();

    let keys_dir = tempfile::tempdir().unwrap();
    let sign_key = keys_dir.path().join("signing.hex");
    let verify_key = keys_dir.path().join("verify.hex");
    std::fs::write(&sign_key, TEST_SEED_HEX).unwrap();
    std::fs::write(&verify_key, derive_verifying_key_hex(TEST_SEED_HEX)).unwrap();
    let envelope_file = keys_dir.path().join("attestation.json");

    with_cache_dir(|_cache| {
        let signed = Command::new(corvid_bin())
            .args([
                "trace-diff",
                &base_sha,
                &head_sha,
                "agent.cor",
                "--narrative=off",
                "--format=in-toto",
                "--sign",
                sign_key.to_str().unwrap(),
            ])
            .current_dir(repo)
            .output()
            .expect("sign");
        assert!(signed.status.success());
        std::fs::write(&envelope_file, &signed.stdout).unwrap();

        let verified = Command::new(corvid_bin())
            .args([
                "receipt",
                "verify",
                envelope_file.to_str().unwrap(),
                "--key",
                verify_key.to_str().unwrap(),
            ])
            .output()
            .expect("verify");
        assert!(
            verified.status.success(),
            "in-toto envelope must verify: stderr={}",
            String::from_utf8_lossy(&verified.stderr),
        );
        assert!(
            String::from_utf8_lossy(&verified.stderr).contains("signature OK"),
            "expected `signature OK`"
        );

        // The verified payload is the Statement (inner), not the
        // bare receipt. Consumers that care about the Corvid
        // receipt itself drill into `predicate.receipt`.
        let payload: serde_json::Value =
            serde_json::from_slice(&verified.stdout).expect("statement");
        assert_eq!(payload["_type"], "https://in-toto.io/Statement/v1");
    });
}

#[test]
fn corvid_native_sign_still_works_alongside_in_toto() {
    // Regression: adding in-toto support must not break the
    // existing `--format=json --sign` / `--format=markdown --sign`
    // paths that H-5-signed shipped.
    let (repo_tmp, base_sha, head_sha) = setup_repo();
    let repo = repo_tmp.path();

    let keys_dir = tempfile::tempdir().unwrap();
    let sign_key = keys_dir.path().join("signing.hex");
    let verify_key = keys_dir.path().join("verify.hex");
    std::fs::write(&sign_key, TEST_SEED_HEX).unwrap();
    std::fs::write(&verify_key, derive_verifying_key_hex(TEST_SEED_HEX)).unwrap();

    with_cache_dir(|_cache| {
        let signed = Command::new(corvid_bin())
            .args([
                "trace-diff",
                &base_sha,
                &head_sha,
                "agent.cor",
                "--narrative=off",
                "--format=json",
                "--sign",
                sign_key.to_str().unwrap(),
            ])
            .current_dir(repo)
            .output()
            .expect("sign");
        assert!(signed.status.success());

        let envelope: serde_json::Value =
            serde_json::from_slice(&signed.stdout).unwrap();

        // Not in-toto — the Corvid-native payloadType survives.
        assert_eq!(
            envelope["payloadType"],
            "application/vnd.corvid-receipt+json",
            "non-in-toto --sign must keep Corvid-native payloadType"
        );
    });
}
