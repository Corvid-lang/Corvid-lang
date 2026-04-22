//! End-to-end integration test for `corvid trace-diff`.
//!
//! Builds a tiny git repository in a tempdir with two commits, each
//! carrying a different `.cor` source, then invokes the compiled
//! `corvid` binary with `trace-diff <base-sha> <head-sha> <path>`
//! and asserts the receipt contains the expected algebra-delta
//! sections. Unit tests in `crates/corvid-cli/src/trace_diff/mod.rs`
//! cover the reviewer in isolation; this test covers the git-
//! integration path.

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
        "git {args:?} failed: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
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
    // `target/debug/corvid[.exe]` assembled by cargo for the binary
    // crate. `env!("CARGO_BIN_EXE_corvid")` resolves to the right
    // path in integration tests.
    PathBuf::from(env!("CARGO_BIN_EXE_corvid"))
}

// `corvid trace-diff` compares the AI-safety surface of a program
// between two commits. That surface is the set of `pub extern "c"`
// agents (the 22-B ABI descriptor's scope). Private helpers don't
// appear in the receipt — they aren't the interface the host sees.
const BASE_SOURCE: &str = r#"
pub extern "c" agent greet() -> Int:
    return 1
"#;

const HEAD_SOURCE_WITH_ADDED_AGENT: &str = r#"
pub extern "c" agent greet() -> Int:
    return 1

pub extern "c" agent summarize() -> Int:
    return 2
"#;

#[test]
fn trace_diff_end_to_end_reports_added_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    run_git(repo, &["init", "--quiet", "-b", "main"]);

    let src = repo.join("agent.cor");

    write_file(&src, BASE_SOURCE);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "base"]);
    let base_sha = run_git(repo, &["rev-parse", "HEAD"]);

    write_file(&src, HEAD_SOURCE_WITH_ADDED_AGENT);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "head"]);
    let head_sha = run_git(repo, &["rev-parse", "HEAD"]);

    let output = Command::new(corvid_bin())
        .args(["trace-diff", &base_sha, &head_sha, "agent.cor", "--format=markdown"])
        .current_dir(repo)
        .output()
        .expect("run corvid trace-diff");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "trace-diff failed: exit={:?} stdout=\n{stdout}\nstderr=\n{stderr}",
        output.status.code()
    );

    assert!(
        stdout.contains("# PR Behavior Receipt"),
        "receipt header missing. stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("Added") && stdout.contains("summarize"),
        "added-agent section missing. stdout:\n{stdout}"
    );
}

/// `--narrative=off` must produce a byte-deterministic receipt
/// (no LLM call, no adapter probe, no stderr narrative warning).
/// Used by CI for reproducible gating; exercised here to prove the
/// H-4 wire path respects the opt-out.
#[test]
fn trace_diff_narrative_off_emits_boilerplate_deterministically() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    run_git(repo, &["init", "--quiet", "-b", "main"]);

    let src = repo.join("agent.cor");
    write_file(&src, BASE_SOURCE);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "base"]);
    let base_sha = run_git(repo, &["rev-parse", "HEAD"]);
    write_file(&src, HEAD_SOURCE_WITH_ADDED_AGENT);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "head"]);
    let head_sha = run_git(repo, &["rev-parse", "HEAD"]);

    let run = |_: u32| {
        Command::new(corvid_bin())
            .args([
                "trace-diff",
                &base_sha,
                &head_sha,
                "agent.cor",
                "--narrative=off",
                "--format=markdown",
            ])
            .current_dir(repo)
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("OPENAI_API_KEY")
            .env_remove("CORVID_MODEL")
            .output()
            .expect("run corvid trace-diff --narrative=off")
    };

    let first = run(0);
    let second = run(1);
    assert!(first.status.success(), "first run failed");
    assert!(second.status.success(), "second run failed");

    let stdout1 = String::from_utf8_lossy(&first.stdout).into_owned();
    let stdout2 = String::from_utf8_lossy(&second.stdout).into_owned();
    assert_eq!(
        stdout1, stdout2,
        "`--narrative=off` must be byte-deterministic across reruns"
    );

    // Receipt must show the pre-H-4 boilerplate header (no prose).
    assert!(
        stdout1.contains("Comparing base vs. head along Corvid's effect algebra."),
        "boilerplate header missing. stdout:\n{stdout1}"
    );

    // `--narrative=off` must not touch the adapter path, so
    // stderr stays clean of any narrative-rejection messages.
    let stderr1 = String::from_utf8_lossy(&first.stderr);
    assert!(
        !stderr1.contains("narrative rejected"),
        "--narrative=off should never hit the validator: {stderr1}"
    );
}

