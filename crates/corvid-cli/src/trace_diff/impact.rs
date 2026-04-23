//! Counterfactual trace-impact computation for `corvid trace-diff`.
//!
//! Given a `.cor` source at two SHAs and a directory of recorded
//! traces, this module replays every trace against both sides via
//! the 21-inv-G-harness, categorises the per-trace verdicts into the
//! five buckets the reviewer renders (`passed_both`, `newly_diverged`,
//! `newly_passing`, `diverged_both`, `errored`), and packages them
//! into a [`TraceImpact`] value the reviewer consumes.
//!
//! The numeric formatting (percentages, bucket counts) happens here
//! rather than in the reviewer because Corvid doesn't yet have an
//! `Int→String` primitive. Once the language gains one, the
//! reviewer can take ownership of formatting and this module shrinks
//! to bucketing + path-list emission.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use corvid_driver::{
    run_replay_from_source_with_builder_async, ReplayMode, Runtime, RuntimeBuilder,
};
use corvid_runtime::{
    AnthropicAdapter, OpenAiAdapter, PromotePromptMode, StdinApprover, TestFromTracesOptions,
    TraceHarnessMode, TraceHarnessRequest, TraceHarnessRun, Verdict,
};
use serde::Serialize;

/// Cap on how many newly-divergent trace paths the reviewer lists in
/// the receipt. Larger populations are summarised as a count + a
/// truncation notice; the full list is preserved in the JSON output
/// that 21-inv-H-5 will ship for bot consumption.
pub(super) const NEWLY_DIVERGED_PATH_CAP: usize = 20;

/// Mirror of the reviewer's `TraceImpact` type. The Rust side owns
/// the numeric formatting (Corvid doesn't have an Int→String
/// primitive today — the follow-up language slice that adds one
/// will let the reviewer format its own counts). The reviewer still
/// owns whether the section is rendered, where it appears in the
/// receipt, and the narrative lines between the pre-formatted
/// summary and the path list.
#[derive(Debug, Clone, Serialize)]
pub(super) struct TraceImpact {
    pub(super) has_traces: bool,
    pub(super) any_newly_diverged: bool,
    pub(super) summary_line: String,
    pub(super) impact_percentage: String,
    pub(super) newly_diverged_paths: Vec<String>,
}

impl TraceImpact {
    pub(super) fn empty() -> Self {
        Self {
            has_traces: false,
            any_newly_diverged: false,
            summary_line: String::new(),
            impact_percentage: String::new(),
            newly_diverged_paths: Vec::new(),
        }
    }

    /// Sentinel impact for "user supplied `--traces` but the dir
    /// has no `.jsonl` files." Keeps `has_traces == false` so the
    /// reviewer renders nothing rather than an empty section. The
    /// summary string is preserved so JSON consumers in 21-inv-H-5
    /// can surface the reason.
    pub(super) fn empty_with_summary(summary: &str) -> Self {
        let mut s = Self::empty();
        s.summary_line = summary.to_string();
        s
    }
}

/// Replay every `.jsonl` trace under `trace_dir` against both
/// `base_source` and `head_source`, then digest the per-trace
/// verdicts into a `TraceImpact`.
///
/// The two sources are written to a scratch directory before the
/// regression harness is invoked — the harness needs real paths
/// (it compiles against a `corvid.toml` via `load_corvid_config_for`
/// which walks up from the source's parent). Scratch files use the
/// original `source_path_hint`'s file stem so compile errors name a
/// familiar filename.
pub(super) fn compute_trace_impact(
    base_source: &str,
    head_source: &str,
    source_path_hint: &Path,
    trace_dir: &Path,
) -> Result<TraceImpact> {
    let trace_paths = collect_trace_files(trace_dir)?;
    if trace_paths.is_empty() {
        return Ok(TraceImpact::empty_with_summary(
            "No `.jsonl` traces found under the supplied `--traces` directory.",
        ));
    }

    let scratch = tempfile::tempdir().context("create scratch dir for source-at-two-shas")?;
    let stem = source_path_hint
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("source");
    let base_path = scratch.path().join(format!("{stem}.base.cor"));
    let head_path = scratch.path().join(format!("{stem}.head.cor"));
    std::fs::write(&base_path, base_source)
        .context("write base source for counterfactual replay")?;
    std::fs::write(&head_path, head_source)
        .context("write head source for counterfactual replay")?;

    let base_verdicts = run_harness_against_source(&trace_paths, &base_path)
        .context("running harness against base source")?;
    let head_verdicts = run_harness_against_source(&trace_paths, &head_path)
        .context("running harness against head source")?;

    Ok(categorise_impact(base_verdicts, head_verdicts))
}

