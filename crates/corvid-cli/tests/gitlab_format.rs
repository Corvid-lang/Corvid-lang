//! End-to-end integration tests for `--format=gitlab`.
//!
//! Verifies the output is a valid CodeClimate-compatible JSON
//! array (the shape GitLab CI consumes via
//! `artifacts.reports.codequality`), that severity tracks the
//! default regression policy, and that fingerprints stay stable
//! across runs so the GitLab MR widget dedupes issues on
//! pipeline re-runs.

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

const BASE_SOURCE: &str = r#"
pub extern "c" agent greet() -> Int:
    return 1
"#;

/// Head adds a new agent — this is a non-regression delta
/// (`agent.added:*`), so the default policy stays green and
/// every resulting issue gets `info` severity.
const HEAD_ADDED: &str = r#"
pub extern "c" agent greet() -> Int:
    return 1

pub extern "c" agent summarize() -> Int:
    return 2
"#;

/// Head adds a dangerous tool and routes the agent through it.
/// The ABI derives `dangerous = true` transitively; the default
/// policy flags this as a `dangerous_gained` regression, so
/// severity should escalate to `major`.
const HEAD_DANGEROUS: &str = r#"
tool issue_refund(id: String) -> Int dangerous

pub extern "c" agent greet() -> Int:
    approve IssueRefund("t1")
    return issue_refund("t1")
"#;

fn setup_repo(head_source: &str) -> (tempfile::TempDir, String, String) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    run_git(repo, &["init", "--quiet", "-b", "main"]);
    let src = repo.join("agent.cor");
    write_file(&src, BASE_SOURCE);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "base"]);
    let base_sha = run_git(repo, &["rev-parse", "HEAD"]);
    write_file(&src, head_source);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "head"]);
    let head_sha = run_git(repo, &["rev-parse", "HEAD"]);
    (tmp, base_sha, head_sha)
}

fn run_gitlab(repo: &Path, base_sha: &str, head_sha: &str) -> std::process::Output {
    Command::new(corvid_bin())
        .args([
            "trace-diff",
            base_sha,
            head_sha,
            "agent.cor",
            "--narrative=off",
            "--format=gitlab",
        ])
        .current_dir(repo)
        .output()
        .expect("run corvid trace-diff --format=gitlab")
}

#[test]
fn gitlab_format_emits_codeclimate_json_array() {
    let (repo_tmp, base_sha, head_sha) = setup_repo(HEAD_ADDED);
    let repo = repo_tmp.path();

    let output = run_gitlab(repo, &base_sha, &head_sha);
    assert!(
        output.status.success(),
        "exit={:?} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
    );

    let parsed: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output is valid JSON");
    assert!(parsed.is_array(), "GitLab codequality is a JSON array");

    let issues = parsed.as_array().unwrap();
    assert!(!issues.is_empty(), "at least one issue for the added agent");

    // Every issue must carry the five CodeClimate-required fields
    // GitLab pulls out on the MR Changes tab.
    for issue in issues {
        assert!(issue["description"].is_string());
        assert!(issue["check_name"].is_string());
        assert!(issue["fingerprint"].is_string());
        assert!(issue["severity"].is_string());
        assert!(issue["location"]["path"].is_string());
        assert!(issue["location"]["lines"]["begin"].is_number());
    }
}

#[test]
fn non_regression_deltas_use_info_severity() {
    // Adding an agent is an improvement under the default policy;
    // the gate stays green (exit 0) and the issue is `info`, not
    // `major`. This keeps additive-PR noise out of the MR widget.
    let (repo_tmp, base_sha, head_sha) = setup_repo(HEAD_ADDED);
    let repo = repo_tmp.path();

    let output = run_gitlab(repo, &base_sha, &head_sha);
    assert_eq!(
        output.status.code(),
        Some(0),
        "additive diff should not trip the gate"
    );

    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let issues = parsed.as_array().unwrap();
    for issue in issues {
        assert_eq!(
            issue["severity"].as_str().unwrap(),
            "info",
            "non-regression delta must be `info`, got issue={issue}",
        );
    }
}

#[test]
fn regression_deltas_use_major_severity() {
    // Flipping an agent to `@dangerous` is a safety regression;
    // the default policy trips (exit 1) and the issue severity
    // escalates to `major` so GitLab surfaces it as a blocking
    // finding in the MR widget.
    let (repo_tmp, base_sha, head_sha) = setup_repo(HEAD_DANGEROUS);
    let repo = repo_tmp.path();

    let output = run_gitlab(repo, &base_sha, &head_sha);
    assert_eq!(
        output.status.code(),
        Some(1),
        "dangerous_gained must trip the default policy"
    );

    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let issues = parsed.as_array().unwrap();
    assert!(!issues.is_empty());
    assert!(
        issues
            .iter()
            .any(|i| i["severity"].as_str() == Some("major")),
        "at least one issue must be `major` severity for a regression"
    );
}

#[test]
fn every_issue_uses_the_stable_check_name() {
    // All Corvid findings share one `check_name` so GitLab's MR
    // widget groups them under a single rule. Hardcoded here so
    // regressions on the constant show up loudly.
    let (repo_tmp, base_sha, head_sha) = setup_repo(HEAD_ADDED);
    let repo = repo_tmp.path();

    let output = run_gitlab(repo, &base_sha, &head_sha);
    assert!(output.status.success());

    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for issue in parsed.as_array().unwrap() {
        assert_eq!(issue["check_name"], "corvid.trace-diff");
    }
}

#[test]
fn fingerprints_are_stable_across_runs() {
    // Regression guard for the GitLab MR widget: if fingerprints
    // drift between pipeline runs, the widget shows phantom "new"
    // findings on every re-run and drowns reviewers. This test
    // runs the exact same PR twice and asserts byte-identical
    // output.
    let (repo_tmp, base_sha, head_sha) = setup_repo(HEAD_ADDED);
    let repo = repo_tmp.path();

    let run1 = run_gitlab(repo, &base_sha, &head_sha);
    let run2 = run_gitlab(repo, &base_sha, &head_sha);
    assert!(run1.status.success() && run2.status.success());
    assert_eq!(
        run1.stdout, run2.stdout,
        "byte-identical output across pipeline runs"
    );
}

#[test]
fn gitlab_ci_env_var_auto_selects_gitlab_format() {
    // `--format=auto` under `GITLAB_CI=true` must pick the
    // gitlab renderer. This is the UX promise: a user dropping
    // `corvid trace-diff ...` into a GitLab job without a
    // `--format` flag gets codequality JSON on stdout, ready to
    // redirect into `gl-code-quality-report.json`.
    let (repo_tmp, base_sha, head_sha) = setup_repo(HEAD_ADDED);
    let repo = repo_tmp.path();

    let output = Command::new(corvid_bin())
        .args([
            "trace-diff",
            &base_sha,
            &head_sha,
            "agent.cor",
            "--narrative=off",
            "--format=auto",
        ])
        .current_dir(repo)
        .env("GITLAB_CI", "true")
        // Clear competing env vars that might outrank GITLAB_CI
        // in the detection precedence.
        .env_remove("GITHUB_ACTIONS")
        .output()
        .expect("run auto-format under GITLAB_CI");

    assert!(
        output.status.success(),
        "exit={:?} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
    );

    // Auto-detected gitlab output is a JSON array (not the
    // markdown or github-check shape).
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        parsed.is_array(),
        "GITLAB_CI=true with --format=auto must produce a JSON array (codequality)"
    );
}
