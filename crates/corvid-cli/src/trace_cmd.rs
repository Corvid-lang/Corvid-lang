//! `corvid trace list` and `corvid trace show` — inspect recorded
//! traces under `target/trace/`.
//!
//! These commands work against any trace file recorded by
//! any runtime tier (once Dev B's `21-B-rec-interp` +
//! `21-B-rec-native` land), and pre-21-A-schema legacy traces
//! that don't carry a `SchemaHeader`. The only requirement is
//! that the file is line-delimited JSON of `TraceEvent` values.

use anyhow::{Context, Result};
use corvid_trace_schema::{
    read_events_from_path, schema_version_of, validate_supported_schema, TraceEvent,
};
use std::path::{Path, PathBuf};

/// Default trace directory, relative to the working directory.
/// Callers override with `--trace-dir`.
const DEFAULT_TRACE_DIR: &str = "target/trace";

/// Entry for `corvid trace list [--trace-dir <path>]`.
pub fn run_list(trace_dir: Option<&Path>) -> Result<u8> {
    let dir = trace_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_TRACE_DIR));

    if !dir.exists() {
        println!("no traces at `{}` (directory does not exist)", dir.display());
        return Ok(0);
    }

    let files = collect_trace_files(&dir)
        .with_context(|| format!("failed to scan `{}`", dir.display()))?;

    if files.is_empty() {
        println!("no traces at `{}`", dir.display());
        return Ok(0);
    }

    print_list_header();
    for file in &files {
        let entry = summarize_trace(file);
        print_list_row(&entry);
    }
    Ok(0)
}

/// Entry for `corvid trace show <id-or-path> [--trace-dir <path>]`.
///
/// `id_or_path` is either a direct file path (e.g.
/// `target/trace/run-1.jsonl`) or a run id (e.g. `run-1700000000000`)
/// resolved against `trace_dir`.
pub fn run_show(id_or_path: &str, trace_dir: Option<&Path>) -> Result<u8> {
    let path = resolve_trace_path(id_or_path, trace_dir)
        .with_context(|| format!("failed to locate trace `{id_or_path}`"))?;

    let events = read_events_from_path(&path).with_context(|| {
        format!("failed to read trace at `{}`", path.display())
    })?;

    if events.is_empty() {
        anyhow::bail!("trace `{}` is empty", path.display());
    }

    // Warn but don't fail on unsupported schema — the user asked
    // to see the file, so show it even if the binary can't replay
    // it.
    if let Err(err) = validate_supported_schema(&events) {
        eprintln!("warning: {err}");
    }

    for event in &events {
        let pretty = serde_json::to_string_pretty(event)
            .unwrap_or_else(|_| format!("{event:?}"));
        println!("{pretty}");
    }
    Ok(0)
}

/// One row in `corvid trace list`'s output.
struct TraceSummary {
    path: PathBuf,
    run_id: Option<String>,
    schema: Option<u32>,
    event_count: usize,
    first_ts_ms: Option<u64>,
    last_ts_ms: Option<u64>,
    error: Option<String>,
}

fn summarize_trace(path: &Path) -> TraceSummary {
    match read_events_from_path(path) {
        Ok(events) => TraceSummary {
            path: path.to_path_buf(),
            run_id: run_id_of(&events).map(|s| s.to_string()),
            schema: schema_version_of(&events),
            event_count: events.len(),
            first_ts_ms: first_ts(&events),
            last_ts_ms: last_ts(&events),
            error: None,
        },
        Err(err) => TraceSummary {
            path: path.to_path_buf(),
            run_id: None,
            schema: None,
            event_count: 0,
            first_ts_ms: None,
            last_ts_ms: None,
            error: Some(err.to_string()),
        },
    }
}

fn print_list_header() {
    println!(
        "{:<36} {:<8} {:>8} {:>14} {:>14}  {}",
        "run_id", "schema", "events", "first_ts_ms", "last_ts_ms", "path"
    );
    println!(
        "{:-<36} {:-<8} {:->8} {:->14} {:->14}  {:-<20}",
        "", "", "", "", "", ""
    );
}

fn print_list_row(entry: &TraceSummary) {
    if let Some(err) = &entry.error {
        println!(
            "{:<36} {:<8} {:>8} {:>14} {:>14}  {}  [{}]",
            "<read failed>",
            "?",
            "?",
            "?",
            "?",
            entry.path.display(),
            err
        );
        return;
    }
    let run_id = entry.run_id.as_deref().unwrap_or("<unknown>");
    let schema = entry
        .schema
        .map(|v| format!("v{v}"))
        .unwrap_or_else(|| "legacy".into());
    let first = entry
        .first_ts_ms
        .map(|ts| ts.to_string())
        .unwrap_or_else(|| "-".into());
    let last = entry
        .last_ts_ms
        .map(|ts| ts.to_string())
        .unwrap_or_else(|| "-".into());
    println!(
        "{:<36} {:<8} {:>8} {:>14} {:>14}  {}",
        run_id,
        schema,
        entry.event_count,
        first,
        last,
        entry.path.display()
    );
}

