//! `corvid replay <trace>` — re-execute a Corvid program from a
//! recorded JSONL trace.
//!
//! Two modes:
//!
//! 1. **Plain replay** (`corvid replay <trace>`) — substitutes
//!    recorded responses for every live call and reproduces the
//!    original run byte-for-byte. Runtime lands in Phase 21
//!    slices `21-C-replay-interp` (Dev B, interpreter tier) and
//!    `21-C-replay-native` (Dev B, native tier).
//!
//! 2. **Differential model replay** (`corvid replay --model <id>
//!    <trace>`) — swaps the replay adapter's response source
//!    from "recorded value" to "live call against a different
//!    model." Produces a divergence report listing the steps
//!    whose output differs from the recorded one. Runtime seam
//!    lands in Phase 21 slice `21-inv-B-adapter` (Dev B).
//!
//! Until those runtime slices land, both modes:
//!   1. Load the trace and validate the schema header.
//!   2. Print a summary (event count, run id, schema version).
//!   3. In `--model` mode, print the target model and what the
//!      differential report will cover.
//!   4. Return exit code 1 with a clean "not yet available"
//!      message pointing at the pending runtime slice.
//!
//! Loading + validating is useful as a pre-flight check today —
//! catches corrupted traces and schema-version mismatches before
//! the replay engine ever sees the file.

use anyhow::{Context, Result};
use corvid_trace_schema::{
    read_events_from_path, schema_version_of, validate_supported_schema, TraceEvent,
};
use std::path::Path;

/// Exit code returned when the runtime replay path isn't available
/// yet. Callers (CI, tests) can distinguish "tool not implemented"
/// from "trace malformed" by checking this specific code.
pub const EXIT_NOT_IMPLEMENTED: u8 = 1;

/// Entry for `corvid replay <trace> [--model <id>]`.
///
/// With `model = None`, runs a plain replay (verbatim
/// reproduction). With `model = Some(id)`, runs a differential
/// replay against the named model and reports divergences.
pub fn run_replay(trace: &Path, model: Option<&str>) -> Result<u8> {
    let events = read_events_from_path(trace).with_context(|| {
        format!("failed to load trace at `{}`", trace.display())
    })?;

    if events.is_empty() {
        anyhow::bail!("trace `{}` is empty", trace.display());
    }

    validate_supported_schema(&events).with_context(|| {
        format!("trace `{}` uses an unsupported schema", trace.display())
    })?;

    print_summary(trace, &events);

    match model {
        None => plain_replay_stub(),
        Some(model_id) => differential_replay_stub(model_id, &events),
    }
}

/// Plain-replay stub. Today returns a "not yet available"
/// message; once `21-C-replay-interp` / `21-C-replay-native`
/// land, this branch invokes the replay runtime.
fn plain_replay_stub() -> Result<u8> {
    eprintln!();
    eprintln!(
        "note: `corvid replay` is not yet available. Replay-runtime support \
         ships in Phase 21 slice 21-C-replay-interp (interpreter tier) and \
         21-C-replay-native (native tier). Trace load + schema validation \
         succeeded above; once the runtime slices land, this command will \
         re-execute the program with recorded responses substituted for live \
         calls."
    );
    Ok(EXIT_NOT_IMPLEMENTED)
}

/// Differential-replay stub. Today counts how many LLM calls are
/// in the trace (so users see the work the future divergence
/// report will cover) and returns the "not yet available" exit
/// code. Once `21-inv-B-adapter` lands, this branch invokes the
/// replay runtime in model-swap mode and renders the divergence
/// table.
fn differential_replay_stub(model_id: &str, events: &[TraceEvent]) -> Result<u8> {
    let llm_call_count = events
        .iter()
        .filter(|e| matches!(e, TraceEvent::LlmCall { .. }))
        .count();
    let llm_result_count = events
        .iter()
        .filter(|e| matches!(e, TraceEvent::LlmResult { .. }))
        .count();

    eprintln!();
    eprintln!("differential replay mode — target model: `{model_id}`");
    eprintln!(
        "    trace contains {llm_call_count} LLM call(s) and {llm_result_count} \
         recorded LLM result(s); the differential report will compare each \
         recorded result against `{model_id}`'s output for the same prompt."
    );
    eprintln!();
    eprintln!(
        "note: differential replay is not yet available. The model-swap seam \
         ships in Phase 21 slice 21-inv-B-adapter (Dev B); this CLI will wire \
         into it once landed. No LLM calls are made today — trace load + \
         schema validation succeeded above."
    );
    Ok(EXIT_NOT_IMPLEMENTED)
}