/// `--narrative=on` with no adapter configured must hard-fail
/// with a specific error pointing at the missing env vars. Under
/// `auto` the absence is silent; under `on` the caller opted in
/// and deserves a typed failure.
#[test]
fn trace_diff_narrative_on_without_adapter_hard_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    run_git(repo, &["init", "--quiet", "-b", "main"]);

    let src = repo.join("agent.cor");
    write_file(&src, BASE_SOURCE);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "base"]);
    let base_sha = run_git(repo, &["rev-parse", "HEAD"]);
    write_file(&src, HEAD_SOURCE_WITH_ADDED_AGENT);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "head"]);
    let head_sha = run_git(repo, &["rev-parse", "HEAD"]);

    let output = Command::new(corvid_bin())
        .args([
            "trace-diff",
            &base_sha,
            &head_sha,
            "agent.cor",
            "--narrative=on",
        ])
        .current_dir(repo)
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("CORVID_MODEL")
        .output()
        .expect("run corvid trace-diff --narrative=on");

    assert!(
        !output.status.success(),
        "expected non-zero exit with --narrative=on and no adapter"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--narrative=on requires an LLM adapter"),
        "expected typed adapter-missing error, got stderr:\n{stderr}"
    );
}

#[test]
fn trace_diff_end_to_end_reports_no_changes_when_source_is_identical() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    run_git(repo, &["init", "--quiet", "-b", "main"]);

    let src = repo.join("agent.cor");
    write_file(&src, BASE_SOURCE);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "base"]);
    let base_sha = run_git(repo, &["rev-parse", "HEAD"]);

    // Touch an unrelated file and make a second commit whose content
    // for agent.cor is unchanged.
    write_file(&repo.join("README.md"), "docs");
    run_git(repo, &["add", "README.md"]);
    run_git(repo, &["commit", "--quiet", "-m", "docs"]);
    let head_sha = run_git(repo, &["rev-parse", "HEAD"]);

    let output = Command::new(corvid_bin())
        .args(["trace-diff", &base_sha, &head_sha, "agent.cor", "--format=markdown"])
        .current_dir(repo)
        .output()
        .expect("run corvid trace-diff");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "trace-diff failed");
    assert!(
        stdout.contains("No algebraic changes detected"),
        "no-change message missing. stdout:\n{stdout}"
    );
}

// H-3 fixture: real Corvid source that compiles to an ABI with an
// `IssueRefund` approval label and an un-grounded `answer` agent.
// The head version adds a second approval label (`WireTransfer`) to
// `refund_bot` and promotes `answer`'s return to `Grounded<String>`
// via `cite_source`. Both changes are exactly what 21-inv-H-3 must
// surface in the receipt.
// Base source: one `pub extern "c"` agent `refund_bot` that already
// needs `IssueRefund` approval; one internal helper `explain` whose
// return is *not* `Grounded<T>`.
const H3_BASE_SOURCE: &str = r#"
effect retrieval:
    data: grounded

tool get_order(id: String) -> String
tool cite_source(q: String) -> Grounded<String> uses retrieval
tool issue_refund(id: String) -> String dangerous

prompt explain_prompt(q: String) -> String:
    "explain {q}"

agent explain(q: String) -> String:
    return explain_prompt(q)

pub extern "c"
agent refund_bot(id: String) -> String:
    order = get_order(id)
    note = explain(order)
    approve IssueRefund(order)
    return issue_refund(order)
"#;

// Head source — two differences vs base:
//   1. `refund_bot` gains a second approve site (`SendNotice`), a
//      non-dangerous approval label on the same exported agent.
//   2. `explain` is strengthened to return `Grounded<String>` via a
//      `cite_source` dependency — the reviewer must flag the
//      provenance upgrade on the helper agent.
const H3_HEAD_SOURCE: &str = r#"
effect retrieval:
    data: grounded

tool get_order(id: String) -> String
tool cite_source(q: String) -> Grounded<String> uses retrieval
tool issue_refund(id: String) -> String dangerous

prompt explain_prompt(q: Grounded<String>) -> Grounded<String>:
    "explain {q}"

agent explain(q: String) -> Grounded<String>:
    source = cite_source(q)
    return explain_prompt(source)

pub extern "c"
agent refund_bot(id: String) -> String:
    order = get_order(id)
    note = explain(order)
    approve IssueRefund(order)
    approve SendNotice(order)
    return issue_refund(order)
"#;

