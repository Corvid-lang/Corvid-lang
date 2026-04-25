//! Driver integration for `corvid trace-diff --stack` — walks a
//! commit range, materializes per-commit inputs, and feeds the
//! algebra composer in [`super::stacked`] to emit a `StackReceipt`.
//!
//! This module owns the I/O + orchestration half of the
//! `21-inv-H-5-stacked` slice. The algebra lives next door in
//! [`super::stacked`]; this file wires the CLI to that algebra via:
//!
//! - `--stack` flag shapes (auto-range from positional args,
//!   explicit git-range expression, explicit comma-separated SHAs)
//! - CI env auto-detect (GitHub Actions + GitLab CI) for the
//!   auto-range case — users drop `corvid trace-diff ... --stack`
//!   into a job without touching the positional SHAs
//! - `git log --first-parent --reverse <base>..<head>` to resolve
//!   the commit list
//! - per-commit source fetch + compile + diff, producing a
//!   `StackInput` for each commit
//! - canonical per-commit receipt hash (sha256 over commit SHA +
//!   delta records) so the Merkle stack hash is stable across
//!   runs without depending on the content-addressed cache
//!   (cache integration lands with the replay engine in a later
//!   commit of the slice)
//!
//! Step 2/N deliberately ships a narrow surface: JSON output
//! only, no signing, no `--traces` integration. Each restriction
//! is a typed error rather than silent degradation so users can
//! see exactly which later commit of the slice unlocks what
//! they're asking for.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use corvid_driver::{compile_to_abi_with_config, load_corvid_config_for};
use corvid_runtime::Verdict;
use corvid_types::config::CorvidConfig;
use rayon::prelude::*;
use sha2::{Digest, Sha256};

use super::impact::{collect_trace_files, run_harness_against_source, TraceImpact};
use super::narrative::{compute_diff_summary, DeltaRecord, DiffSummary, ReceiptNarrative};
use super::policy;
use super::receipt::{OutputFormat, Receipt, RECEIPT_SCHEMA_VERSION};
use super::stack_attribution::{
    can_skip_replay, commit_affected_agents, compute_stack_attributions,
    trace_exercised_agents, WaypointData,
};
use super::stacked::{
    self, AnomalySeverity, SignatureStatus, StackInput,
};
use super::TraceDiffArgs;

/// Parsed value of the `--stack[=<spec>]` flag.
#[derive(Debug, Clone)]
pub(crate) enum StackSpec {
    /// `--stack` alone. Commit range is derived from the
    /// positional `<base>..<head>` args; in CI, env vars
    /// (`GITHUB_BASE_REF` / `CI_MERGE_REQUEST_DIFF_BASE_SHA`)
    /// override the positional SHAs so jobs can invoke the CLI
    /// without copy-pasting the PR boundaries into the command
    /// line.
    AutoRange,
    /// `--stack=<git-range-expression>` — anything `git log`
    /// accepts as a range (`main..feature`, `HEAD~5..HEAD`,
    /// `abc123..def456`, etc.). Positional base/head are still
    /// required by clap but the range expression determines the
    /// actual commit list.
    Range(String),
    /// `--stack=<sha1>,<sha2>,<sha3>` — explicit enumeration.
    /// Positional base is taken as the parent-of-first-commit;
    /// positional head is reported verbatim in the `head_sha`
    /// field of the receipt. No `git log` walk.
    Explicit(Vec<String>),
}

/// Parse the raw `--stack=<spec>` value into a `StackSpec`. Empty
/// or whitespace-only input yields `AutoRange`; a comma in the
/// value triggers `Explicit` parsing; anything else is treated as
/// a git range expression.
pub(crate) fn parse_stack_spec(raw: &str) -> Result<StackSpec, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(StackSpec::AutoRange);
    }
    if trimmed.contains(',') {
        let shas: Vec<String> = trimmed
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect();
        if shas.is_empty() {
            return Err("`--stack=<comma-list>` had no commits".into());
        }
        return Ok(StackSpec::Explicit(shas));
    }
    Ok(StackSpec::Range(trimmed.to_string()))
}