/// Scan `trace_dir` for `.jsonl` files. Non-recursive — matches the
/// shape the recorder writes (siblings under one dir). Fails cleanly
/// if `trace_dir` is missing or not a directory. Lexicographic sort
/// so receipts are byte-stable across reruns on the same OS.
pub(super) fn collect_trace_files(trace_dir: &Path) -> Result<Vec<PathBuf>> {
    if !trace_dir.exists() {
        anyhow::bail!("trace directory `{}` does not exist", trace_dir.display());
    }
    if !trace_dir.is_dir() {
        anyhow::bail!("trace directory `{}` is not a directory", trace_dir.display());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(trace_dir)
        .with_context(|| format!("reading trace dir `{}`", trace_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// Run the 21-inv-G-harness against a single source path in plain
/// replay mode. Returns a per-trace verdict map keyed by path so the
/// caller can zip base-run and head-run results without caring about
/// ordering.
pub(super) fn run_harness_against_source(
    trace_paths: &[PathBuf],
    source_path: &Path,
) -> Result<std::collections::BTreeMap<PathBuf, Verdict>> {
    let options = TestFromTracesOptions {
        replay_model: None,
        promote: false,
        flake_detect: None,
        prompt_mode: PromotePromptMode::AutoStdin,
    };

    let tokio_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime for counterfactual replay")?;

    let source_path_owned: PathBuf = source_path.to_path_buf();
    let report = tokio_rt.block_on(corvid_runtime::run_test_from_traces(
        trace_paths.to_vec(),
        options,
        move |request| {
            let source_path = source_path_owned.clone();
            async move { dispatch_replay_for_trace_diff(&source_path, request).await }
        },
    ));

    if report.aborted {
        anyhow::bail!(
            "counterfactual replay aborted unexpectedly (promote-mode should not be reachable)"
        );
    }

    Ok(report
        .per_trace
        .into_iter()
        .map(|o| (o.path, o.verdict))
        .collect())
}

/// Trace-diff's harness runner. Plain replay only — no differential,
/// no record-current. Structurally mirrors the test_from_traces
/// dispatcher but keeps its own copy because the two callsites'
/// options differ (trace-diff never promotes, never swaps models).
async fn dispatch_replay_for_trace_diff(
    source_path: &Path,
    request: TraceHarnessRequest,
) -> std::result::Result<TraceHarnessRun, corvid_runtime::RuntimeError> {
    match request.mode {
        TraceHarnessMode::Replay => {
            let base_builder = default_runtime_builder();
            let outcome = run_replay_from_source_with_builder_async(
                &request.trace_path,
                source_path,
                ReplayMode::Plain,
                base_builder,
            )
            .await
            .map_err(|err| corvid_runtime::RuntimeError::ReplayTraceLoad {
                path: request.trace_path.clone(),
                message: format!("{err:#}"),
            })?;
            Ok(TraceHarnessRun {
                final_output: None,
                ok: outcome.ran_cleanly(),
                error: outcome.result_error.clone(),
                emitted_trace_path: request.trace_path.clone(),
                differential_report: outcome.differential_report,
            })
        }
        TraceHarnessMode::Differential { .. } | TraceHarnessMode::RecordCurrent => {
            Err(corvid_runtime::RuntimeError::ReplayTraceLoad {
                path: request.trace_path.clone(),
                message:
                    "trace-diff's counterfactual path only runs plain replay; \
                     Differential and RecordCurrent requests should not reach here"
                        .into(),
            })
        }
    }
}

fn default_runtime_builder() -> RuntimeBuilder {
    let mut builder = Runtime::builder().approver(Arc::new(StdinApprover::new()));
    if let Ok(model) = std::env::var("CORVID_MODEL") {
        builder = builder.default_model(&model);
    }
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        builder = builder.llm(Arc::new(AnthropicAdapter::new(key)));
    }
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        builder = builder.llm(Arc::new(OpenAiAdapter::new(key)));
    }
    builder
}

/// Categorise per-trace verdicts into the five buckets the reviewer
/// renders. Traces present on only one side are treated as errored
/// (shouldn't happen — both harness runs consume the same input —
/// but the defensive branch avoids silent truncation).
fn categorise_impact(
    base: std::collections::BTreeMap<PathBuf, Verdict>,
    head: std::collections::BTreeMap<PathBuf, Verdict>,
) -> TraceImpact {
    let mut total = 0usize;
    let mut passed_both = 0usize;
    let mut newly_diverged = 0usize;
    let mut newly_passing = 0usize;
    let mut diverged_both = 0usize;
    let mut errored = 0usize;
    let mut newly_diverged_paths: Vec<String> = Vec::new();

    let mut all_paths: std::collections::BTreeSet<&PathBuf> = base.keys().collect();
    all_paths.extend(head.keys());

    for path in all_paths {
        total += 1;
        match (base.get(path), head.get(path)) {
            (Some(Verdict::Passed), Some(Verdict::Passed)) => passed_both += 1,
            (Some(Verdict::Passed), Some(Verdict::Diverged)) => {
                newly_diverged += 1;
                newly_diverged_paths.push(display_path(path));
            }
            (Some(Verdict::Diverged), Some(Verdict::Passed)) => newly_passing += 1,
            (Some(Verdict::Diverged), Some(Verdict::Diverged)) => diverged_both += 1,
            _ => errored += 1,
        }
    }

    let any_newly_diverged = newly_diverged > 0;
    if newly_diverged_paths.len() > NEWLY_DIVERGED_PATH_CAP {
        let total_newly = newly_diverged_paths.len();
        newly_diverged_paths.truncate(NEWLY_DIVERGED_PATH_CAP);
        newly_diverged_paths.push(format!(
            "... (and {} more)",
            total_newly - NEWLY_DIVERGED_PATH_CAP
        ));
    }

    let summary_line = format!(
        "Replayed {total} trace(s) against base and head: \
         {passed_both} passed on both, {newly_diverged} newly diverged under head, \
         {newly_passing} newly passing (base bug fixes), {diverged_both} diverged on both, \
         {errored} errored."
    );

    let impact_percentage = if total == 0 {
        "0.0%".to_string()
    } else {
        let pct = (newly_diverged as f64 * 100.0) / total as f64;
        format!("{pct:.1}%")
    };

    TraceImpact {
        has_traces: true,
        any_newly_diverged,
        summary_line,
        impact_percentage,
        newly_diverged_paths,
    }
}

fn display_path(p: &Path) -> String {
    // Use just the file name — absolute paths are noisy for human
    // readers and unstable across machines. Operators who want the
    // full path can grep the original trace dir.
    p.file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str) -> PathBuf {
        PathBuf::from(name)
    }

    #[test]
    fn categorise_buckets_passed_both() {
        let mut base = std::collections::BTreeMap::new();
        base.insert(p("a.jsonl"), Verdict::Passed);
        let mut head = std::collections::BTreeMap::new();
        head.insert(p("a.jsonl"), Verdict::Passed);
        let impact = categorise_impact(base, head);
        assert!(!impact.any_newly_diverged);
        assert!(impact.summary_line.contains("1 passed on both"));
    }

    #[test]
    fn categorise_flags_newly_diverged() {
        let mut base = std::collections::BTreeMap::new();
        base.insert(p("safe.jsonl"), Verdict::Passed);
        base.insert(p("drift.jsonl"), Verdict::Passed);
        let mut head = std::collections::BTreeMap::new();
        head.insert(p("safe.jsonl"), Verdict::Passed);
        head.insert(p("drift.jsonl"), Verdict::Diverged);
        let impact = categorise_impact(base, head);
        assert!(impact.any_newly_diverged);
        assert_eq!(impact.newly_diverged_paths, vec!["drift.jsonl"]);
        assert!(impact.summary_line.contains("1 newly diverged"));
        assert!(
            impact.impact_percentage.starts_with("50"),
            "got: {}",
            impact.impact_percentage
        );
    }

    #[test]
    fn categorise_flags_bug_fixes_under_head() {
        let mut base = std::collections::BTreeMap::new();
        base.insert(p("was_broken.jsonl"), Verdict::Diverged);
        let mut head = std::collections::BTreeMap::new();
        head.insert(p("was_broken.jsonl"), Verdict::Passed);
        let impact = categorise_impact(base, head);
        assert!(!impact.any_newly_diverged);
        assert!(impact.summary_line.contains("1 newly passing"));
    }

    #[test]
    fn categorise_caps_the_displayed_path_list() {
        // Many newly-divergent traces → displayed list is capped,
        // trailing "and N more" notice appears, summary_line still
        // carries the full count.
        let mut base = std::collections::BTreeMap::new();
        let mut head = std::collections::BTreeMap::new();
        for i in 0..(NEWLY_DIVERGED_PATH_CAP + 5) {
            let name = format!("t{i}.jsonl");
            base.insert(p(&name), Verdict::Passed);
            head.insert(p(&name), Verdict::Diverged);
        }
        let impact = categorise_impact(base, head);
        assert!(impact.any_newly_diverged);
        assert_eq!(impact.newly_diverged_paths.len(), NEWLY_DIVERGED_PATH_CAP + 1);
        assert!(
            impact
                .newly_diverged_paths
                .last()
                .unwrap()
                .contains("and 5 more"),
            "got: {:?}",
            impact.newly_diverged_paths.last()
        );
    }
}