fn print_summary(trace: &Path, events: &[TraceEvent]) {
    let schema = schema_version_of(events)
        .map(|v| format!("v{v}"))
        .unwrap_or_else(|| "legacy (no header)".into());
    let run_id = run_id_of(events).unwrap_or("<unknown>");
    println!(
        "trace loaded: {} events, run_id={}, schema={}, path={}",
        events.len(),
        run_id,
        schema,
        trace.display()
    );
}

/// Pull the run_id from whichever event carries one first. Works
/// across legacy headerless traces and 21-A-schema-era traces with
/// a `SchemaHeader` up front.
fn run_id_of(events: &[TraceEvent]) -> Option<&str> {
    events.iter().find_map(|event| match event {
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_trace_schema::{
        write_events_to_path, SCHEMA_VERSION, WRITER_INTERPRETER,
    };
    use std::path::PathBuf;

    fn test_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "corvid-cli-replay-test-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_sample(dir: &std::path::Path, run_id: &str, with_llm: bool) -> PathBuf {
        let path = dir.join(format!("{run_id}.jsonl"));
        let mut events = vec![
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
        ];
        if with_llm {
            events.push(TraceEvent::LlmCall {
                ts_ms: 3,
                run_id: run_id.into(),
                prompt: "classify".into(),
                model: Some("claude-opus-4-6".into()),
                rendered: None,
                args: vec![],
            });
            events.push(TraceEvent::LlmResult {
                ts_ms: 4,
                run_id: run_id.into(),
                prompt: "classify".into(),
                model: Some("claude-opus-4-6".into()),
                result: serde_json::json!("refund"),
            });
        }
        events.push(TraceEvent::RunCompleted {
            ts_ms: 5,
            run_id: run_id.into(),
            ok: true,
            result: None,
            error: None,
        });
        write_events_to_path(&path, &events).unwrap();
        path
    }

    #[test]
    fn plain_replay_stub_returns_not_implemented_exit_code() {
        let dir = test_dir();
        let path = write_sample(&dir, "run-plain", false);
        let code = run_replay(&path, None).unwrap();
        assert_eq!(code, EXIT_NOT_IMPLEMENTED);
    }

    #[test]
    fn differential_replay_stub_returns_not_implemented_exit_code() {
        let dir = test_dir();
        let path = write_sample(&dir, "run-diff", true);
        let code = run_replay(&path, Some("claude-opus-5-0")).unwrap();
        assert_eq!(code, EXIT_NOT_IMPLEMENTED);
    }

    #[test]
    fn empty_trace_is_rejected_with_error() {
        let dir = test_dir();
        let path = dir.join("empty.jsonl");
        std::fs::write(&path, "").unwrap();
        let err = run_replay(&path, None).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn missing_trace_file_reports_clean_io_error() {
        let path = std::env::temp_dir().join(format!(
            "corvid-cli-replay-nonexistent-{}.jsonl",
            std::process::id()
        ));
        // Make sure it really is absent.
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
        let err = run_replay(&path, None).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("failed to load trace"),
            "expected load-failure context, got: {msg}"
        );
    }

    #[test]
    fn differential_replay_accepts_model_without_llm_events() {
        // A trace with no LLM calls should still accept a
        // differential-replay invocation — the stub reports
        // zero LLM calls in the divergence preview.
        let dir = test_dir();
        let path = write_sample(&dir, "run-no-llm", false);
        let code = run_replay(&path, Some("claude-opus-5-0")).unwrap();
        assert_eq!(code, EXIT_NOT_IMPLEMENTED);
    }
}