/// Resolve the (base, head) pair for `AutoRange` mode. CI env
/// vars win over positional args so the CLI works in a vanilla CI
/// job configuration. GitHub Actions sets `GITHUB_BASE_REF` +
/// `GITHUB_HEAD_REF`; GitLab sets `CI_MERGE_REQUEST_DIFF_BASE_SHA`
/// + `CI_COMMIT_SHA`. When neither is set, positional wins.
fn auto_resolve_range(positional_base: &str, positional_head: &str) -> (String, String) {
    let gha_base = std::env::var("GITHUB_BASE_REF").ok().filter(|s| !s.is_empty());
    let gha_head = std::env::var("GITHUB_HEAD_REF")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("GITHUB_SHA").ok().filter(|s| !s.is_empty()));
    if let (Some(base), Some(head)) = (gha_base, gha_head) {
        return (base, head);
    }
    let gl_base = std::env::var("CI_MERGE_REQUEST_DIFF_BASE_SHA")
        .ok()
        .filter(|s| !s.is_empty());
    let gl_head = std::env::var("CI_COMMIT_SHA").ok().filter(|s| !s.is_empty());
    if let (Some(base), Some(head)) = (gl_base, gl_head) {
        return (base, head);
    }
    (positional_base.to_string(), positional_head.to_string())
}

/// Resolve the stack's commit list. Returns `(parent_of_first,
/// commits_in_chronological_order)`. For `AutoRange` and `Range`,
/// uses `git log --first-parent --reverse`. For `Explicit`, the
/// commits are the user-provided list and `parent_of_first` is the
/// positional base SHA.
fn resolve_commits_with_base(
    spec: &StackSpec,
    positional_base: &str,
    positional_head: &str,
) -> Result<(String, Vec<String>)> {
    match spec {
        StackSpec::AutoRange => {
            let (base, head) = auto_resolve_range(positional_base, positional_head);
            let commits = git_log_range(&format!("{base}..{head}"))?;
            Ok((base, commits))
        }
        StackSpec::Range(expr) => {
            let (base, _) = expr.split_once("..").ok_or_else(|| {
                anyhow!(
                    "`--stack=<range>` must be a two-endpoint git range (`<base>..<head>`); got `{expr}`"
                )
            })?;
            let commits = git_log_range(expr)?;
            Ok((base.to_string(), commits))
        }
        StackSpec::Explicit(shas) => Ok((positional_base.to_string(), shas.clone())),
    }
}

