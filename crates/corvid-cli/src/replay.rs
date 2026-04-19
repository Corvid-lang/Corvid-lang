//! `corvid replay <trace>` — re-execute a Corvid program from a
//! recorded JSONL trace.
//!
//! Three modes:
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
//! 3. **Counterfactual mutation replay** (`corvid replay --mutate
//!    <STEP> <JSON> <trace>`) — replays the trace with exactly
//!    one recorded response overridden at position `<STEP>` and
//!    reports the downstream behavior diff. `<STEP>` is the
//!    1-based index among substitutable events (ToolCall /
//!    LlmCall / ApprovalRequest). Runtime seam lands in Phase 21
//!    slice `21-inv-D-runtime` (Dev B).
//!
//! Until those runtime slices land, all three modes:
//!   1. Load the trace and validate the schema header.
//!   2. Print a summary (event count, run id, schema version).
//!   3. In `--model` mode, print the target model and what the
//!      differential report will cover.
//!   4. In `--mutate` mode, validate the step index and JSON
//!      replacement, then print the mutation plan.
//!   5. Return exit code 1 with a clean "not yet available"
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

/// Entry for `corvid replay <trace> [--model <id>] [--mutate STEP JSON]`.
///
/// `model` and `mutate` are mutually exclusive at the CLI layer
/// (clap enforces this); callers that construct args another way
/// must respect the same invariant. With neither set, runs a
/// plain replay. With `model`, runs a differential model replay.
/// With `mutate`, runs a counterfactual mutation replay.
pub fn run_replay(
    trace: &Path,
    model: Option<&str>,
    mutate: Option<&[String]>,
) -> Result<u8> {
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

    match (model, mutate) {
        (Some(_), Some(_)) => {
            // Defensive — clap enforces mutual exclusion at parse
            // time, but the library-level entry is stricter than
            // its CLI-level caller so a non-clap caller can't slip
            // past the invariant.
            anyhow::bail!(
                "`--model` and `--mutate` are mutually exclusive; pick one counterfactual axis"
            );
        }
        (Some(model_id), None) => differential_replay_stub(model_id, &events),
        (None, Some(args)) => mutate_replay_stub(args, &events),
        (None, None) => plain_replay_stub(),
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

/// Counterfactual-mutation stub. Validates the `(step, json)`
/// pair against the trace, prints the mutation plan, and returns
/// the "not yet available" exit code. Once `21-inv-D-runtime`
/// lands, this branch invokes the replay runtime with a single
/// response override at the requested step and renders the
/// downstream behavior diff.
fn mutate_replay_stub(args: &[String], events: &[TraceEvent]) -> Result<u8> {
    // clap enforces num_args = 2, but be explicit so library-level
    // callers get a clean error rather than an index panic.
    if args.len() != 2 {
        anyhow::bail!(
            "`--mutate` takes exactly two arguments (STEP and JSON); got {}",
            args.len()
        );
    }

    let step_1based: usize = args[0]
        .parse()
        .with_context(|| format!("`--mutate` STEP must be a positive integer; got `{}`", args[0]))?;
    if step_1based == 0 {
        anyhow::bail!("`--mutate` STEP is 1-based; 0 is not a valid step");
    }

    let replacement: serde_json::Value = serde_json::from_str(&args[1])
        .with_context(|| format!("`--mutate` JSON did not parse: `{}`", args[1]))?;

    let substitutable: Vec<(usize, &TraceEvent)> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| is_substitutable(e))
        .collect();

    if step_1based > substitutable.len() {
        anyhow::bail!(
            "`--mutate` STEP {} is out of range; trace has {} substitutable event(s)",
            step_1based,
            substitutable.len()
        );
    }

    let (_, event) = substitutable[step_1based - 1];
    let (kind, name) = describe_substitutable(event);

    eprintln!();
    eprintln!(
        "counterfactual replay — step {step_1based} ({kind} `{name}`) will be overridden"
    );
    eprintln!(
        "    recorded response replaced with: {}",
        serde_json::to_string(&replacement).unwrap_or_else(|_| "<unrenderable>".into())
    );
    eprintln!(
        "    the behavior diff will report every downstream step whose value changed"
    );
    eprintln!();
    eprintln!(
        "note: counterfactual replay is not yet available. The mutation seam \
         ships in Phase 21 slice 21-inv-D-runtime (Dev B); this CLI will wire \
         into it once landed. No LLM calls are made today — trace load + \
         schema validation + mutation validation succeeded above."
    );
    Ok(EXIT_NOT_IMPLEMENTED)
}

/// True when `event` is a call that can have its recorded response
/// substituted by the replay engine. The mutation seam operates
/// only on these; pointing `--mutate` at a `SchemaHeader`,
/// `RunStarted`, dispatch-metadata, or a result/response event is
/// rejected up front.
fn is_substitutable(event: &TraceEvent) -> bool {
    matches!(
        event,
        TraceEvent::ToolCall { .. }
            | TraceEvent::LlmCall { .. }
            | TraceEvent::ApprovalRequest { .. }
    )
}

/// Render a substitutable event as `(kind_label, name)` for the
/// mutation-plan preview. Non-substitutable variants return
/// `("other", "<unknown>")` so misuse fails loudly rather than
/// silently.
fn describe_substitutable(event: &TraceEvent) -> (&'static str, &str) {
    match event {
        TraceEvent::ToolCall { tool, .. } => ("tool_call", tool.as_str()),
        TraceEvent::LlmCall { prompt, .. } => ("llm_call", prompt.as_str()),
        TraceEvent::ApprovalRequest { label, .. } => ("approval_request", label.as_str()),
        _ => ("other", "<unknown>"),
    }
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
        let code = run_replay(&path, None, None).unwrap();
        assert_eq!(code, EXIT_NOT_IMPLEMENTED);
    }

    #[test]
    fn differential_replay_stub_returns_not_implemented_exit_code() {
        let dir = test_dir();
        let path = write_sample(&dir, "run-diff", true);
        let code = run_replay(&path, Some("claude-opus-5-0"), None).unwrap();
        assert_eq!(code, EXIT_NOT_IMPLEMENTED);
    }

    #[test]
    fn empty_trace_is_rejected_with_error() {
        let dir = test_dir();
        let path = dir.join("empty.jsonl");
        std::fs::write(&path, "").unwrap();
        let err = run_replay(&path, None, None).unwrap_err();
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
        let err = run_replay(&path, None, None).unwrap_err();
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
        let code = run_replay(&path, Some("claude-opus-5-0"), None).unwrap();
        assert_eq!(code, EXIT_NOT_IMPLEMENTED);
    }

    fn mutate_args(step: usize, json: &str) -> Vec<String> {
        vec![step.to_string(), json.into()]
    }

    #[test]
    fn mutate_stub_returns_not_implemented_exit_code() {
        let dir = test_dir();
        let path = write_sample(&dir, "run-mutate", true);
        let args = mutate_args(1, "\"cancel\"");
        let code = run_replay(&path, None, Some(args.as_slice())).unwrap();
        assert_eq!(code, EXIT_NOT_IMPLEMENTED);
    }

    #[test]
    fn mutate_rejects_out_of_range_step() {
        let dir = test_dir();
        // One substitutable event (the LlmCall); asking for step 5
        // is out of range.
        let path = write_sample(&dir, "run-mutate-oor", true);
        let args = mutate_args(5, "\"refund\"");
        let err = run_replay(&path, None, Some(args.as_slice())).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("out of range") && msg.contains("substitutable"),
            "got: {msg}"
        );
    }

    #[test]
    fn mutate_rejects_invalid_json_replacement() {
        let dir = test_dir();
        let path = write_sample(&dir, "run-mutate-badjson", true);
        let args = mutate_args(1, "{not valid");
        let err = run_replay(&path, None, Some(args.as_slice())).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("JSON did not parse"), "got: {msg}");
    }

    #[test]
    fn mutate_rejects_non_positive_step() {
        let dir = test_dir();
        let path = write_sample(&dir, "run-mutate-zero", true);
        let args = mutate_args(0, "\"refund\"");
        let err = run_replay(&path, None, Some(args.as_slice())).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("1-based"), "got: {msg}");
    }

    #[test]
    fn mutate_rejects_trace_with_no_substitutable_events() {
        // A trace consisting of only the header + RunStarted +
        // RunCompleted has zero substitutable events; any step
        // index is out of range.
        let dir = test_dir();
        let path = write_sample(&dir, "run-mutate-none", false);
        let args = mutate_args(1, "\"refund\"");
        let err = run_replay(&path, None, Some(args.as_slice())).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("out of range") && msg.contains("0 substitutable"),
            "got: {msg}"
        );
    }

    #[test]
    fn mutate_and_model_passed_together_is_rejected() {
        // Library-level defensive check: the CLI layer already
        // rejects via clap's `conflicts_with`, but callers that
        // bypass clap should still get a clean error rather than
        // silently preferring one over the other.
        let dir = test_dir();
        let path = write_sample(&dir, "run-mutate-conflict", true);
        let args = mutate_args(1, "\"refund\"");
        let err = run_replay(
            &path,
            Some("claude-opus-5-0"),
            Some(args.as_slice()),
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("mutually exclusive"), "got: {msg}");
    }
}
