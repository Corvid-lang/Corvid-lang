//! `corvid trace-diff <base-sha> <head-sha> <path>` — PR behavior
//! receipt (Phase 21 slice 21-inv-H-1).
//!
//! Git-integrates at the two ends of a PR, extracts the 22-B ABI
//! descriptor from each, digests both down to a shared `Descriptor`
//! shape, and hands them to an in-repo Corvid reviewer agent that
//! walks the algebra and emits a markdown receipt.
//!
//! The reviewer itself is a Corvid program ([`reviewer.cor`]) rather
//! than a Rust helper. That is Corvid's thesis: AI-native governance
//! is a first-class programming domain with compile-time guarantees.
//! Shipping the flagship PR-review tool in Rust would soften the
//! thesis — it would be the same shortcut Python would make shipping
//! its linter in bash. The reviewer runs through the interpreter,
//! consumes `Descriptor` values produced by Rust-side digestion of
//! the raw `corvid-abi` descriptor, and returns a markdown `String`
//! that the CLI prints.
//!
//! Slice scope: added / removed agents + trust-tier / `@dangerous` /
//! `@replayable` transitions. Follow-up slices:
//!
//! - `21-inv-H-2` counterfactual replay over `--traces <dir>`.
//! - `21-inv-H-3` structured approval-contract + provenance drill-down.
//! - `21-inv-H-4` LLM-generated prose summary grounded in the algebra.
//! - `21-inv-H-5` `--format=github-check|markdown|json` outputs.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use corvid_abi::{AbiAgent, CorvidAbi};
use corvid_driver::{
    compile_to_abi_with_config, compile_to_ir, load_corvid_config_for, run_ir_with_runtime,
    Runtime,
};
use corvid_runtime::ProgrammaticApprover;
use corvid_vm::{json_to_value, Value};
use serde::Serialize;

/// The Corvid source of the reviewer agent. Baked into the binary
/// so the CLI is self-contained: no lookup path, no user
/// configuration, same reviewer behavior across every install of
/// the same `corvid` build. Users can still read + fork this file
/// in the repo (`crates/corvid-cli/src/trace_diff/reviewer.cor`).
const REVIEWER_SOURCE: &str = include_str!("reviewer.cor");

/// Parsed args for `corvid trace-diff`. Library-level callers
/// construct directly; the `corvid` binary builds this from clap.
pub struct TraceDiffArgs<'a> {
    /// Git revision for the "before" side of the diff (typically
    /// the PR base branch tip).
    pub base_sha: &'a str,
    /// Git revision for the "after" side of the diff (typically
    /// the PR head branch tip).
    pub head_sha: &'a str,
    /// Path within the repo to the single `.cor` source file to
    /// compare. Multi-file sources are a follow-up slice.
    pub source_path: &'a Path,
}

/// Run `corvid trace-diff`: fetch source at both SHAs, compile each
/// to a `CorvidAbi` descriptor, digest to `Descriptor`s, run the
/// reviewer agent, print the receipt. Returns 0 on clean execution
/// regardless of whether changes were found — the receipt itself
/// carries the verdict. Downstream CI policy-gating slices can
/// non-zero-exit based on receipt content.
pub fn run_trace_diff(args: TraceDiffArgs<'_>) -> Result<u8> {
    let base_source = git_show(args.base_sha, args.source_path)
        .with_context(|| format!("fetching `{}` at base `{}`", args.source_path.display(), args.base_sha))?;
    let head_source = git_show(args.head_sha, args.source_path)
        .with_context(|| format!("fetching `{}` at head `{}`", args.source_path.display(), args.head_sha))?;

    let config = load_corvid_config_for(args.source_path);
    let source_path_str = args.source_path.to_string_lossy().replace('\\', "/");
    let generated_at = "1970-01-01T00:00:00Z"; // stable — receipt is byte-deterministic across re-runs

    let base_abi = compile_to_abi_with_config(&base_source, &source_path_str, generated_at, config.as_ref())
        .map_err(|diags| anyhow!("base source at `{}` failed to compile: {} diagnostic(s)", args.base_sha, diags.len()))?;
    let head_abi = compile_to_abi_with_config(&head_source, &source_path_str, generated_at, config.as_ref())
        .map_err(|diags| anyhow!("head source at `{}` failed to compile: {} diagnostic(s)", args.head_sha, diags.len()))?;

    let receipt = invoke_reviewer(&base_abi, &head_abi).context("reviewer agent execution failed")?;
    print!("{receipt}");
    Ok(0)
}