/// `git log --first-parent --reverse --format=%H <range-expr>`.
/// Reverse order puts the oldest commit first so iteration
/// produces waypoints in chronological order.
fn git_log_range(range_expr: &str) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args([
            "log",
            "--first-parent",
            "--reverse",
            "--format=%H",
            range_expr,
        ])
        .output()
        .with_context(|| format!("invoking `git log {range_expr}` (is git on PATH?)"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "`git log {range_expr}` failed: {}",
            stderr.trim()
        ));
    }
    let stdout =
        String::from_utf8(output.stdout).context("`git log` returned non-UTF-8")?;
    Ok(stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// Compile the source at `parent_sha` and `commit_sha`, diff the
/// two ABIs, and pack the result into a `StackInput` with a
/// stable content-addressed `receipt_hash`.
///
/// The hash here is synthesized from the commit SHA + delta
/// records rather than read from the cache. Step 4/N (replay
/// engine) wires this path through the hash-addressed cache so
/// re-composition reuses previously-computed per-commit
/// receipts; for step 2/N the goal is deterministic hashes
/// across runs, not deduplication.
fn compute_per_commit_input(
    parent_sha: &str,
    commit_sha: &str,
    source_path: &Path,
    config: Option<&CorvidConfig>,
) -> Result<StackInput> {
    let parent_source = git_show(parent_sha, source_path).with_context(|| {
        format!(
            "fetching `{}` at parent `{parent_sha}`",
            source_path.display()
        )
    })?;
    let commit_source = git_show(commit_sha, source_path).with_context(|| {
        format!(
            "fetching `{}` at commit `{commit_sha}`",
            source_path.display()
        )
    })?;

    let source_path_str = source_path.to_string_lossy().replace('\\', "/");
    let generated_at = "1970-01-01T00:00:00Z";

    let parent_abi = compile_to_abi_with_config(
        &parent_source,
        &source_path_str,
        generated_at,
        config,
    )
    .map_err(|diags| {
        anyhow!(
            "source at parent `{parent_sha}` failed to compile: {} diagnostic(s)",
            diags.len()
        )
    })?;
    let commit_abi = compile_to_abi_with_config(
        &commit_source,
        &source_path_str,
        generated_at,
        config,
    )
    .map_err(|diags| {
        anyhow!(
            "source at commit `{commit_sha}` failed to compile: {} diagnostic(s)",
            diags.len()
        )
    })?;

    let DiffSummary { records } = compute_diff_summary(&parent_abi, &commit_abi);

    let receipt_hash = per_commit_receipt_hash(commit_sha, &records);

    Ok(StackInput {
        commit_sha: commit_sha.to_string(),
        receipt_hash,
        envelope_hash: None,
        signature_status: SignatureStatus::Unsigned,
        deltas: records,
    })
}

/// Canonical per-commit hash used as the Merkle leaf input.
/// Content-addressed: same `(commit, deltas)` → same hash. The
/// field separator bytes (`\0` between delta key + summary; `\n`
/// between records) are deliberate: they can't appear inside
/// either string because delta keys and summaries are ASCII by
/// construction, so the encoding is injective.
fn per_commit_receipt_hash(
    commit_sha: &str,
    records: &[super::narrative::DeltaRecord],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(commit_sha.as_bytes());
    hasher.update(b"|");
    for record in records {
        hasher.update(record.key.as_bytes());
        hasher.update(b"\0");
        hasher.update(record.summary.as_bytes());
        hasher.update(b"\n");
    }
    hex::encode(hasher.finalize())
}

/// `git show <rev>:<path>` — reused from the single-commit path
/// because step 2/N doesn't justify refactoring the helper out of
/// the parent module yet. When the parent module's git surface
/// grows, it'll get its own file.
fn git_show(rev: &str, path: &Path) -> Result<String> {
    let rel = path.to_string_lossy().replace('\\', "/");
    let spec = format!("{rev}:{rel}");
    let output = Command::new("git")
        .args(["show", &spec])
        .output()
        .context("invoking `git show` (is git on PATH?)")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("`git show {spec}` failed: {}", stderr.trim()));
    }
    String::from_utf8(output.stdout)
        .with_context(|| format!("`git show {spec}` returned non-UTF-8 content"))
}

