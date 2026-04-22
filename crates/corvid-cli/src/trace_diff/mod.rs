//! `corvid trace-diff <base-sha> <head-sha> <path>` — PR behavior
//! receipt.
//!
//! Git-integrates at the two ends of a PR, extracts the 22-B ABI
//! descriptor from each, digests both down to a shared `Descriptor`
//! shape, and hands them to an in-repo Corvid reviewer agent that
//! walks the algebra and emits a markdown receipt. With an optional
//! `--traces <dir>`, each `.jsonl` trace under `<dir>` is replayed
//! against base and head; the reviewer appends a counterfactual
//! impact section reporting which traces would have newly diverged
//! under the PR's changes.
//!
//! The reviewer itself is a Corvid program ([`reviewer.cor`]) rather
//! than a Rust helper. That is Corvid's thesis: AI-native governance
//! is a first-class programming domain with compile-time guarantees.
//! Shipping the flagship PR-review tool in Rust would soften the
//! thesis — it would be the same shortcut Python would make shipping
//! its linter in bash. The reviewer runs through the interpreter,
//! consumes `Descriptor` + `TraceImpact` values produced by Rust-side
//! digestion, and returns a markdown `String` that the CLI prints.
//!
//! This module owns the top-level orchestration: git extraction of
//! the source at both SHAs, ABI compilation, delegation to the
//! impact + reviewer-invocation submodules, and receipt emission.
//! The heavier sub-concerns live next door:
//!
//! - [`impact`] — counterfactual replay against base + head,
//!   per-trace verdict bucketing, [`TraceImpact`] construction.
//! - [`reviewer_invocation`] — compile the in-repo reviewer,
//!   digest `CorvidAbi` → `Descriptor`, call `review_pr`.
//!
//! Landed:
//!
//! - `21-inv-H-1` static algebra diff (added / removed agents;
//!   trust-tier / `@dangerous` / `@replayable` transitions across
//!   the `pub extern "c"` exported surface).
//! - `21-inv-H-2` counterfactual replay: `--traces <dir>` replays each
//!   trace against base and head, receipt reports the newly-divergent
//!   population + an impact percentage.
//! - `21-inv-H-3` structured approval-contract + provenance drill-down.
//!
//! Follow-up slices:
//!
//! - `21-inv-H-4` LLM-generated prose summary grounded in the algebra.
//! - `21-inv-H-5` `--format=github-check|markdown|json` outputs.

mod impact;
mod narrative;
mod reviewer_invocation;

use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use corvid_driver::{compile_to_abi_with_config, load_corvid_config_for};

use impact::{compute_trace_impact, TraceImpact};
pub use narrative::NarrativeMode;
use narrative::{compute_diff_summary, validate_narrative, ReceiptNarrative};
use reviewer_invocation::{detect_adapter, invoke_narrative_prompt, invoke_reviewer, NoAdapter};

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
    /// Optional directory of `.jsonl` traces to replay against both
    /// sides. When present, the receipt includes a counterfactual
    /// impact section; when absent, the receipt is the static
    /// algebra diff only (21-inv-H-1 behavior).
    pub trace_dir: Option<&'a Path>,
    /// Whether to run the LLM narrative prompt for the top-of-
    /// receipt prose paragraph. Default is [`NarrativeMode::Auto`]
    /// (use when an adapter is configured, skip silently
    /// otherwise). CI and deterministic-reproducer callers pick
    /// [`NarrativeMode::Off`].
    pub narrative_mode: NarrativeMode,
}

/// Run `corvid trace-diff`: fetch source at both SHAs, compile each
/// to a `CorvidAbi` descriptor, digest to `Descriptor`s, optionally
/// replay every trace in the corpus against both sides to build a
/// `TraceImpact`, run the reviewer agent, print the receipt. Returns
/// 0 on clean execution regardless of whether changes or divergences
/// were found — the receipt itself carries the verdict. Downstream
/// CI policy-gating slices can non-zero-exit based on receipt content.
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

    let impact = match args.trace_dir {
        Some(dir) => compute_trace_impact(&base_source, &head_source, args.source_path, dir)
            .context("counterfactual replay failed")?,
        None => TraceImpact::empty(),
    };

    let narrative = resolve_narrative(&base_abi, &head_abi, args.narrative_mode)?;

    let receipt = invoke_reviewer(&base_abi, &head_abi, &impact, &narrative)
        .context("reviewer agent execution failed")?;
    print!("{receipt}");
    Ok(0)
}

/// Drive the narrative pipeline: compute the diff summary, detect
/// (or refuse) an LLM adapter per the user's mode, invoke the
/// `summarise_diff` prompt, validate the citations, emit a stderr
/// warning on any rejection, and always fall back to
/// `ReceiptNarrative::empty()` when anything goes sideways. The
/// reviewer's byte-determinism invariant is preserved: identical
/// inputs always produce an identical narrative *or* the same
/// empty sentinel.
fn resolve_narrative(
    base: &corvid_abi::CorvidAbi,
    head: &corvid_abi::CorvidAbi,
    mode: NarrativeMode,
) -> Result<ReceiptNarrative> {
    if matches!(mode, NarrativeMode::Off) {
        return Ok(ReceiptNarrative::empty());
    }

    let builder = match detect_adapter() {
        Ok(b) => b,
        Err(reason) => {
            return match mode {
                NarrativeMode::On => Err(anyhow!(
                    "--narrative=on requires an LLM adapter ({})",
                    match reason {
                        NoAdapter::NoModelSelected =>
                            "set `CORVID_MODEL` and one of `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`",
                        NoAdapter::NoApiKeyForModel =>
                            "`CORVID_MODEL` is set but no matching API key is exported",
                    }
                )),
                NarrativeMode::Auto => Ok(ReceiptNarrative::empty()),
                NarrativeMode::Off => unreachable!(),
            };
        }
    };

    let diff = compute_diff_summary(base, head);
    // Empty diff → empty narrative. Skip the prompt call entirely;
    // there's nothing to summarise and the prompt would just pay a
    // round-trip to generate an empty response.
    if diff.records.is_empty() {
        return Ok(ReceiptNarrative::empty());
    }

    let narrative = match invoke_narrative_prompt(&diff, builder) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("narrative rejected: prompt invocation failed: {e:#}");
            return Ok(ReceiptNarrative::empty());
        }
    };

    match validate_narrative(&narrative, &diff) {
        Ok(()) => Ok(narrative),
        Err(reason) => {
            eprintln!("narrative rejected: {reason}");
            Ok(ReceiptNarrative::empty())
        }
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