#[test]
fn trace_diff_reports_added_approval_label_and_grounded_promotion() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    run_git(repo, &["init", "--quiet", "-b", "main"]);

    let src = repo.join("app.cor");
    write_file(&src, H3_BASE_SOURCE);
    run_git(repo, &["add", "app.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "base"]);
    let base_sha = run_git(repo, &["rev-parse", "HEAD"]);

    write_file(&src, H3_HEAD_SOURCE);
    run_git(repo, &["add", "app.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "head"]);
    let head_sha = run_git(repo, &["rev-parse", "HEAD"]);

    let output = Command::new(corvid_bin())
        .args(["trace-diff", &base_sha, &head_sha, "app.cor", "--format=markdown"])
        .current_dir(repo)
        .output()
        .expect("run corvid trace-diff");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "trace-diff failed: exit={:?} stdout=\n{stdout}\nstderr=\n{stderr}",
        output.status.code()
    );
    // New approval label on an existing agent:
    assert!(
        stdout.contains("approve site `SendNotice` added"),
        "receipt missing added-approve-label detection. stdout:\n{stdout}"
    );
    // Strengthened provenance on an existing agent:
    assert!(
        stdout.contains("return value gained `Grounded<T>` provenance"),
        "receipt missing grounded-gained detection. stdout:\n{stdout}"
    );
}

/// `--traces <dir>` on an empty directory must (a) parse, (b) reach
/// the counterfactual-replay subsystem, (c) cleanly report that no
/// traces were found, and (d) render the receipt without an impact
/// section. Exercises the H-2 wire path end-to-end without needing a
/// recorded fixture.
#[test]
fn trace_diff_with_empty_traces_dir_renders_no_impact_section() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    run_git(repo, &["init", "--quiet", "-b", "main"]);

    let src = repo.join("agent.cor");
    write_file(&src, BASE_SOURCE);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "only"]);
    let base_sha = run_git(repo, &["rev-parse", "HEAD"]);
    let head_sha = base_sha.clone();

    let traces_dir = repo.join("empty_traces");
    std::fs::create_dir_all(&traces_dir).unwrap();

    let output = Command::new(corvid_bin())
        .args([
            "trace-diff",
            &base_sha,
            &head_sha,
            "agent.cor",
            "--traces",
            traces_dir.to_str().unwrap(),
            "--format=markdown",
        ])
        .current_dir(repo)
        .output()
        .expect("run corvid trace-diff --traces");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "trace-diff --traces failed: exit={:?} stdout=\n{stdout}\nstderr=\n{stderr}",
        output.status.code()
    );
    assert!(
        stdout.contains("# PR Behavior Receipt"),
        "receipt header missing. stdout:\n{stdout}"
    );
    // The empty-dir branch sets `has_traces = false` so the reviewer
    // renders zero content for the impact section — slice-1 receipts
    // are unchanged by a `--traces` pointing at an empty dir.
    assert!(
        !stdout.contains("Counterfactual Replay Impact"),
        "empty traces dir must not render an impact section. stdout:\n{stdout}"
    );
}

/// `--traces <dir>` pointed at a non-existent path must fail cleanly
/// with a typed error that names the directory, not an opaque panic.
#[test]
fn trace_diff_with_missing_traces_dir_errors_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    run_git(repo, &["init", "--quiet", "-b", "main"]);

    let src = repo.join("agent.cor");
    write_file(&src, BASE_SOURCE);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "only"]);
    let base_sha = run_git(repo, &["rev-parse", "HEAD"]);

    let missing_dir = repo.join("does_not_exist");

    let output = Command::new(corvid_bin())
        .args([
            "trace-diff",
            &base_sha,
            &base_sha,
            "agent.cor",
            "--traces",
            missing_dir.to_str().unwrap(),
        ])
        .current_dir(repo)
        .output()
        .expect("run corvid trace-diff --traces missing-dir");

    assert!(
        !output.status.success(),
        "expected non-zero exit for missing --traces dir"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does not exist"),
        "expected stderr to name the missing dir, got:\n{stderr}"
    );
}

#[test]
fn trace_diff_errors_cleanly_when_base_sha_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    run_git(repo, &["init", "--quiet", "-b", "main"]);
    let src = repo.join("agent.cor");
    write_file(&src, BASE_SOURCE);
    run_git(repo, &["add", "agent.cor"]);
    run_git(repo, &["commit", "--quiet", "-m", "only"]);
    let head_sha = run_git(repo, &["rev-parse", "HEAD"]);

    let output = Command::new(corvid_bin())
        .args(["trace-diff", "deadbeef000000000000000000000000", &head_sha, "agent.cor"])
        .current_dir(repo)
        .output()
        .expect("run corvid trace-diff");

    assert!(
        !output.status.success(),
        "expected non-zero exit for unknown base sha"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("git show") || stderr.contains("fetching"),
        "expected stderr to mention the git failure, got:\n{stderr}"
    );
}