/// Run the 21-inv-G harness against every waypoint in the stack
/// (base + each commit), producing per-trace `Attribution` records
/// with commit-level `responsible_commit` and the full delta set
/// of that commit as `candidate_deltas`. Delta-level narrowing
/// (isolation replay + ddmin) refines `candidate_deltas` in a
/// later commit of the slice.
///
/// Per-waypoint replay with **algebra-directed skipping** and
/// **parallel execution**. The algebra decides which (waypoint,
/// trace) pairs need replaying; rayon runs the surviving ones
/// concurrently across cores. Order of the resulting waypoint
/// list is preserved regardless of parallelism — results collect
/// by original commit index.
///
/// Skip invariant: when a trace's exercised-agents set is
/// provably disjoint from a commit's affected-agents set, the
/// trace's verdict at that waypoint cannot differ from base's,
/// so we skip the replay and inherit the base verdict directly.
/// Behaviorally equivalent to full replay. The `no_skip`
/// parameter forces full replay for debugging / audit.
///
/// Persistent (cross-invocation) memoization lands in a follow-up
/// once the content-addressed Merkle DAG cache wires into this
/// path; for now the skip + parallelism pair is sufficient for
/// realistic stack sizes and unlocks delta-ddmin in step 3c/N
/// by keeping per-subset replay tractable.
#[allow(clippy::too_many_arguments)]
fn compute_attributions_for_stack(
    trace_dir: &Path,
    source_path_hint: &Path,
    source_path_str: &str,
    base_sha: &str,
    commits: &[String],
    commit_delta_sets: &[Vec<String>],
    config: Option<&CorvidConfig>,
    no_skip: bool,
) -> Result<Vec<super::stack_attribution::Attribution>> {
    let trace_paths = collect_trace_files(trace_dir)?;
    if trace_paths.is_empty() {
        // No traces → no attributions. Not an error; the user may
        // be running `--stack --traces <dir>` defensively in CI
        // without always having a populated corpus.
        return Ok(Vec::new());
    }

    // Load trace bytes once for content-addressed `trace_id`.
    let mut trace_files: Vec<(PathBuf, Vec<u8>)> = Vec::with_capacity(trace_paths.len());
    for path in &trace_paths {
        let bytes = std::fs::read(path)
            .with_context(|| format!("reading trace `{}`", path.display()))?;
        trace_files.push((path.clone(), bytes));
    }

    // Scratch directory for each waypoint's source: the harness
    // compiles against a real filesystem path (it looks for
    // `corvid.toml` via `load_corvid_config_for`), so each
    // waypoint needs its own file.
    let scratch = tempfile::tempdir()
        .context("create scratch dir for per-waypoint replay sources")?;
    let stem = source_path_hint
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("source");

    // Pre-compute each trace's exercised-agents set so the skip
    // decision is O(affected ∩ exercised) per (waypoint, trace),
    // not O(re-parse trace) per decision.
    let trace_exercised: Vec<std::collections::BTreeSet<String>> = trace_files
        .iter()
        .map(|(_, bytes)| trace_exercised_agents(bytes))
        .collect();

    let mut waypoints: Vec<WaypointData> = Vec::with_capacity(commits.len() + 1);

    // Base waypoint. Always replayed in full — it's the reference
    // every subsequent waypoint is compared against, so there's no
    // disjoint-set optimization to apply here.
    let base_source = git_show(base_sha, source_path_hint).with_context(|| {
        format!(
            "fetching base `{}` at `{base_sha}`",
            source_path_hint.display()
        )
    })?;
    let base_scratch_path = scratch.path().join(format!("{stem}.waypoint_0.cor"));
    std::fs::write(&base_scratch_path, &base_source)
        .context("write base source for waypoint replay")?;
    let _ = config; // config passed through for future compile-time use
    let _ = source_path_str;
    let base_verdicts = run_harness_against_source(&trace_paths, &base_scratch_path)
        .with_context(|| format!("harness against base `{base_sha}`"))?;
    let base_tags = verdict_map_to_tags(base_verdicts);
    waypoints.push(WaypointData {
        commit_sha: base_sha.to_string(),
        verdict_tags: base_tags.clone(),
    });

    // Per-waypoint work is independent — different commit sources,
    // different scratch paths, different harness runs with no
    // shared mutable state. Dispatch through rayon so N waypoint
    // replays run concurrently (bounded by rayon's thread pool,
    // which defaults to num_cpus). Order is preserved because we
    // collect by original index.
    struct WaypointComputation {
        waypoint: WaypointData,
        skipped: usize,
        replayed: usize,
    }

    let commit_computations: Vec<WaypointComputation> = commits
        .par_iter()
        .enumerate()
        .map(|(i, commit_sha)| -> Result<WaypointComputation> {
            let affected = commit_affected_agents(&commit_delta_sets[i]);

            // Partition traces into replay-required vs skippable.
            // A trace is skippable when its exercised-agents set
            // is provably disjoint from this commit's affected
            // set — replay would, by algebra, produce the same
            // verdict as base.
            let mut replay_required_paths: Vec<PathBuf> = Vec::new();
            let mut skipped_paths: Vec<&PathBuf> = Vec::new();
            for (path, exercised) in trace_paths.iter().zip(&trace_exercised) {
                if !no_skip && can_skip_replay(exercised, &affected) {
                    skipped_paths.push(path);
                } else {
                    replay_required_paths.push(path.clone());
                }
            }

            // Build the waypoint's verdict tags: skipped traces
            // inherit base's tag directly; replay-required traces
            // get their actual verdict from the harness.
            let mut verdict_tags = BTreeMap::new();
            for path in &skipped_paths {
                if let Some(tag) = base_tags.get(*path) {
                    verdict_tags.insert((*path).clone(), tag.clone());
                }
            }
            if !replay_required_paths.is_empty() {
                let commit_source =
                    git_show(commit_sha, source_path_hint).with_context(|| {
                        format!(
                            "fetching commit `{}` at `{commit_sha}`",
                            source_path_hint.display()
                        )
                    })?;
                let commit_scratch_path =
                    scratch.path().join(format!("{stem}.waypoint_{}.cor", i + 1));
                std::fs::write(&commit_scratch_path, &commit_source)
                    .context("write commit source for waypoint replay")?;
                let commit_verdicts =
                    run_harness_against_source(&replay_required_paths, &commit_scratch_path)
                        .with_context(|| {
                            format!("harness against commit `{commit_sha}`")
                        })?;
                for (path, tag) in verdict_map_to_tags(commit_verdicts) {
                    verdict_tags.insert(path, tag);
                }
            }

            Ok(WaypointComputation {
                waypoint: WaypointData {
                    commit_sha: commit_sha.clone(),
                    verdict_tags,
                },
                skipped: skipped_paths.len(),
                replayed: replay_required_paths.len(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let (total_skipped, total_replayed): (usize, usize) = commit_computations
        .iter()
        .fold((0, 0), |(s, r), w| (s + w.skipped, r + w.replayed));
    for computation in commit_computations {
        waypoints.push(computation.waypoint);
    }

    // Stderr summary of skip effectiveness. CI consumers can grep
    // this to confirm the algebra is earning its keep.
    if !no_skip && total_skipped > 0 {
        eprintln!(
            "algebra-directed skip: {total_skipped} (replay, trace) pairs skipped, {total_replayed} replayed across {} commits",
            commits.len()
        );
    }

    Ok(compute_stack_attributions(
        &trace_files,
        &waypoints,
        commit_delta_sets,
    ))
}

/// Map harness verdicts to the string tags the attribution algebra
/// compares. Kept in the driver module because it's the only place
/// that bridges `corvid_runtime::Verdict` into the tag vocabulary
/// `stack_attribution` defines.
fn verdict_map_to_tags(
    verdicts: BTreeMap<PathBuf, Verdict>,
) -> BTreeMap<PathBuf, String> {
    verdicts
        .into_iter()
        .map(|(path, verdict)| {
            let tag = match verdict {
                Verdict::Passed => "passed",
                Verdict::Diverged => "diverged",
                // `Flaky`, `Promoted`, and `Error` shouldn't reach
                // trace-diff's plain-replay path — they're
                // promote-mode / record-current / infra-error
                // signals. Map them to distinct tags so the
                // algebra surfaces them as divergences from
                // `passed`/`diverged` rather than silently
                // collapsing to one of the two.
                Verdict::Flaky => "flaky",
                Verdict::Promoted => "promoted",
                Verdict::Error => "errored",
            };
            (path, tag.to_string())
        })
        .collect()
}

/// Top-level stack-mode entry point. Called from `run_trace_diff`
/// when `--stack` is present; emits the composed `StackReceipt`
/// as canonical JSON on stdout.
pub(super) fn run_trace_diff_stack(
    spec: &StackSpec,
    args: &TraceDiffArgs<'_>,
) -> Result<u8> {
    let env_signing_key = std::env::var_os("CORVID_SIGNING_KEY").is_some();
    if args.sign_key_path.is_some() || env_signing_key {
        return Err(anyhow!(
            "`--stack` with `--sign` / `CORVID_SIGNING_KEY` is not yet implemented (Merkle signing ships in a later commit of 21-inv-H-5-stacked)"
        ));
    }
    if !matches!(args.format, OutputFormat::Json) {
        return Err(anyhow!(
            "`--stack` currently only supports `--format=json`; markdown / github-check / gitlab / in-toto renderers ship in a later commit of 21-inv-H-5-stacked"
        ));
    }

    let (base_sha, commits) = resolve_commits_with_base(spec, args.base_sha, args.head_sha)?;
    if commits.is_empty() {
        return Err(anyhow!(
            "stack range resolved to zero commits; check the range expression and that the commits exist"
        ));
    }
    let head_sha = commits
        .last()
        .expect("non-empty guarded above")
        .clone();

    let config = load_corvid_config_for(args.source_path);
    let source_path_str = args.source_path.to_string_lossy().replace('\\', "/");

    let mut parent = base_sha.clone();
    let mut inputs = Vec::with_capacity(commits.len());
    for commit in &commits {
        let input =
            compute_per_commit_input(&parent, commit, args.source_path, config.as_ref())
                .with_context(|| {
                    format!("computing per-commit input for `{commit}` (parent `{parent}`)")
                })?;
        inputs.push(input);
        parent = commit.clone();
    }

    let range_spec_str = match spec {
        StackSpec::AutoRange => format!("{}..{}", base_sha, head_sha),
        StackSpec::Range(r) => r.clone(),
        StackSpec::Explicit(shas) => shas.join(","),
    };

    // Snapshot the per-commit delta keys before `inputs` is moved
    // into `compose_stack`. Attribution needs them aligned to
    // `commits[..]` (not including base) so the algebra can map a
    // `diverged_at: commits[i]` back to the delta set that shipped
    // in that commit.
    let commit_delta_sets: Vec<Vec<String>> = inputs
        .iter()
        .map(|input| {
            input
                .deltas
                .iter()
                .map(|d| d.key.clone())
                .collect()
        })
        .collect();

    let mut receipt = stacked::compose_stack(
        &base_sha,
        &head_sha,
        &source_path_str,
        &range_spec_str,
        inputs,
    );

    if let Some(trace_dir) = args.trace_dir {
        receipt.attributions = compute_attributions_for_stack(
            trace_dir,
            args.source_path,
            &source_path_str,
            &base_sha,
            &commits,
            &commit_delta_sets,
            config.as_ref(),
            args.no_replay_skip,
        )
        .context("counterfactual replay across stack waypoints failed")?;
    }
    receipt.verdict = policy::apply_policy(&stack_policy_receipt(&receipt), args.policy_path)?;

    let json = serde_json::to_string_pretty(&receipt)
        .expect("StackReceipt is trivially serializable");
    print!("{json}\n");

    if receipt
        .anomalies
        .iter()
        .any(|a| matches!(a.severity, AnomalySeverity::HardFail))
    {
        return Err(anyhow!(
            "stack composition hit a hard-fail anomaly; see `anomalies` in the emitted receipt"
        ));
    }
    if !receipt.anomalies.is_empty() {
        eprintln!("stack anomalies surfaced:");
        for anomaly in &receipt.anomalies {
            eprintln!(
                "  - [{:?}] {}",
                anomaly.class, anomaly.detail
            );
        }
        return Ok(1);
    }
    if !receipt.verdict.ok {
        eprintln!("regression policy tripped:");
        for flag in &receipt.verdict.flags {
            eprintln!("  - {flag}");
        }
        return Ok(1);
    }

    Ok(0)
}

fn stack_policy_receipt(receipt: &stacked::StackReceipt) -> Receipt {
    Receipt {
        schema_version: RECEIPT_SCHEMA_VERSION,
        base_sha: receipt.base_sha.clone(),
        head_sha: receipt.head_sha.clone(),
        source_path: receipt.source_path.clone(),
        deltas: receipt
            .history
            .iter()
            .map(|delta| DeltaRecord {
                key: delta.key.clone(),
                summary: delta.summary.clone(),
            })
            .collect(),
        impact: TraceImpact::empty(),
        narrative: ReceiptNarrative::empty(),
        narrative_rejected: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_stack_spec_empty_is_auto_range() {
        assert!(matches!(
            parse_stack_spec("").unwrap(),
            StackSpec::AutoRange
        ));
        assert!(matches!(
            parse_stack_spec("   ").unwrap(),
            StackSpec::AutoRange
        ));
    }

    #[test]
    fn parse_stack_spec_range_expression() {
        match parse_stack_spec("main..feature").unwrap() {
            StackSpec::Range(r) => assert_eq!(r, "main..feature"),
            other => panic!("expected Range, got {other:?}"),
        }
    }

    #[test]
    fn parse_stack_spec_explicit_list() {
        match parse_stack_spec("abc,def, ghi ").unwrap() {
            StackSpec::Explicit(shas) => assert_eq!(shas, vec!["abc", "def", "ghi"]),
            other => panic!("expected Explicit, got {other:?}"),
        }
    }

    #[test]
    fn parse_stack_spec_rejects_empty_list() {
        assert!(parse_stack_spec(",,").is_err());
    }

    #[test]
    fn per_commit_receipt_hash_is_deterministic() {
        let records = vec![super::super::narrative::DeltaRecord {
            key: "agent.added:foo".into(),
            summary: "new agent `foo`".into(),
        }];
        let a = per_commit_receipt_hash("sha1", &records);
        let b = per_commit_receipt_hash("sha1", &records);
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn per_commit_receipt_hash_differs_by_commit_sha() {
        let records = vec![super::super::narrative::DeltaRecord {
            key: "agent.added:foo".into(),
            summary: "".into(),
        }];
        let a = per_commit_receipt_hash("sha1", &records);
        let b = per_commit_receipt_hash("sha2", &records);
        assert_ne!(a, b);
    }
}
