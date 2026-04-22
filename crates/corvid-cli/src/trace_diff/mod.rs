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

mod gitlab;
mod impact;
mod in_toto;
mod narrative;
mod receipt;
mod reviewer_invocation;
mod stacked;
pub(crate) mod signing;

use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use corvid_driver::{compile_to_abi_with_config, load_corvid_config_for};

use impact::{compute_trace_impact, TraceImpact};
pub use narrative::NarrativeMode;
use narrative::{
    compute_diff_summary, validate_narrative, DiffSummary, NarrativeRejection, ReceiptNarrative,
};
pub use receipt::OutputFormat;
use receipt::{apply_default_policy, render_github_check, render_json, Receipt};
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
    /// Output format. [`OutputFormat::Markdown`] for human review,
    /// [`OutputFormat::GithubCheck`] for GitHub Actions
    /// annotation commands, [`OutputFormat::Json`] for bot
    /// consumption. Callers can pick via [`OutputFormat::parse`]
    /// from `--format=<mode>`.
    pub format: OutputFormat,
    /// When `Some`, sign the canonical JSON receipt with the
    /// ed25519 key at the given path and emit a DSSE envelope
    /// instead of the raw format output. The key file is parsed
    /// as hex (64 hex chars = 32-byte seed) or raw (32 bytes).
    /// When `None` but `CORVID_SIGNING_KEY` is set in the
    /// environment, fall back to the env var. When neither is
    /// set, no signing happens and the `--format` output is
    /// emitted unchanged.
    pub sign_key_path: Option<&'a Path>,
    /// Key ID to embed in the DSSE envelope's `signatures[0].keyid`
    /// field. Free-form identifier; typically the hex prefix of
    /// the verifying key or a project-chosen label. Defaults to
    /// `"corvid-default"` when signing is active but no label
    /// is supplied.
    pub sign_key_id: Option<&'a str>,
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

    let diff = compute_diff_summary(&base_abi, &head_abi);
    let (narrative, narrative_rejected) = resolve_narrative(&diff, args.narrative_mode)?;

    let receipt = Receipt::build(
        args.base_sha,
        args.head_sha,
        &source_path_str,
        &base_abi,
        &head_abi,
        impact,
        narrative.clone(),
        narrative_rejected,
    );

    let verdict = apply_default_policy(&receipt);

    // Markdown stays Corvid-side — the reviewer agent owns the
    // human-readable layout. Other formats are Rust renderers
    // over the shared Receipt.
    let rendered = match args.format {
        OutputFormat::Markdown => invoke_reviewer(&base_abi, &head_abi, &receipt.impact, &narrative)
            .context("reviewer agent execution failed")?,
        OutputFormat::GithubCheck => render_github_check(&receipt, &verdict),
        OutputFormat::Json => render_json(&receipt, &verdict),
        OutputFormat::InToto => in_toto::render_in_toto(
            &receipt,
            &verdict,
            head_source.as_bytes(),
            &source_path_str,
        ),
        OutputFormat::Gitlab => gitlab::render_gitlab(&receipt, &verdict),
    };

    // Signing path: when a key source is available, wrap the
    // canonical payload in a DSSE envelope. Payload shape +
    // envelope payloadType both follow the `--format` flag —
    // in-toto Statements get `application/vnd.in-toto+json` so
    // cosign / slsa-verifier consume them natively; every other
    // format signs the canonical JSON receipt with Corvid's
    // native payloadType. Markdown and github-check fall back to
    // signing the JSON receipt (signing a markdown string makes
    // no sense for cryptographic tooling).
    let key_source = signing::resolve_key_source(args.sign_key_path);
    if let Some(source) = key_source {
        let (payload, payload_type) = match args.format {
            OutputFormat::InToto => (
                in_toto::render_in_toto(
                    &receipt,
                    &verdict,
                    head_source.as_bytes(),
                    &source_path_str,
                ),
                in_toto::IN_TOTO_DSSE_PAYLOAD_TYPE,
            ),
            // All non-in-toto formats sign the canonical JSON
            // receipt — that's the byte-stable, schema-versioned
            // Corvid-native payload.
            _ => (
                render_json(&receipt, &verdict),
                signing::CORVID_RECEIPT_PAYLOAD_TYPE,
            ),
        };
        let signing_key = signing::load_signing_key(&source)
            .with_context(|| "loading signing key")?;
        let key_id = args.sign_key_id.unwrap_or("corvid-default");
        let envelope =
            signing::sign_envelope(payload.as_bytes(), payload_type, &signing_key, key_id);
        let envelope_json = signing::envelope_to_json(&envelope);

        // Persist to the receipt cache keyed by the RECEIPT's
        // hash (not the wrapped payload), so `corvid receipt
        // show <hash>` always resolves to the raw receipt
        // regardless of whether it was signed bare or wrapped
        // in an in-toto Statement. Cache failures are non-fatal.
        let receipt_json_for_cache = render_json(&receipt, &verdict);
        match crate::receipt_cache::store(
            receipt_json_for_cache.as_bytes(),
            envelope_json.as_bytes(),
        ) {
            Ok(stored) => {
                eprintln!("Corvid-Receipt: {}", stored.hash);
            }
            Err(e) => {
                eprintln!(
                    "warning: signed receipt not cached locally ({e}); signature is still emitted on stdout"
                );
            }
        }

        print!("{envelope_json}");
    } else {
        print!("{rendered}");
    }

    // Every format surfaces the gate-failure reasons on stderr
    // so operators reading only stderr know what tripped. JSON
    // consumers parse `verdict.flags` directly; markdown readers
    // see the receipt body + stderr reasons.
    if !verdict.ok {
        eprintln!("regression policy tripped:");
        for flag in &verdict.flags {
            eprintln!("  - {flag}");
        }
        return Ok(1);
    }

    Ok(0)
}

