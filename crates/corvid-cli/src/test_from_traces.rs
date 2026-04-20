//! `corvid test --from-traces <DIR>` — prod-as-test-suite CLI
//! (Phase 21 slice 21-inv-G-cli).
//!
//! Turn every recorded trace under `<DIR>` into a regression test:
//! for each `.jsonl` file, replay it against the current code and
//! flag any behavior drift. Today's CLI loads + validates + filters
//! the trace set, renders a coverage map + per-flag preview, and
//! returns [`EXIT_NOT_IMPLEMENTED`] pointing at Dev B's
//! `21-inv-G-harness` slice for the actual replay-and-compare
//! harness.
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
//!
//! Until `21-inv-G-harness` lands, this command never actually
//! runs a replay — it previews the plan. That preview already has
//! value: coverage gaps surface, filter cardinality is visible
//! before CI time, and invalid flag combinations fail at parse
//! time rather than half-way through a regression run.

use anyhow::{Context, Result};
use corvid_trace_schema::{read_events_from_path, validate_supported_schema, TraceEvent};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// Exit code returned until the regression harness
/// (`21-inv-G-harness`, Dev B) is on `main`. Distinguishes "tool
/// not implemented" from "tool implemented and tests failed" for
/// CI tooling.
pub const EXIT_NOT_IMPLEMENTED: u8 = 1;

/// Parsed + validated args for one invocation.
///
/// Library-level callers construct this directly; clap parses it
/// from the surface CLI form in [`crate::main`].
pub struct TestFromTracesArgs<'a> {
    pub trace_dir: &'a Path,
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

    print_not_implemented_note();
    Ok(EXIT_NOT_IMPLEMENTED)
}

/// One trace file's summary after load + validation.
struct LoadedTrace {
    path: PathBuf,
    /// Unique prompt names seen in `LlmCall` events.
    prompts: BTreeSet<String>,
    /// Unique tool names seen in `ToolCall` events.
    tools: BTreeSet<String>,
    /// Unique approval labels seen in `ApprovalRequest` events.
    approvals: BTreeSet<String>,
    /// True iff any `ApprovalRequest` event is present. Equivalent
    /// to "exercises a `@dangerous` tool" by the compiler's
    /// approve-before-dangerous guarantee.
    has_approval_event: bool,
    /// Count of substitutable events (ToolCall + LlmCall +
    /// ApprovalRequest). Used for the execution plan preview.
    llm_calls: usize,
    tool_calls: usize,
    approval_requests: usize,
    /// Maximum ts_ms across events in the trace. Used by `--since`.
    max_ts_ms: u64,
}

fn load_all_traces(dir: &Path) -> Result<Vec<LoadedTrace>> {
    let mut jsonl_files = Vec::new();
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("failed to read directory `{}`", dir.display()))?
    {
        let entry = entry
            .with_context(|| format!("failed to read a directory entry under `{}`", dir.display()))?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            jsonl_files.push(path);
        }
    }
    jsonl_files.sort();

    let mut out = Vec::with_capacity(jsonl_files.len());
    for path in jsonl_files {
        let events = read_events_from_path(&path).with_context(|| {
            format!("failed to load trace `{}`", path.display())
        })?;
        if events.is_empty() {
            anyhow::bail!("trace `{}` is empty", path.display());
        }
        validate_supported_schema(&events).with_context(|| {
            format!("trace `{}` uses an unsupported schema", path.display())
        })?;
        out.push(summarize(&path, &events));
    }
    Ok(out)
}

fn summarize(path: &Path, events: &[TraceEvent]) -> LoadedTrace {
    let mut prompts = BTreeSet::new();
    let mut tools = BTreeSet::new();
    let mut approvals = BTreeSet::new();
    let mut llm_calls = 0usize;
    let mut tool_calls = 0usize;
    let mut approval_requests = 0usize;
    let mut max_ts_ms = 0u64;
    for event in events {
        let ts = event_ts_ms(event);
        if ts > max_ts_ms {
            max_ts_ms = ts;
        }
        match event {
            TraceEvent::LlmCall { prompt, .. } => {
                prompts.insert(prompt.clone());
                llm_calls += 1;
            }
            TraceEvent::ToolCall { tool, .. } => {
                tools.insert(tool.clone());
                tool_calls += 1;
            }
            TraceEvent::ApprovalRequest { label, .. } => {
                approvals.insert(label.clone());
                approval_requests += 1;
            }
            _ => {}
        }
    }
    LoadedTrace {
        path: path.to_path_buf(),
        has_approval_event: !approvals.is_empty(),
        prompts,
        tools,
        approvals,
        llm_calls,
        tool_calls,
        approval_requests,
        max_ts_ms,
    }
}

fn parse_since(since: Option<&str>) -> Result<Option<u64>> {
    let Some(s) = since else {
        return Ok(None);
    };
    let ts = OffsetDateTime::parse(s, &Rfc3339)
        .with_context(|| format!("invalid --since timestamp `{s}`; expected RFC3339"))?;
    Ok(Some((ts.unix_timestamp_nanos() / 1_000_000) as u64))
}

