//! End-to-end integration tests for `corvid trace-diff --sign` +
//! `corvid receipt verify`.
//!
//! Exercises the full cryptographic path: generate a deterministic
//! ed25519 keypair, write the seed + public key to temp files, run
//! the CLI to produce a DSSE envelope, run the CLI again to verify
//! it, and assert the receipt round-trips bit-identical.
//!
//! Deterministic keys (from a fixed 32-byte seed) mean the tests
//! fail deterministically and don't depend on the host's RNG.

use std::path::{Path, PathBuf};
use std::process::Command;

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

/// Deterministic ed25519 seed used for every test (32 bytes, all
/// `0x42`). Tests derive the verifying key from this seed at
/// runtime via `ed25519_dalek::SigningKey::from_bytes(...).verifying_key()`,
/// so the test file doesn't have to embed a cryptographic
/// invariant that would drift if the derivation ever changed.
const TEST_SEED_HEX: &str =
    "4242424242424242424242424242424242424242424242424242424242424242";

fn derive_verifying_key_hex(seed_hex: &str) -> String {
    use ed25519_dalek::SigningKey;
    let mut seed = [0u8; 32];
    hex::decode_to_slice(seed_hex, &mut seed).expect("valid hex seed");
    let sk = SigningKey::from_bytes(&seed);
    hex::encode(sk.verifying_key().to_bytes())
}

const SIMPLE_SOURCE: &str = r#"
pub extern "c" agent greet() -> Int:
    return 1
"#;

const SIMPLE_SOURCE_ADDED: &str = r#"
pub extern "c" agent greet() -> Int:
    return 1

pub extern "c" agent summarize() -> Int:
    return 2
"#;

/// Set the receipt cache to an isolated temp dir so tests don't
/// touch `~/.cache/corvid/receipts`. The override is read by
/// `receipt_cache::cache_dir` via the
/// `CORVID_RECEIPT_CACHE_DIR` env var.
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
fn sign_then_verify_roundtrips_end_to_end() {
    let repo_tmp = tempfile::tempdir().unwrap();
    let repo = repo_tmp.path();
    run_git(repo, &["init", "--quiet", "-b", "main"]);

    // Commit base, then head — an added agent ensures the receipt
    // is non-empty so the signing path carries real payload.
    let src = repo.join("agent.cor");
    write_file(&src, SIMPLE_SOURCE);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "base"]);
    let base_sha = run_git(repo, &["rev-parse", "HEAD"]);
    write_file(&src, SIMPLE_SOURCE_ADDED);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "head"]);
    let head_sha = run_git(repo, &["rev-parse", "HEAD"]);

    // Write the deterministic signing + verifying keys to files
    // in the repo's tmp dir.
    let keys_dir = tempfile::tempdir().unwrap();
    let sign_key = keys_dir.path().join("signing.hex");
    let verify_key = keys_dir.path().join("verify.hex");
    std::fs::write(&sign_key, TEST_SEED_HEX).unwrap();
    std::fs::write(&verify_key, derive_verifying_key_hex(TEST_SEED_HEX)).unwrap();

    // Produce a signed envelope.
    let envelope_file = keys_dir.path().join("receipt.envelope.json");

    with_cache_dir(|_cache| {
        let signed = Command::new(corvid_bin())
            .args([
                "trace-diff",
                &base_sha,
                &head_sha,
                "agent.cor",
                "--narrative=off",
                "--sign",
                sign_key.to_str().unwrap(),
            ])
            .current_dir(repo)
            .output()
            .expect("run corvid trace-diff --sign");

        assert!(
            signed.status.success(),
            "trace-diff --sign failed: exit={:?} stdout=\n{}\nstderr=\n{}",
            signed.status.code(),
            String::from_utf8_lossy(&signed.stdout),
            String::from_utf8_lossy(&signed.stderr),
        );

        // Stderr should announce the receipt hash for downstream
        // tooling.
        let stderr = String::from_utf8_lossy(&signed.stderr);
        assert!(
            stderr.contains("Corvid-Receipt:"),
            "expected Corvid-Receipt: <hash> on stderr, got:\n{stderr}"
        );

        // The stdout envelope should parse as a DSSE envelope
        // (JSON with `payloadType`, `payload`, `signatures`).
        let stdout = String::from_utf8_lossy(&signed.stdout).into_owned();
        let envelope_json: serde_json::Value =
            serde_json::from_str(&stdout).expect("envelope is valid JSON");
        assert_eq!(
            envelope_json["payloadType"],
            "application/vnd.corvid-receipt+json"
        );
        assert!(envelope_json["payload"].is_string());
        assert!(envelope_json["signatures"].as_array().map(|a| !a.is_empty()).unwrap_or(false));

        // Write the envelope to a file so `receipt verify` can
        // pick it up via a path argument.
        std::fs::write(&envelope_file, &stdout).unwrap();

        // Verify with the matching key.
        let verified = Command::new(corvid_bin())
            .args([
                "receipt",
                "verify",
                envelope_file.to_str().unwrap(),
                "--key",
                verify_key.to_str().unwrap(),
            ])
            .output()
            .expect("run corvid receipt verify");

        assert!(
            verified.status.success(),
            "verify failed: exit={:?} stdout=\n{}\nstderr=\n{}",
            verified.status.code(),
            String::from_utf8_lossy(&verified.stdout),
            String::from_utf8_lossy(&verified.stderr),
        );

        // Stderr from verify should confirm `signature OK`.
        assert!(
            String::from_utf8_lossy(&verified.stderr).contains("signature OK"),
            "expected `signature OK` on stderr"
        );

        // Stdout from verify is the inner receipt JSON. It must
        // parse as a Corvid receipt (has `schema_version`).
        let payload: serde_json::Value =
            serde_json::from_slice(&verified.stdout).expect("inner payload is valid JSON");
        assert_eq!(payload["schema_version"], 2);
    });
}