/// Drive the narrative pipeline: detect (or refuse) an LLM
/// adapter per the user's mode, invoke the `summarise_diff`
/// prompt with the pre-computed diff summary, validate citations,
/// emit a stderr warning on any rejection, and always fall back
/// to `ReceiptNarrative::empty()` when anything goes sideways.
/// Returns `(narrative, was_rejected)` — the `was_rejected`
/// bool lets downstream renderers surface that the LLM
/// generated *something* but we didn't trust it. Only the
/// Rust-computed `diff` is passed in so there's one canonical
/// delta-key set across the narrative + receipt path.
fn resolve_narrative(
    diff: &DiffSummary,
    mode: NarrativeMode,
) -> Result<(ReceiptNarrative, bool)> {
    if matches!(mode, NarrativeMode::Off) {
        return Ok((ReceiptNarrative::empty(), false));
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
                NarrativeMode::Auto => Ok((ReceiptNarrative::empty(), false)),
                NarrativeMode::Off => unreachable!(),
            };
        }
    };

    // Empty diff → empty narrative. Skip the prompt call entirely;
    // there's nothing to summarise and the prompt would just pay a
    // round-trip to generate an empty response.
    if diff.records.is_empty() {
        return Ok((ReceiptNarrative::empty(), false));
    }

    let narrative = match invoke_narrative_prompt(diff, builder) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("narrative rejected: prompt invocation failed: {e:#}");
            return Ok((ReceiptNarrative::empty(), true));
        }
    };

    match validate_narrative(&narrative, diff) {
        Ok(()) => Ok((narrative, false)),
        Err(NarrativeRejection::UnknownCitationKey(_))
        | Err(NarrativeRejection::NonEmptyBodyWithoutCitations)
        | Err(NarrativeRejection::DuplicateCitationKey(_)) => {
            // Prompt returned something the validator refused.
            // Surface the reason on stderr and flip the rejection
            // flag so renderers + JSON consumers can mark it.
            let reason = match validate_narrative(&narrative, diff) {
                Err(r) => r.to_string(),
                Ok(()) => "unknown".to_string(),
            };
            eprintln!("narrative rejected: {reason}");
            Ok((ReceiptNarrative::empty(), true))
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