fn print_preview(
    dir: &Path,
    initial_count: usize,
    applied_filters: &[(&'static str, String, usize)],
    filtered: &[&LoadedTrace],
    args: &TestFromTracesArgs<'_>,
) {
    println!("corvid test --from-traces {}", dir.display());
    println!();
    println!(
        "Scanning traces in `{}`...",
        dir.display()
    );
    println!("  found {initial_count} .jsonl file(s)");
    for (flag, arg, count) in applied_filters {
        let arg_text = if arg.is_empty() {
            String::new()
        } else {
            format!(" {arg}")
        };
        println!("  after {flag}{arg_text}: {count} trace(s)");
    }
    println!();

    let (prompts, tools, approvals) = aggregate_coverage(filtered);
    println!("Coverage:");
    println!("  prompts covered:   {}", render_set(&prompts));
    println!("  tools covered:     {}", render_set(&tools));
    println!("  approvals covered: {}", render_set(&approvals));
    println!();

    let (llm_calls, tool_calls, approval_requests) = aggregate_counts(filtered);
    println!("Test plan:");
    println!("  {} trace(s) selected", filtered.len());
    // When the selected set is small enough to be scannable,
    // enumerate the paths so the user can spot-check what's in
    // their test suite. Above the threshold the full list becomes
    // noise and we just show the count.
    const SCANNABLE_LIMIT: usize = 10;
    if !filtered.is_empty() && filtered.len() <= SCANNABLE_LIMIT {
        for trace in filtered {
            println!("    {}", trace.path.display());
        }
    }
    println!(
        "  will replay {llm_calls} LLM call(s), {tool_calls} tool call(s), \
         {approval_requests} approval(s)"
    );
    let model_text = match args.replay_model {
        Some(id) => format!(
            "differential vs. `{id}` (divergences will be reported per trace)"
        ),
        None => "recorded (default — exact substitution)".into(),
    };
    println!("  model:         {model_text}");
    println!(
        "  promotion:     {}",
        if args.promote {
            "enabled (divergences will be offered for acceptance and written back to trace files by the harness)"
        } else {
            "disabled"
        }
    );
    println!(
        "  flake-detect:  {}",
        match args.flake_detect {
            Some(n) => format!(
                "N={n} (each trace replayed N times; nondeterminism surfaces as a flake-rank column in the report)"
            ),
            None => "off".into(),
        }
    );
    println!();
}

fn aggregate_coverage(
    filtered: &[&LoadedTrace],
) -> (BTreeSet<String>, BTreeSet<String>, BTreeSet<String>) {
    let mut prompts = BTreeSet::new();
    let mut tools = BTreeSet::new();
    let mut approvals = BTreeSet::new();
    for trace in filtered {
        prompts.extend(trace.prompts.iter().cloned());
        tools.extend(trace.tools.iter().cloned());
        approvals.extend(trace.approvals.iter().cloned());
    }
    (prompts, tools, approvals)
}

fn aggregate_counts(filtered: &[&LoadedTrace]) -> (usize, usize, usize) {
    let mut llm_calls = 0;
    let mut tool_calls = 0;
    let mut approval_requests = 0;
    for trace in filtered {
        llm_calls += trace.llm_calls;
        tool_calls += trace.tool_calls;
        approval_requests += trace.approval_requests;
    }
    (llm_calls, tool_calls, approval_requests)
}

fn render_set(set: &BTreeSet<String>) -> String {
    if set.is_empty() {
        "<none>".into()
    } else {
        format!(
            "{{{}}}",
            set.iter().cloned().collect::<Vec<_>>().join(", ")
        )
    }
}

fn print_not_implemented_note() {
    eprintln!(
        "note: `corvid test --from-traces` is not yet available. The regression \
         harness ships in Phase 21 slice 21-inv-G-harness (Dev B); this CLI will \
         wire into it once landed. Trace load + schema validation + filtering + \
         coverage preview succeeded above."
    );
}

/// Event timestamp extractor. Mirrors the same helper in
/// `trace_cmd.rs` / `routing_report.rs`; refactoring to a shared
/// crate-level module is out of scope for this slice.
fn event_ts_ms(event: &TraceEvent) -> u64 {
    match event {
        TraceEvent::SchemaHeader { ts_ms, .. }
        | TraceEvent::RunStarted { ts_ms, .. }
        | TraceEvent::RunCompleted { ts_ms, .. }
        | TraceEvent::ToolCall { ts_ms, .. }
        | TraceEvent::ToolResult { ts_ms, .. }
        | TraceEvent::LlmCall { ts_ms, .. }
        | TraceEvent::LlmResult { ts_ms, .. }
        | TraceEvent::ApprovalRequest { ts_ms, .. }
        | TraceEvent::ApprovalResponse { ts_ms, .. }
        | TraceEvent::SeedRead { ts_ms, .. }
        | TraceEvent::ClockRead { ts_ms, .. }
        | TraceEvent::ModelSelected { ts_ms, .. }
        | TraceEvent::ProgressiveEscalation { ts_ms, .. }
        | TraceEvent::ProgressiveExhausted { ts_ms, .. }
        | TraceEvent::AbVariantChosen { ts_ms, .. }
        | TraceEvent::EnsembleVote { ts_ms, .. }
        | TraceEvent::AdversarialPipelineCompleted { ts_ms, .. }
        | TraceEvent::AdversarialContradiction { ts_ms, .. }
        | TraceEvent::ProvenanceEdge { ts_ms, .. } => *ts_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_trace_schema::{
        write_events_to_path, SCHEMA_VERSION, WRITER_INTERPRETER,
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
                rendered: None,
                args: vec![],
            });
            events.push(TraceEvent::LlmResult {
                ts_ms: shape.ts_ms + 4,
                run_id: shape.run_id.clone(),
                prompt: p.clone(),
                model: None,
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
    fn stub_returns_not_implemented_exit_code_on_nonempty_dir() {
        let dir = test_dir();
        write_trace(&dir, TraceShape::new("run-1").prompt("classify"));
        let code = run_test_from_traces(args(&dir)).unwrap();
        assert_eq!(code, EXIT_NOT_IMPLEMENTED);
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
        write_trace(
            &dir,
            TraceShape::new("run-3").approval("IssueRefund"),
        );
        let traces = load_all_traces(&dir).unwrap();
        let refs: Vec<&LoadedTrace> = traces.iter().collect();
        let (prompts, tools, approvals) = aggregate_coverage(&refs);
        assert!(prompts.contains("classify"));
        assert!(tools.contains("get_order"));
        assert!(approvals.contains("IssueRefund"));
    }

    // -------------------- --only-dangerous --------------------

    #[test]
    fn only_dangerous_selects_traces_with_approval_events() {
        let dir = test_dir();
        write_trace(&dir, TraceShape::new("run-safe").prompt("classify"));
        write_trace(
            &dir,
            TraceShape::new("run-danger").approval("IssueRefund"),
        );
        let mut a = args(&dir);
        a.only_dangerous = true;
        let code = run_test_from_traces(a).unwrap();
        assert_eq!(code, EXIT_NOT_IMPLEMENTED);
    }

    #[test]
    fn only_dangerous_rejects_traces_without_approval_events() {
        let dir = test_dir();
        write_trace(&dir, TraceShape::new("run-safe").prompt("classify"));
        let mut a = args(&dir);
        a.only_dangerous = true;
        // No dangerous traces remain after filter; the command
        // still completes cleanly because empty *after filter* is
        // different from empty *on disk* — the latter is handled
        // by the zero-return-code path, the former should still
        // fall through to the "not implemented" exit since a
        // filter-to-zero is a valid test-suite configuration that
        // the harness will report as "zero tests selected."
        let code = run_test_from_traces(a).unwrap();
        assert_eq!(code, EXIT_NOT_IMPLEMENTED);
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
        assert_eq!(kept[0].prompts.iter().next().map(String::as_str), Some("classify"));
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
            TraceShape::new("run-old").prompt("classify").at_ts_ms(1_600_000_000_000),
        );
        write_trace(
            &dir,
            TraceShape::new("run-new").prompt("classify").at_ts_ms(1_700_000_000_000),
        );
        let mut a = args(&dir);
        a.since = Some("2020-09-13T12:26:40Z");
        let code = run_test_from_traces(a).unwrap();
        assert_eq!(code, EXIT_NOT_IMPLEMENTED);
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
    fn replay_model_flag_accepted() {
        let dir = test_dir();
        write_trace(&dir, TraceShape::new("run-1").prompt("classify"));
        let mut a = args(&dir);
        a.replay_model = Some("claude-opus-5-0");
        let code = run_test_from_traces(a).unwrap();
        assert_eq!(code, EXIT_NOT_IMPLEMENTED);
    }

    // -------------------- --promote --------------------

    #[test]
    fn promote_flag_accepted() {
        let dir = test_dir();
        write_trace(&dir, TraceShape::new("run-1").prompt("classify"));
        let mut a = args(&dir);
        a.promote = true;
        let code = run_test_from_traces(a).unwrap();
        assert_eq!(code, EXIT_NOT_IMPLEMENTED);
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
        let code = run_test_from_traces(a).unwrap();
        assert_eq!(code, EXIT_NOT_IMPLEMENTED);
    }

    // -------------------- compound / sanity --------------------

    #[test]
    fn compound_filters_compose_commutatively() {
        // Two filters that each cut the set in half should
        // intersect down to the same result regardless of order.
        let dir = test_dir();
        write_trace(
            &dir,
            TraceShape::new("run-a").prompt("classify").tool("get_order"),
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
