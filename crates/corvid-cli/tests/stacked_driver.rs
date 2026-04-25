//! End-to-end integration tests for `corvid trace-diff --stack`.
//!
//! Builds a real git repository with a sequence of commits, then
//! drives the CLI across the commit range and asserts on the
//! emitted `StackReceipt` JSON. Exercises the algebra composer
//! through the full driver path: `--stack` flag parsing, git log
//! walk, per-commit source fetch + compile + diff, composition,
//! JSON emission.

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

/// Build a tempdir git repo with a sequence of commits, each
/// writing the given `agent.cor` contents. Returns the repo path
/// and the commit SHAs for each stage (including base at index 0).
fn setup_stack(stages: &[&str]) -> (tempfile::TempDir, Vec<String>) {
    assert!(
        stages.len() >= 2,
        "need at least base + one head commit for a meaningful stack"
    );
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    run_git(repo, &["init", "--quiet", "-b", "main"]);

    let src = repo.join("agent.cor");
    let mut shas = Vec::with_capacity(stages.len());
    for (i, stage) in stages.iter().enumerate() {
        write_file(&src, stage);
        run_git(repo, &["add", "agent.cor"]);
        let msg = format!("commit-{i}");
        run_git(repo, &["commit", "--quiet", "-m", &msg]);
        shas.push(run_git(repo, &["rev-parse", "HEAD"]));
    }
    (tmp, shas)
}

const BASE_SOURCE: &str = r#"
pub extern "c" agent greet() -> Int:
    return 1
"#;

const ADD_FOO_SOURCE: &str = r#"
pub extern "c" agent greet() -> Int:
    return 1

pub extern "c" agent foo() -> Int:
    return 2
"#;

const DANGEROUS_TOOL_SOURCE: &str = r#"
tool issue_refund(id: String) -> Int dangerous

pub extern "c" agent greet() -> Int:
    return 1

pub extern "c" agent foo() -> Int:
    approve IssueRefund("t1")
    return issue_refund("t1")
"#;

const ALLOW_ALL_POLICY: &str = r#"
@deterministic
agent apply_policy(receipt: PolicyReceipt) -> Verdict:
    return Verdict(true, [])
"#;