#[test]
fn verify_rejects_envelope_signed_with_different_key() {
    let repo_tmp = tempfile::tempdir().unwrap();
    let repo = repo_tmp.path();
    run_git(repo, &["init", "--quiet", "-b", "main"]);
    let src = repo.join("agent.cor");
    write_file(&src, SIMPLE_SOURCE);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "base"]);
    let base_sha = run_git(repo, &["rev-parse", "HEAD"]);
    write_file(&src, SIMPLE_SOURCE_ADDED);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "head"]);
    let head_sha = run_git(repo, &["rev-parse", "HEAD"]);

    let keys_dir = tempfile::tempdir().unwrap();
    let sign_key = keys_dir.path().join("signing.hex");
    std::fs::write(&sign_key, TEST_SEED_HEX).unwrap();

    // Wrong verifying key — all zeros, matching a different seed.
    // This pair doesn't match the signing key, so verify must
    // fail.
    let wrong_verify_key = keys_dir.path().join("wrong.hex");
    std::fs::write(
        &wrong_verify_key,
        "3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29",
    )
    .unwrap();

    let envelope_file = keys_dir.path().join("receipt.envelope.json");

    with_cache_dir(|_cache| {
        let signed = Command::new(corvid_bin())
            .args([
                "trace-diff",
                &base_sha,
                &head_sha,
                "agent.cor",
                "--narrative=off",
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
                wrong_verify_key.to_str().unwrap(),
            ])
            .output()
            .expect("verify");

        assert!(
            !verified.status.success(),
            "verify must fail with a mismatched key"
        );
        let stderr = String::from_utf8_lossy(&verified.stderr);
        assert!(
            stderr.contains("verification failed")
                || stderr.contains("signature"),
            "expected verification-failed message, got:\n{stderr}"
        );
    });
}

#[test]
fn verify_rejects_tampered_payload() {
    let repo_tmp = tempfile::tempdir().unwrap();
    let repo = repo_tmp.path();
    run_git(repo, &["init", "--quiet", "-b", "main"]);
    let src = repo.join("agent.cor");
    write_file(&src, SIMPLE_SOURCE);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "base"]);
    let base_sha = run_git(repo, &["rev-parse", "HEAD"]);
    write_file(&src, SIMPLE_SOURCE_ADDED);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "head"]);
    let head_sha = run_git(repo, &["rev-parse", "HEAD"]);

    let keys_dir = tempfile::tempdir().unwrap();
    let sign_key = keys_dir.path().join("signing.hex");
    let verify_key = keys_dir.path().join("verify.hex");
    std::fs::write(&sign_key, TEST_SEED_HEX).unwrap();
    std::fs::write(&verify_key, derive_verifying_key_hex(TEST_SEED_HEX)).unwrap();

    let envelope_file = keys_dir.path().join("receipt.envelope.json");

    with_cache_dir(|_cache| {
        let signed = Command::new(corvid_bin())
            .args([
                "trace-diff",
                &base_sha,
                &head_sha,
                "agent.cor",
                "--narrative=off",
                "--sign",
                sign_key.to_str().unwrap(),
            ])
            .current_dir(repo)
            .output()
            .expect("sign");
        assert!(signed.status.success());

        // Tamper: replace the inner payload field with a new
        // (valid base64) but cryptographically unrelated payload.
        let mut envelope: serde_json::Value =
            serde_json::from_slice(&signed.stdout).unwrap();
        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
        envelope["payload"] =
            serde_json::Value::String(B64.encode(br#"{"tampered": true}"#));
        std::fs::write(
            &envelope_file,
            serde_json::to_string_pretty(&envelope).unwrap(),
        )
        .unwrap();

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
            !verified.status.success(),
            "tampered envelope must not verify"
        );
    });
}