/// The digested view the Corvid reviewer consumes. Mirrors the
/// `type` decls in [`reviewer.cor`] field-for-field; `json_to_value`
/// coerces through this shape at the FFI boundary. Extra fields on
/// `CorvidAbi` that the reviewer doesn't yet care about are simply
/// not included here — `json_to_value` ignores them.
#[derive(Serialize)]
struct Descriptor {
    agents: Vec<AgentSummary>,
}

#[derive(Serialize)]
struct AgentSummary {
    name: String,
    trust_tier: String,
    is_dangerous: bool,
    is_replayable: bool,
}

fn digest(abi: &CorvidAbi) -> Descriptor {
    Descriptor {
        agents: abi.agents.iter().map(digest_agent).collect(),
    }
}

fn digest_agent(agent: &AbiAgent) -> AgentSummary {
    AgentSummary {
        name: agent.name.clone(),
        trust_tier: agent
            .effects
            .trust_tier
            .clone()
            .unwrap_or_else(|| "unspecified".to_string()),
        is_dangerous: agent.attributes.dangerous,
        is_replayable: agent.attributes.replayable,
    }
}

/// Compile the in-repo reviewer source, coerce both descriptors to
/// typed `Value`s, and run `review_pr` through the interpreter.
fn invoke_reviewer(base_abi: &CorvidAbi, head_abi: &CorvidAbi) -> Result<String> {
    let reviewer_ir = compile_to_ir(REVIEWER_SOURCE)
        .map_err(|diags| anyhow!("reviewer source failed to compile: {} diagnostic(s)", diags.len()))?;

    let descriptor_type = reviewer_ir
        .types
        .iter()
        .find(|t| t.name == "Descriptor")
        .ok_or_else(|| anyhow!("reviewer source missing `Descriptor` type"))?;
    let types_by_id = reviewer_ir.types.iter().map(|t| (t.id, t)).collect();

    let base_json = serde_json::to_value(digest(base_abi))?;
    let head_json = serde_json::to_value(digest(head_abi))?;

    let expected = corvid_types::Type::Struct(descriptor_type.id);
    let base_value = json_to_value(base_json, &expected, &types_by_id)
        .map_err(|e| anyhow!("base descriptor → Value: {e:?}"))?;
    let head_value = json_to_value(head_json, &expected, &types_by_id)
        .map_err(|e| anyhow!("head descriptor → Value: {e:?}"))?;

    // The reviewer is `@deterministic` and calls no LLMs, tools, or
    // approvers. A minimal runtime with a programmatic approver (any
    // policy — it will never be consulted) is enough to satisfy the
    // interpreter's required-approver invariant.
    let runtime = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_no()))
        .build();

    let tokio_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime for reviewer")?;

    let result = tokio_rt
        .block_on(run_ir_with_runtime(
            &reviewer_ir,
            Some("review_pr"),
            vec![base_value, head_value],
            &runtime,
        ))
        .map_err(|e| anyhow!("reviewer `review_pr` failed: {e}"))?;

    match result {
        Value::String(s) => Ok(s.to_string()),
        other => Err(anyhow!(
            "reviewer `review_pr` returned non-String value: {other:?}"
        )),
    }
}

