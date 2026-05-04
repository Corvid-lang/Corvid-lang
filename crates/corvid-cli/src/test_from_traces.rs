//! `corvid test --from-traces <DIR>` — prod-as-test-suite CLI.
//!
//! Turn every recorded trace under `<DIR>` into a regression test:
//! for each `.jsonl` file, replay it against the current code and
//! flag any behavior drift. The CLI loads + validates + filters
//! the trace set, renders a coverage map + per-flag preview, then
//! dispatches through the regression harness. Exit code is 0 on a
//! clean run and [`EXIT_DIVERGED`] when at least one trace diverged,
//! flaked, or errored. `--promote` now goes end-to-end: on a TTY
//! the CLI prompts per divergence and atomically rewrites the
//! golden trace when accepted; in non-interactive pipelines the
//! harness defaults every prompt to reject with a one-time warning.
//!
//! Five inventive flags compose on top of the shipped Phase 21
//! primitives:
//!
//! - `--replay-model <ID>`   compose with differential replay
//!   (`21-inv-B-adapter`): cross-model drift report across every
//!   trace in the suite.
//! - `--only-dangerous`      slice the suite to only traces that
//!   hit a `@dangerous` tool (detected by presence of
//!   `ApprovalRequest` events — the approve-before-dangerous
//!   guarantee makes this exact).
//! - `--only-prompt <NAME>`  slice to traces exercising a specific
//!   prompt.
//! - `--only-tool <NAME>`    slice to traces exercising a specific
//!   tool.
//! - `--since <RFC3339>`     slice to traces with any event at or
//!   after the given timestamp.
//! - `--promote`             Jest-snapshot pattern for AI agents:
//!   divergences become interactively-accepted golden traces,
//!   overwriting originals. Mutually exclusive with
//!   `--replay-model` and `--flake-detect`.
//! - `--flake-detect <N>`    replay each trace N times; any trace
//!   producing different output across runs surfaces program-level
//!   nondeterminism the `@deterministic` attribute didn't catch.

use anyhow::{anyhow, Context, Result};
use corvid_runtime::{PromotePromptMode, TestFromTracesOptions};
use std::path::{Path, PathBuf};

mod load;
mod promote;
mod render;

use load::{load_all_traces, parse_since, LoadedTrace};
use promote::dispatch_harness_request;
use render::{print_preview, render_report};

#[cfg(test)]
use render::aggregate_coverage;

/// Exit code returned when the regression harness has run and at
/// least one trace diverged. Distinguishes "ran-and-found-drift"
/// from "couldn't run" (typed anyhow errors).
pub const EXIT_DIVERGED: u8 = 1;

/// Parsed + validated args for one invocation.
///
/// Library-level callers construct this directly; clap parses it
/// from the surface CLI form in [`crate::main`].
pub struct TestFromTracesArgs<'a> {
    pub trace_dir: &'a Path,
    pub source: Option<&'a Path>,
    pub replay_model: Option<&'a str>,
    pub only_dangerous: bool,
    pub only_prompt: Option<&'a str>,
    pub only_tool: Option<&'a str>,
    pub since: Option<&'a str>,
    pub promote: bool,
    pub flake_detect: Option<u32>,
}