#[test]
fn stacked_json_composes_over_commit_range() {
    // Three commits: base → adds foo → makes foo dangerous.
    // The stack contains two per-commit diffs:
    //   c1: agent.added:foo
    //   c2: agent.dangerous_gained:foo
    let (repo_tmp, shas) = setup_stack(&[BASE_SOURCE, ADD_FOO_SOURCE, DANGEROUS_TOOL_SOURCE]);
    let repo = repo_tmp.path();
    let base = &shas[0];
    let head = &shas[2];

    let output = Command::new(corvid_bin())
        .args([
            "trace-diff",
            base,
            head,
            "agent.cor",
            "--narrative=off",
            "--format=json",
            "--stack",
        ])
        .current_dir(repo)
        .output()
        .expect("run corvid trace-diff --stack");

    assert_eq!(
        output.status.code(),
        Some(1),
        "dangerous regression in stack history should trip aggregate policy; stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );

    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("output is valid JSON");
    assert_eq!(parsed["verdict"]["ok"], false);
    assert!(
        parsed["verdict"]["flags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|flag| flag.as_str().unwrap().contains("@dangerous")),
        "stack verdict should include the dangerous regression flag: {}",
        serde_json::to_string_pretty(&parsed["verdict"]).unwrap()
    );

    // StackReceipt top-level shape.
    assert_eq!(parsed["schema_version"], 2);
    assert_eq!(parsed["base_sha"], base.as_str());
    assert_eq!(parsed["head_sha"], head.as_str());
    assert_eq!(parsed["source_path"], "agent.cor");
    assert_eq!(parsed["stack_hash"].as_str().unwrap().len(), 64);

    // Two per-commit components.
    let components = parsed["components"].as_array().unwrap();
    assert_eq!(components.len(), 2);
    assert_eq!(components[0]["commit_sha"], shas[1].as_str());
    assert_eq!(components[1]["commit_sha"], shas[2].as_str());

    // History view preserves every per-commit delta in order.
    let history = parsed["history"].as_array().unwrap();
    assert!(!history.is_empty(), "history view must not be empty");

    // At least one delta in normal_form — agent foo was added and
    // gained dangerous; nothing cancels over this range.
    let normal_form = parsed["normal_form"].as_array().unwrap();
    assert!(
        !normal_form.is_empty(),
        "net delta set should be non-empty for add+make-dangerous"
    );
    // Every surviving delta must carry introduced_at.
    for delta in normal_form {
        let introduced_at = delta["introduced_at"].as_str().unwrap();
        assert!(
            shas.contains(&introduced_at.to_string()),
            "introduced_at `{introduced_at}` must point at one of the stack's commits"
        );
    }
}

#[test]
fn stacked_round_trip_empties_normal_form() {
    // Three commits: base → adds foo → removes foo again.
    // Algebraically: add+remove cancels. Normal form should be
    // empty; history should carry both deltas.
    let (repo_tmp, shas) = setup_stack(&[BASE_SOURCE, ADD_FOO_SOURCE, BASE_SOURCE]);
    let repo = repo_tmp.path();

    let output = Command::new(corvid_bin())
        .args([
            "trace-diff",
            &shas[0],
            &shas[2],
            "agent.cor",
            "--narrative=off",
            "--format=json",
            "--stack",
        ])
        .current_dir(repo)
        .output()
        .expect("run corvid trace-diff --stack");

    assert!(
        output.status.success(),
        "exit={:?} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
    );

    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let normal_form = parsed["normal_form"].as_array().unwrap();
    assert!(
        normal_form.is_empty(),
        "add+remove should cancel to identity; got {normal_form:?}"
    );
    let history = parsed["history"].as_array().unwrap();
    assert_eq!(
        history.len(),
        2,
        "history must preserve both intermediate deltas"
    );
}

#[test]
fn stacked_rejects_sign_flag_in_v1() {
    // --stack with --sign should return a typed error in step 2/N.
    // Signing integration lands with the Merkle-signing commit later
    // in the slice.
    let (repo_tmp, shas) = setup_stack(&[BASE_SOURCE, ADD_FOO_SOURCE]);
    let repo = repo_tmp.path();

    let keys_dir = tempfile::tempdir().unwrap();
    let key_path = keys_dir.path().join("signing.hex");
    std::fs::write(
        &key_path,
        "4242424242424242424242424242424242424242424242424242424242424242",
    )
    .unwrap();

    let output = Command::new(corvid_bin())
        .args([
            "trace-diff",
            &shas[0],
            &shas[1],
            "agent.cor",
            "--narrative=off",
            "--format=json",
            "--stack",
            "--sign",
            key_path.to_str().unwrap(),
        ])
        .current_dir(repo)
        .output()
        .expect("run corvid trace-diff --stack --sign");

    assert!(!output.status.success(), "stack+sign must fail in step 2/N");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Merkle signing"),
        "error must point at the later-commit signing path; got: {stderr}"
    );
}

#[test]
fn stacked_rejects_non_json_format_in_v1() {
    // Step 2/N currently emits JSON only. Markdown / github-check /
    // gitlab / in-toto arms unlock with the renderer-lift commit.
    let (repo_tmp, shas) = setup_stack(&[BASE_SOURCE, ADD_FOO_SOURCE]);
    let repo = repo_tmp.path();

    let output = Command::new(corvid_bin())
        .args([
            "trace-diff",
            &shas[0],
            &shas[1],
            "agent.cor",
            "--narrative=off",
            "--format=gitlab",
            "--stack",
        ])
        .current_dir(repo)
        .output()
        .expect("run corvid trace-diff --stack --format=gitlab");

    assert!(
        !output.status.success(),
        "non-JSON format must fail in step 2/N"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--format=json"),
        "error must name the currently-supported format"
    );
}

#[test]
fn stacked_rejects_empty_range() {
    // Range where base == head has no commits in it.
    let (repo_tmp, shas) = setup_stack(&[BASE_SOURCE, ADD_FOO_SOURCE]);
    let repo = repo_tmp.path();

    let output = Command::new(corvid_bin())
        .args([
            "trace-diff",
            &shas[1],
            &shas[1],
            "agent.cor",
            "--narrative=off",
            "--format=json",
            "--stack",
        ])
        .current_dir(repo)
        .output()
        .expect("run corvid trace-diff --stack on empty range");

    assert!(
        !output.status.success(),
        "empty commit range must be an error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("zero commits"),
        "error message must name the empty-range cause; got: {stderr}"
    );
}

#[test]
fn stacked_explicit_list_spec_uses_provided_shas() {
    // `--stack=<sha1>,<sha2>` should compose over the explicit
    // list, bypassing `git log`.
    let (repo_tmp, shas) = setup_stack(&[BASE_SOURCE, ADD_FOO_SOURCE, DANGEROUS_TOOL_SOURCE]);
    let repo = repo_tmp.path();
    let explicit = format!("{},{}", shas[1], shas[2]);

    let output = Command::new(corvid_bin())
        .args([
            "trace-diff",
            &shas[0],
            &shas[2],
            "agent.cor",
            "--narrative=off",
            "--format=json",
            &format!("--stack={explicit}"),
        ])
        .current_dir(repo)
        .output()
        .expect("run corvid trace-diff --stack=<list>");

    assert_eq!(
        output.status.code(),
        Some(1),
        "explicit stack contains a dangerous regression and should trip policy; stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );

    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let components = parsed["components"].as_array().unwrap();
    assert_eq!(components.len(), 2);
    // range_spec reflects the explicit list so re-runs with the
    // same set produce the same stack hash.
    assert!(parsed["range_spec"]
        .as_str()
        .unwrap()
        .contains(&shas[1])
        && parsed["range_spec"]
            .as_str()
            .unwrap()
            .contains(&shas[2]));
}

#[test]
fn stacked_custom_policy_can_allow_history_regression() {
    let (repo_tmp, shas) = setup_stack(&[BASE_SOURCE, ADD_FOO_SOURCE, DANGEROUS_TOOL_SOURCE]);
    let repo = repo_tmp.path();
    let policy = repo.join("allow_stack.cor");
    write_file(&policy, ALLOW_ALL_POLICY);

    let output = Command::new(corvid_bin())
        .args([
            "trace-diff",
            &shas[0],
            &shas[2],
            "agent.cor",
            "--narrative=off",
            "--format=json",
            "--stack",
            "--policy",
            policy.to_str().unwrap(),
        ])
        .current_dir(repo)
        .output()
        .expect("run corvid trace-diff --stack --policy");

    assert!(
        output.status.success(),
        "custom allow policy should pass stack receipt; stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["verdict"]["ok"], true);
    assert!(
        parsed["history"]
            .as_array()
            .unwrap()
            .iter()
            .any(|delta| delta["key"]
                .as_str()
                .unwrap()
                .contains("dangerous_gained")),
        "custom policy must not erase archived regression history"
    );
}

#[test]
fn stacked_with_empty_traces_dir_lifts_the_step2_ban_and_emits_no_attributions() {
    // Step 2/N blocked `--stack --traces <dir>` with a typed error.
    // Step 3b/N lifts that ban; an empty traces directory must now
    // produce a clean exit with no attribution records (the receipt
    // is still a valid algebra-only StackReceipt).
    let (repo_tmp, shas) = setup_stack(&[BASE_SOURCE, ADD_FOO_SOURCE]);
    let repo = repo_tmp.path();
    let traces = tempfile::tempdir().unwrap();

    let output = Command::new(corvid_bin())
        .args([
            "trace-diff",
            &shas[0],
            &shas[1],
            "agent.cor",
            "--narrative=off",
            "--format=json",
            "--stack",
            "--traces",
            traces.path().to_str().unwrap(),
        ])
        .current_dir(repo)
        .output()
        .expect("run corvid trace-diff --stack --traces <empty>");

    assert!(
        output.status.success(),
        "empty traces dir must not error; exit={:?} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
    );

    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    // Algebra-only receipt shape preserved; no `attributions` key
    // because the vector is empty (skip_serializing_if kicks in).
    assert!(
        parsed.get("attributions").is_none()
            || parsed["attributions"].as_array().map(|a| a.is_empty()).unwrap_or(false),
        "empty traces corpus must produce no attributions; got: {}",
        serde_json::to_string_pretty(&parsed).unwrap()
    );
}

#[test]
fn stacked_with_missing_traces_dir_errors_cleanly() {
    // `--traces <missing>` must surface a structured error (not a
    // panic, not a silent no-op).
    let (repo_tmp, shas) = setup_stack(&[BASE_SOURCE, ADD_FOO_SOURCE]);
    let repo = repo_tmp.path();

    let output = Command::new(corvid_bin())
        .args([
            "trace-diff",
            &shas[0],
            &shas[1],
            "agent.cor",
            "--narrative=off",
            "--format=json",
            "--stack",
            "--traces",
            "/nonexistent/path/that/does/not/exist",
        ])
        .current_dir(repo)
        .output()
        .expect("run corvid trace-diff --stack --traces <missing>");

    assert!(
        !output.status.success(),
        "missing traces dir must be an error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does not exist") || stderr.contains("nonexistent"),
        "error must name the missing-dir cause; got: {stderr}"
    );
}

#[test]
fn stacked_json_is_byte_stable_across_runs() {
    // Same inputs → byte-identical JSON. Regression guard so
    // downstream consumers (cache, renderers) can trust stability.
    let (repo_tmp, shas) = setup_stack(&[BASE_SOURCE, ADD_FOO_SOURCE]);
    let repo = repo_tmp.path();

    let run = || {
        let output = Command::new(corvid_bin())
            .args([
                "trace-diff",
                &shas[0],
                &shas[1],
                "agent.cor",
                "--narrative=off",
                "--format=json",
                "--stack",
            ])
            .current_dir(repo)
            .output()
            .expect("run corvid trace-diff --stack");
        assert!(output.status.success());
        output.stdout
    };
    assert_eq!(run(), run(), "byte-identical across re-runs");
}