/// `git show <rev>:<path>` → file contents. Returns a typed error
/// if git isn't available, the rev doesn't exist, or the path
/// isn't tracked at that rev.
fn git_show(rev: &str, path: &Path) -> Result<String> {
    let rel = path.to_string_lossy().replace('\\', "/");
    let spec = format!("{rev}:{rel}");
    let output = Command::new("git")
        .args(["show", &spec])
        .output()
        .context("invoking `git show` (is git on PATH?)")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "`git show {spec}` failed: {}",
            stderr.trim()
        ));
    }
    String::from_utf8(output.stdout)
        .with_context(|| format!("`git show {spec}` returned non-UTF-8 content"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reviewer_source_compiles() {
        // The reviewer must be kept typecheck-clean at commit time.
        // This test fails at CI time if a refactor of corvid-types or
        // the Corvid surface breaks the reviewer without updating it.
        let ir = compile_to_ir(REVIEWER_SOURCE)
            .expect("embedded reviewer.cor compiles on every `cargo test`");
        assert!(
            ir.agents.iter().any(|a| a.name == "review_pr"),
            "reviewer IR must expose `review_pr` agent"
        );
        assert!(
            ir.types.iter().any(|t| t.name == "Descriptor"),
            "reviewer IR must expose `Descriptor` type"
        );
    }

    /// Synthesise a tiny `CorvidAbi` for testing via JSON round-trip.
    /// Cheaper than constructing `AbiAgent` field-by-field and stays
    /// resilient if future schema extensions add required fields:
    /// deserialization fills defaults where `#[serde(default)]` is set.
    fn synth_abi(agents: &[(&str, &str, bool, bool)]) -> CorvidAbi {
        let agents_json: Vec<serde_json::Value> = agents
            .iter()
            .map(|(name, trust, dangerous, replayable)| {
                serde_json::json!({
                    "name": name,
                    "symbol": name,
                    "source_span": { "start": 0, "end": 0 },
                    "params": [],
                    "return_type": { "kind": "scalar", "scalar": "Int" },
                    "effects": { "trust_tier": trust },
                    "attributes": {
                        "replayable": replayable,
                        "deterministic": false,
                        "dangerous": dangerous,
                        "pub_extern_c": false
                    },
                    "approval_contract": { "required": dangerous, "labels": [] },
                    "provenance": { "returns_grounded": false, "grounded_param_deps": [] }
                })
            })
            .collect();
        let json = serde_json::json!({
            "corvid_abi_version": corvid_abi::CORVID_ABI_VERSION,
            "compiler_version": "test",
            "source_path": "test.cor",
            "generated_at": "1970-01-01T00:00:00Z",
            "agents": agents_json,
            "prompts": [],
            "tools": [],
            "types": [],
            "approval_sites": []
        });
        corvid_abi::descriptor_from_json(&serde_json::to_string(&json).unwrap())
            .expect("synth_abi JSON deserializes to a CorvidAbi")
    }

    #[test]
    fn reviewer_reports_no_changes_when_both_sides_equal() {
        let base = synth_abi(&[("classify", "autonomous", false, true)]);
        let head = synth_abi(&[("classify", "autonomous", false, true)]);
        let receipt = invoke_reviewer(&base, &head).unwrap();
        assert!(receipt.contains("No algebraic changes detected"), "got: {receipt}");
    }

    #[test]
    fn reviewer_reports_added_agent() {
        let base = synth_abi(&[("classify", "autonomous", false, true)]);
        let head = synth_abi(&[
            ("classify", "autonomous", false, true),
            ("summarize", "autonomous", false, true),
        ]);
        let receipt = invoke_reviewer(&base, &head).unwrap();
        assert!(receipt.contains("Added"), "got: {receipt}");
        assert!(receipt.contains("summarize"), "got: {receipt}");
    }

    #[test]
    fn reviewer_reports_removed_agent() {
        let base = synth_abi(&[
            ("classify", "autonomous", false, true),
            ("summarize", "autonomous", false, true),
        ]);
        let head = synth_abi(&[("classify", "autonomous", false, true)]);
        let receipt = invoke_reviewer(&base, &head).unwrap();
        assert!(receipt.contains("Removed"), "got: {receipt}");
        assert!(receipt.contains("summarize"), "got: {receipt}");
    }

    #[test]
    fn reviewer_flags_dangerous_transition() {
        let base = synth_abi(&[("refund_bot", "human_required", false, false)]);
        let head = synth_abi(&[("refund_bot", "human_required", true, false)]);
        let receipt = invoke_reviewer(&base, &head).unwrap();
        assert!(receipt.contains("became `@dangerous`"), "got: {receipt}");
        assert!(receipt.contains("refund_bot"), "got: {receipt}");
    }

    #[test]
    fn reviewer_flags_trust_tier_change() {
        let base = synth_abi(&[("refund_bot", "human_required", false, false)]);
        let head = synth_abi(&[("refund_bot", "autonomous", false, false)]);
        let receipt = invoke_reviewer(&base, &head).unwrap();
        assert!(receipt.contains("trust changed"), "got: {receipt}");
        assert!(receipt.contains("human_required"), "got: {receipt}");
        assert!(receipt.contains("autonomous"), "got: {receipt}");
    }

    #[test]
    fn reviewer_is_deterministic_across_calls() {
        let base = synth_abi(&[
            ("classify", "autonomous", false, true),
            ("refund_bot", "human_required", true, false),
        ]);
        let head = synth_abi(&[
            ("classify", "autonomous", false, true),
            ("summarize", "autonomous", false, false),
        ]);
        let first = invoke_reviewer(&base, &head).unwrap();
        let second = invoke_reviewer(&base, &head).unwrap();
        assert_eq!(first, second, "@deterministic reviewer must produce byte-identical receipts");
    }
}