/// Enumerate every `.jsonl` file in `dir` (non-recursive). Kept
/// shallow on purpose: `target/trace/` is flat today and the UX
/// is "one trace per file right at this directory."
fn collect_trace_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// Resolve a user-supplied identifier to a trace file path. If
/// the input is already a path on disk, return it. Otherwise,
/// treat it as a run id and look for `<trace_dir>/<id>.jsonl`.
fn resolve_trace_path(id_or_path: &str, trace_dir: Option<&Path>) -> Result<PathBuf> {
    let as_path = PathBuf::from(id_or_path);
    if as_path.is_file() {
        return Ok(as_path);
    }
    let dir = trace_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_TRACE_DIR));
    let candidate = dir.join(format!("{id_or_path}.jsonl"));
    if candidate.is_file() {
        return Ok(candidate);
    }
    anyhow::bail!(
        "no trace matches `{id_or_path}` (looked at the path itself and at `{}`)",
        candidate.display()
    )
}

fn run_id_of(events: &[TraceEvent]) -> Option<&str> {
    events.iter().find_map(event_run_id)
}

fn event_run_id(event: &TraceEvent) -> Option<&str> {
    match event {
        TraceEvent::SchemaHeader { run_id, .. }
        | TraceEvent::RunStarted { run_id, .. }
        | TraceEvent::RunCompleted { run_id, .. }
        | TraceEvent::ToolCall { run_id, .. }
        | TraceEvent::ToolResult { run_id, .. }
        | TraceEvent::LlmCall { run_id, .. }
        | TraceEvent::LlmResult { run_id, .. }
        | TraceEvent::ApprovalRequest { run_id, .. }
        | TraceEvent::ApprovalResponse { run_id, .. }
        | TraceEvent::SeedRead { run_id, .. }
        | TraceEvent::ClockRead { run_id, .. }
        | TraceEvent::ModelSelected { run_id, .. }
        | TraceEvent::ProgressiveEscalation { run_id, .. }
        | TraceEvent::ProgressiveExhausted { run_id, .. }
        | TraceEvent::AbVariantChosen { run_id, .. }
        | TraceEvent::EnsembleVote { run_id, .. }
        | TraceEvent::AdversarialPipelineCompleted { run_id, .. }
        | TraceEvent::AdversarialContradiction { run_id, .. }
        | TraceEvent::ProvenanceEdge { run_id, .. } => Some(run_id.as_str()),
    }
}

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

fn first_ts(events: &[TraceEvent]) -> Option<u64> {
    events.first().map(event_ts_ms)
}

fn last_ts(events: &[TraceEvent]) -> Option<u64> {
    events.last().map(event_ts_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_trace_schema::{
        write_events_to_path, SCHEMA_VERSION, WRITER_INTERPRETER,
    };

    fn test_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "corvid-cli-trace-test-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_sample(dir: &Path, run_id: &str) -> PathBuf {
        let path = dir.join(format!("{run_id}.jsonl"));
        let events = vec![
            TraceEvent::SchemaHeader {
                version: SCHEMA_VERSION,
                writer: WRITER_INTERPRETER.into(),
                commit_sha: None,
                source_path: None,
                ts_ms: 1,
                run_id: run_id.into(),
            },
            TraceEvent::RunStarted {
                ts_ms: 2,
                run_id: run_id.into(),
                agent: "demo".into(),
                args: vec![],
            },
            TraceEvent::RunCompleted {
                ts_ms: 5,
                run_id: run_id.into(),
                ok: true,
                result: None,
                error: None,
            },
        ];
        write_events_to_path(&path, &events).unwrap();
        path
    }

    #[test]
    fn resolve_finds_direct_path() {
        let dir = test_dir();
        let path = write_sample(&dir, "run-direct");
        let resolved = resolve_trace_path(path.to_str().unwrap(), None).unwrap();
        assert_eq!(resolved, path);
    }

    #[test]
    fn resolve_finds_by_run_id_under_trace_dir() {
        let dir = test_dir();
        write_sample(&dir, "run-lookup");
        let resolved = resolve_trace_path("run-lookup", Some(&dir)).unwrap();
        assert!(resolved.ends_with("run-lookup.jsonl"));
    }

    #[test]
    fn resolve_reports_unknown_identifier() {
        let dir = test_dir();
        let err = resolve_trace_path("run-nonexistent", Some(&dir))
            .unwrap_err()
            .to_string();
        assert!(err.contains("no trace matches"));
    }

    #[test]
    fn list_reports_missing_dir_cleanly() {
        let dir = std::env::temp_dir().join(format!(
            "corvid-cli-missing-{}",
            std::process::id()
        ));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        // run_list returns Ok(0) with a "no traces" message; we just
        // ensure it doesn't panic.
        run_list(Some(&dir)).unwrap();
    }

    #[test]
    fn list_collects_multiple_jsonl_files() {
        let dir = test_dir();
        write_sample(&dir, "run-alpha");
        write_sample(&dir, "run-beta");
        std::fs::write(dir.join("not-a-trace.txt"), "").unwrap();

        let files = collect_trace_files(&dir).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|p| p.extension().unwrap() == "jsonl"));
    }

    #[test]
    fn summarize_extracts_run_id_and_counts() {
        let dir = test_dir();
        let path = write_sample(&dir, "run-summary");
        let summary = summarize_trace(&path);
        assert_eq!(summary.event_count, 3);
        assert_eq!(summary.run_id.as_deref(), Some("run-summary"));
        assert_eq!(summary.schema, Some(SCHEMA_VERSION));
    }
}