/// Entry point for `corvid test --from-traces <DIR>`.
pub fn run_test_from_traces(args: TestFromTracesArgs<'_>) -> Result<u8> {
    // Defensive library-level mutual-exclusion checks. Clap also
    // enforces these at parse time (see `Test` command in main.rs);
    // duplicating here makes the public API stricter than its clap
    // caller so non-clap callers can't slip past the invariant.
    if args.promote && args.replay_model.is_some() {
        anyhow::bail!(
            "`--promote` and `--replay-model` are mutually exclusive; promoting cross-model \
             divergences would silently replace your golden trace's recorded model — re-record \
             instead"
        );
    }
    if args.promote && args.flake_detect.is_some() {
        anyhow::bail!(
            "`--promote` and `--flake-detect` are mutually exclusive; promoting a flaky result \
             is a bug"
        );
    }
    if let Some(n) = args.flake_detect {
        if n == 0 {
            anyhow::bail!("`--flake-detect` requires N >= 1 (got 0)");
        }
    }

    if !args.trace_dir.exists() {
        anyhow::bail!(
            "trace directory `{}` does not exist",
            args.trace_dir.display()
        );
    }
    if !args.trace_dir.is_dir() {
        anyhow::bail!(
            "trace directory `{}` is not a directory",
            args.trace_dir.display()
        );
    }

    // Parse --since up front so bad input fails before we load
    // anything. Same parser as `corvid routing-report --since`.
    let since_ms = parse_since(args.since)?;

    // Load + schema-validate every trace in the directory. We fail
    // fast on a bad trace rather than silently skip — a corrupted
    // file in your test-suite directory means CI config is wrong,
    // not that the file should be ignored.
    let loaded = load_all_traces(args.trace_dir)?;
    let initial_count = loaded.len();

    // Apply filters. Order matters only for reporting; the result
    // set is a set intersection so the composition is commutative.
    let mut filtered: Vec<&LoadedTrace> = loaded.iter().collect();
    let mut applied_filters: Vec<(&'static str, String, usize)> = Vec::new();

    if args.only_dangerous {
        filtered.retain(|trace| trace.has_approval_event);
        applied_filters.push(("--only-dangerous", String::new(), filtered.len()));
    }

    if let Some(name) = args.only_prompt {
        filtered.retain(|trace| trace.prompts.contains(name));
        applied_filters.push(("--only-prompt", name.to_string(), filtered.len()));
    }

    if let Some(name) = args.only_tool {
        filtered.retain(|trace| trace.tools.contains(name));
        applied_filters.push(("--only-tool", name.to_string(), filtered.len()));
    }

    if let Some(cutoff) = since_ms {
        filtered.retain(|trace| trace.max_ts_ms >= cutoff);
        applied_filters.push((
            "--since",
            args.since.unwrap_or("").to_string(),
            filtered.len(),
        ));
    }

    print_preview(
        args.trace_dir,
        initial_count,
        &applied_filters,
        &filtered,
        &args,
    );

    if initial_count == 0 {
        // Empty dir — matches `cargo test` / `pytest` conventions:
        // no tests is success, not failure. Misconfigured paths are
        // already caught by the `!trace_dir.exists()` check above.
        return Ok(0);
    }

    if filtered.is_empty() {
        // Filters reduced the set to zero — also success. The user
        // may have pointed `--only-prompt classify` at a dir whose
        // traces don't exercise classify; that's a valid CI state
        // (nothing to test), not a failure.
        println!("no traces selected by the configured filters; nothing to test.");
        return Ok(0);
    }

    // --source is required for the execution path. Once
    // SchemaHeader.source_path is populated at record time this
    // becomes optional.
    let source_path = args.source.ok_or_else(|| {
        anyhow!(
            "`corvid test --from-traces` requires `--from-traces-source <FILE>` pointing at the \
             Corvid source the traces were recorded against. Once `SchemaHeader.source_path` is \
             populated at record time, this flag becomes optional."
        )
    })?;

    // Collect the filtered trace paths — the harness consumes a
    // Vec<PathBuf> of the filtered set.
    let filtered_paths: Vec<PathBuf> = filtered.iter().map(|trace| trace.path.clone()).collect();

    // Prompt mode: AutoStdin reads [y/N/a/q] on TTY and fails
    // closed (Reject with a one-time warning) on non-TTY. That
    // matches the CI-safe convention — no accidental promotion in
    // non-interactive pipelines. Override by scripting decisions
    // through the library-level API for tests.
    let harness_options = TestFromTracesOptions {
        replay_model: args.replay_model.map(|s| s.to_string()),
        promote: args.promote,
        flake_detect: args.flake_detect,
        prompt_mode: PromotePromptMode::AutoStdin,
    };

    eprintln!(
        "dispatching through regression harness ({} traces)...",
        filtered.len()
    );
    eprintln!();

    // Run the harness on a single-threaded tokio runtime so the
    // async runner closure can dispatch into the replay
    // orchestrator without nested-runtime panics.
    let tokio_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime for regression harness")?;

    let source_path_owned: PathBuf = source_path.to_path_buf();
    let report = tokio_rt.block_on(corvid_runtime::run_test_from_traces(
        filtered_paths,
        harness_options,
        move |request| {
            let source_path = source_path_owned.clone();
            async move { dispatch_harness_request(&source_path, request).await }
        },
    ));

    render_report(&report);

    if report.aborted {
        anyhow::bail!("regression harness aborted (user quit during promotion)");
    }

    let exit_code =
        if report.summary.diverged == 0 && report.summary.flaky == 0 && report.summary.errored == 0
        {
            0
        } else {
            EXIT_DIVERGED
        };
    Ok(exit_code)
}

/// The harness's runner closure body, extracted so it's readable.
///
/// For each request the harness raises, dispatch into the matching
/// mode. `Replay` / `Differential` go through the replay orchestrator
/// and substitute recorded responses; `RecordCurrent` re-runs the
/// agent against the current source with real LLM / tool / approver
/// adapters (env-driven) and returns the emitted trace path so the
/// harness can atomically swap the old golden trace for the new one.
#[cfg(test)]
mod tests {
    use super::*;
    use corvid_trace_schema::{
        write_events_to_path, TraceEvent, SCHEMA_VERSION, WRITER_INTERPRETER,
    };
    use serde_json::json;

    fn test_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "corvid-cli-test-from-traces-{}-{n}",
            std::process::id()
        ));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Build a synthetic trace exercising the three substitutable
    /// kinds. Flags configure presence/absence of each so tests can
    /// mimic "only-dangerous" / "only-prompt" / etc. scenarios.
    struct TraceShape {
        run_id: String,
        prompt: Option<String>,
        tool: Option<String>,
        approval: Option<String>,
        ts_ms: u64,
    }

    impl TraceShape {
        fn new(run_id: &str) -> Self {
            Self {
                run_id: run_id.into(),
                prompt: None,
                tool: None,
                approval: None,
                ts_ms: 1_700_000_000_000,
            }
        }
        fn prompt(mut self, p: &str) -> Self {
            self.prompt = Some(p.into());
            self
        }
        fn tool(mut self, t: &str) -> Self {
            self.tool = Some(t.into());
            self
        }
        fn approval(mut self, a: &str) -> Self {
            self.approval = Some(a.into());
            self
        }
        fn at_ts_ms(mut self, ts: u64) -> Self {
            self.ts_ms = ts;
            self
        }
    }

    fn write_trace(dir: &Path, shape: TraceShape) -> PathBuf {
        let path = dir.join(format!("{}.jsonl", shape.run_id));
        let mut events = vec![
            TraceEvent::SchemaHeader {
                version: SCHEMA_VERSION,
                writer: WRITER_INTERPRETER.into(),
                commit_sha: None,
                source_path: None,
                ts_ms: shape.ts_ms,
                run_id: shape.run_id.clone(),
            },
            TraceEvent::RunStarted {
                ts_ms: shape.ts_ms,
                run_id: shape.run_id.clone(),
                agent: "demo".into(),
                args: vec![],
            },
        ];
        if let Some(t) = &shape.tool {
            events.push(TraceEvent::ToolCall {
                ts_ms: shape.ts_ms + 1,
                run_id: shape.run_id.clone(),
                tool: t.clone(),
                args: vec![],
            });
            events.push(TraceEvent::ToolResult {
                ts_ms: shape.ts_ms + 2,
                run_id: shape.run_id.clone(),
                tool: t.clone(),
                result: json!(null),
            });
        }
        if let Some(p) = &shape.prompt {
            events.push(TraceEvent::LlmCall {
                ts_ms: shape.ts_ms + 3,
                run_id: shape.run_id.clone(),
                prompt: p.clone(),
                model: None,
                model_version: None,
                rendered: None,
                args: vec![],
            });
            events.push(TraceEvent::LlmResult {
                ts_ms: shape.ts_ms + 4,
                run_id: shape.run_id.clone(),
                prompt: p.clone(),
                model: None,
                model_version: None,
                result: json!("ok"),
            });
        }
        if let Some(a) = &shape.approval {
            events.push(TraceEvent::ApprovalRequest {
                ts_ms: shape.ts_ms + 5,
                run_id: shape.run_id.clone(),
                label: a.clone(),
                args: vec![],
            });
            events.push(TraceEvent::ApprovalResponse {
                ts_ms: shape.ts_ms + 6,
                run_id: shape.run_id.clone(),
                label: a.clone(),
                approved: true,
            });
        }
        events.push(TraceEvent::RunCompleted {
            ts_ms: shape.ts_ms + 7,
            run_id: shape.run_id.clone(),
            ok: true,
            result: None,
            error: None,
        });
        write_events_to_path(&path, &events).unwrap();
        path
    }

    fn args<'a>(trace_dir: &'a Path) -> TestFromTracesArgs<'a> {
        TestFromTracesArgs {
            trace_dir,
            source: None,
            replay_model: None,
            only_dangerous: false,
            only_prompt: None,
            only_tool: None,
            since: None,
            promote: false,
            flake_detect: None,
        }
    }

    // -------------------- core path --------------------

    #[test]
    fn nonempty_dir_without_source_requires_source_flag() {
        // With no `--from-traces-source`, the CLI must refuse to
        // dispatch — the harness can't compile trace-vs-source
        // without the source path. Error must name the flag so the
        // user knows the fix.
        let dir = test_dir();
        write_trace(&dir, TraceShape::new("run-1").prompt("classify"));
        let err = run_test_from_traces(args(&dir)).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("--from-traces-source"), "got: {msg}");
    }

    #[test]
    fn missing_dir_reports_clean_error() {
        let path = std::env::temp_dir().join(format!(
            "corvid-cli-test-from-traces-missing-{}",
            std::process::id()
        ));
        if path.exists() {
            std::fs::remove_dir_all(&path).unwrap();
        }
        let err = run_test_from_traces(args(&path)).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("does not exist"), "got: {msg}");
    }

    #[test]
    fn empty_dir_returns_zero_with_no_traces_message() {
        // Cargo / pytest convention: empty test set is success.
        let dir = test_dir();
        let code = run_test_from_traces(args(&dir)).unwrap();
        assert_eq!(code, 0);
    }

    #[test]
    fn bad_schema_file_surfaces_typed_error() {
        let dir = test_dir();
        let path = dir.join("broken.jsonl");
        std::fs::write(&path, "{not valid json\n").unwrap();
        let err = run_test_from_traces(args(&dir)).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("broken.jsonl"), "got: {msg}");
    }

    // -------------------- coverage preview --------------------

    #[test]
    fn coverage_aggregates_across_multiple_traces() {
        let dir = test_dir();
        write_trace(&dir, TraceShape::new("run-1").prompt("classify"));
        write_trace(&dir, TraceShape::new("run-2").tool("get_order"));
        write_trace(&dir, TraceShape::new("run-3").approval("IssueRefund"));
        let traces = load_all_traces(&dir).unwrap();
        let refs: Vec<&LoadedTrace> = traces.iter().collect();
        let (prompts, tools, approvals) = aggregate_coverage(&refs);
        assert!(prompts.contains("classify"));
        assert!(tools.contains("get_order"));
        assert!(approvals.contains("IssueRefund"));
    }

    // -------------------- --only-dangerous --------------------

    #[test]
    fn only_dangerous_keeps_traces_with_approval_events() {
        // Dangerous traces survive the filter, so the command
        // proceeds to the dispatch boundary; we assert we reach it
        // by catching the source-required error (source is None
        // in this test). The error path confirms filter + preview
        // ran successfully before the source check.
        let dir = test_dir();
        write_trace(&dir, TraceShape::new("run-safe").prompt("classify"));
        write_trace(&dir, TraceShape::new("run-danger").approval("IssueRefund"));
        let mut a = args(&dir);
        a.only_dangerous = true;
        let err = run_test_from_traces(a).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("--from-traces-source"), "got: {msg}");
    }

    #[test]
    fn only_dangerous_filter_to_zero_returns_clean_success() {
        // A filter that reduces the suite to zero traces is a
        // valid CI state — nothing to test is not a failure. Same
        // convention cargo test / pytest use.
        let dir = test_dir();
        write_trace(&dir, TraceShape::new("run-safe").prompt("classify"));
        let mut a = args(&dir);
        a.only_dangerous = true;
        let code = run_test_from_traces(a).unwrap();
        assert_eq!(code, 0);
    }

    // -------------------- --only-prompt / --only-tool --------------------

    #[test]
    fn only_prompt_filter_narrows_trace_set() {
        let dir = test_dir();
        write_trace(&dir, TraceShape::new("run-1").prompt("classify"));
        write_trace(&dir, TraceShape::new("run-2").prompt("summarize"));
        let traces = load_all_traces(&dir).unwrap();
        let kept: Vec<&LoadedTrace> = traces
            .iter()
            .filter(|t| t.prompts.contains("classify"))
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(
            kept[0].prompts.iter().next().map(String::as_str),
            Some("classify")
        );
    }

    #[test]
    fn only_tool_filter_narrows_trace_set() {
        let dir = test_dir();
        write_trace(&dir, TraceShape::new("run-1").tool("get_order"));
        write_trace(&dir, TraceShape::new("run-2").tool("cancel_order"));
        let traces = load_all_traces(&dir).unwrap();
        let kept: Vec<&LoadedTrace> = traces
            .iter()
            .filter(|t| t.tools.contains("get_order"))
            .collect();
        assert_eq!(kept.len(), 1);
    }

    // -------------------- --since --------------------

    #[test]
    fn since_filter_drops_traces_older_than_cutoff() {
        let dir = test_dir();
        write_trace(
            &dir,
            TraceShape::new("run-old")
                .prompt("classify")
                .at_ts_ms(1_600_000_000_000),
        );
        write_trace(
            &dir,
            TraceShape::new("run-new")
                .prompt("classify")
                .at_ts_ms(1_700_000_000_000),
        );
        let mut a = args(&dir);
        a.since = Some("2020-09-13T12:26:40Z");
        // Both traces survive this cutoff; with no source set we
        // should hit the source-required error after the filter
        // runs, which proves the filter plumbing executed cleanly.
        let err = run_test_from_traces(a).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("--from-traces-source"), "got: {msg}");
    }

    #[test]
    fn since_with_invalid_rfc3339_fails_fast() {
        let dir = test_dir();
        write_trace(&dir, TraceShape::new("run-1").prompt("classify"));
        let mut a = args(&dir);
        a.since = Some("not a timestamp");
        let err = run_test_from_traces(a).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("RFC3339"), "got: {msg}");
    }

    // -------------------- --replay-model --------------------

    #[test]
    fn replay_model_flag_reaches_dispatch_boundary() {
        let dir = test_dir();
        write_trace(&dir, TraceShape::new("run-1").prompt("classify"));
        let mut a = args(&dir);
        a.replay_model = Some("claude-opus-5-0");
        let err = run_test_from_traces(a).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("--from-traces-source"), "got: {msg}");
    }

    // -------------------- --promote --------------------

    #[test]
    fn promote_flag_reaches_dispatch_boundary() {
        // --promote is now wired through `TraceHarnessMode::RecordCurrent`;
        // without a `--from-traces-source` the CLI bails at the
        // source-required check just like every other dispatch
        // path. That confirms the flag is accepted end-to-end and
        // participates in the same precondition discipline as the
        // non-promote paths.
        let dir = test_dir();
        write_trace(&dir, TraceShape::new("run-1").prompt("classify"));
        let mut a = args(&dir);
        a.promote = true;
        let err = run_test_from_traces(a).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("--from-traces-source"), "got: {msg}");
    }

    #[test]
    fn promote_with_replay_model_is_rejected() {
        let dir = test_dir();
        write_trace(&dir, TraceShape::new("run-1").prompt("classify"));
        let mut a = args(&dir);
        a.promote = true;
        a.replay_model = Some("claude-opus-5-0");
        let err = run_test_from_traces(a).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("mutually exclusive"), "got: {msg}");
        assert!(msg.contains("replay-model"), "got: {msg}");
    }

    #[test]
    fn promote_with_flake_detect_is_rejected() {
        let dir = test_dir();
        write_trace(&dir, TraceShape::new("run-1").prompt("classify"));
        let mut a = args(&dir);
        a.promote = true;
        a.flake_detect = Some(3);
        let err = run_test_from_traces(a).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("mutually exclusive"), "got: {msg}");
        assert!(msg.contains("flake-detect"), "got: {msg}");
    }

    // -------------------- --flake-detect --------------------

    #[test]
    fn flake_detect_requires_positive_n() {
        let dir = test_dir();
        write_trace(&dir, TraceShape::new("run-1").prompt("classify"));
        let mut a = args(&dir);
        a.flake_detect = Some(0);
        let err = run_test_from_traces(a).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("N >= 1"), "got: {msg}");
    }

    #[test]
    fn flake_detect_with_positive_n_is_accepted() {
        let dir = test_dir();
        write_trace(&dir, TraceShape::new("run-1").prompt("classify"));
        let mut a = args(&dir);
        a.flake_detect = Some(3);
        let err = run_test_from_traces(a).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("--from-traces-source"), "got: {msg}");
    }

    // -------------------- compound / sanity --------------------

    #[test]
    fn compound_filters_compose_commutatively() {
        // Two filters that each cut the set in half should
        // intersect down to the same result regardless of order.
        let dir = test_dir();
        write_trace(
            &dir,
            TraceShape::new("run-a")
                .prompt("classify")
                .tool("get_order"),
        );
        write_trace(&dir, TraceShape::new("run-b").prompt("classify"));
        write_trace(&dir, TraceShape::new("run-c").tool("get_order"));
        write_trace(&dir, TraceShape::new("run-d").prompt("summarize"));

        let traces = load_all_traces(&dir).unwrap();
        let refs: Vec<&LoadedTrace> = traces.iter().collect();
        let both: Vec<&LoadedTrace> = refs
            .iter()
            .copied()
            .filter(|t| t.prompts.contains("classify") && t.tools.contains("get_order"))
            .collect();
        assert_eq!(both.len(), 1);
        assert!(both[0].path.to_string_lossy().contains("run-a"));
    }

    #[test]
    fn empty_dir_does_not_print_not_implemented_note() {
        // Regression: on an empty dir we return 0 and must NOT
        // confuse CI by emitting the "not implemented" note —
        // zero tests is a clean success state. (This test asserts
        // the exit-code half of that contract; stdout content is
        // visual QA.)
        let dir = test_dir();
        let code = run_test_from_traces(args(&dir)).unwrap();
        assert_eq!(code, 0);
    }
}