#[test]
fn without_sign_flag_normal_output_is_preserved() {
    let repo_tmp = tempfile::tempdir().unwrap();
    let repo = repo_tmp.path();
    run_git(repo, &["init", "--quiet", "-b", "main"]);
    let src = repo.join("agent.cor");
    write_file(&src, SIMPLE_SOURCE);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "base"]);
    let base_sha = run_git(repo, &["rev-parse", "HEAD"]);
    write_file(&src, SIMPLE_SOURCE_ADDED);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "head"]);
    let head_sha = run_git(repo, &["rev-parse", "HEAD"]);

    // No --sign, no env var. Output should be the normal markdown
    // receipt.
    let output = Command::new(corvid_bin())
        .args([
            "trace-diff",
            &base_sha,
            &head_sha,
            "agent.cor",
            "--narrative=off",
            "--format=markdown",
        ])
        .current_dir(repo)
        .env_remove("CORVID_SIGNING_KEY")
        .output()
        .expect("run trace-diff");
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("# PR Behavior Receipt"),
        "expected markdown receipt, got:\n{stdout}"
    );
    // Must NOT be a DSSE envelope
    assert!(
        !stdout.contains("\"payloadType\""),
        "unsigned run should not emit a DSSE envelope"
    );
}

#[test]
fn receipt_show_by_hash_returns_cached_receipt() {
    let repo_tmp = tempfile::tempdir().unwrap();
    let repo = repo_tmp.path();
    run_git(repo, &["init", "--quiet", "-b", "main"]);
    let src = repo.join("agent.cor");
    write_file(&src, SIMPLE_SOURCE);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "base"]);
    let base_sha = run_git(repo, &["rev-parse", "HEAD"]);
    write_file(&src, SIMPLE_SOURCE_ADDED);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "head"]);
    let head_sha = run_git(repo, &["rev-parse", "HEAD"]);

    let keys_dir = tempfile::tempdir().unwrap();
    let sign_key = keys_dir.path().join("signing.hex");
    std::fs::write(&sign_key, TEST_SEED_HEX).unwrap();

    with_cache_dir(|cache| {
        // Sign once to populate the cache.
        let signed = Command::new(corvid_bin())
            .args([
                "trace-diff",
                &base_sha,
                &head_sha,
                "agent.cor",
                "--narrative=off",
                "--sign",
                sign_key.to_str().unwrap(),
            ])
            .current_dir(repo)
            .env("CORVID_RECEIPT_CACHE_DIR", cache)
            .output()
            .expect("sign");
        assert!(signed.status.success());

        // Extract the hash from stderr's `Corvid-Receipt: <hash>`
        // line.
        let stderr = String::from_utf8_lossy(&signed.stderr);
        let hash = stderr
            .lines()
            .find_map(|l| l.strip_prefix("Corvid-Receipt: "))
            .expect("Corvid-Receipt: <hash> on stderr")
            .trim()
            .to_string();
        assert_eq!(hash.len(), 64);

        // `corvid receipt show <full-hash>` returns the cached
        // receipt.
        let shown = Command::new(corvid_bin())
            .args(["receipt", "show", &hash])
            .env("CORVID_RECEIPT_CACHE_DIR", cache)
            .output()
            .expect("show");
        assert!(
            shown.status.success(),
            "show failed: stderr={}",
            String::from_utf8_lossy(&shown.stderr),
        );
        let payload: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
        assert_eq!(payload["schema_version"], 2);

        // Prefix lookup works too.
        let shown_prefix = Command::new(corvid_bin())
            .args(["receipt", "show", &hash[..12]])
            .env("CORVID_RECEIPT_CACHE_DIR", cache)
            .output()
            .expect("show-prefix");
        assert!(shown_prefix.status.success());
    });
}

#[test]
fn receipt_show_with_unknown_hash_errors_cleanly() {
    with_cache_dir(|cache| {
        let out = Command::new(corvid_bin())
            .args(["receipt", "show", "deadbeef1234567890abcdef"])
            .env("CORVID_RECEIPT_CACHE_DIR", cache)
            .output()
            .expect("show-missing");
        assert!(
            !out.status.success(),
            "unknown hash should error"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("no receipt") || stderr.contains("not found"),
            "expected not-found message, got:\n{stderr}"
        );
    });
}
