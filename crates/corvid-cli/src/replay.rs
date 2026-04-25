//! `corvid replay <trace>` — re-execute a Corvid program from a
//! recorded JSONL trace.
//!
//! Three modes:
//!
//! 1. **Plain replay** (`corvid replay <trace>`) — substitutes
//!    recorded responses for every live call and reproduces the
//!    original run byte-for-byte. Runtime ships in
//!    `21-C-replay-interp`; CLI wire-up follows the differential
//!    wire pattern below.
//!
//! 2. **Differential model replay** (`corvid replay --model <id>
//!    --source <file> <trace>`, slice `21-inv-B-cli-wire`) —
//!    swaps the replay adapter's response source from "recorded
//!    value" to "live call against a different model." Produces
//!    a divergence report listing the steps whose output differs
//!    from the recorded one. Runtime seam from
//!    `21-inv-B-adapter`.
//!
//! 3. **Counterfactual mutation replay** (`corvid replay --mutate
//!    <STEP> <JSON> --source <file> <trace>`) — replays the trace
//!    with exactly one recorded response overridden at position
//!    `<STEP>` and reports the downstream behavior diff.
//!    Runtime seam from `21-inv-D-runtime`.
//!
//! Modes 1 and 3 remain in their pre-wire stub state today; their
//! wires are next in the queue (`21-F-cli-wire` +
//! `21-inv-D-cli-wire`).
//!
//! `--source <FILE>` points at the Corvid source the trace was
//! recorded against. It's required for actually-execute modes;
//! the runtime needs the IR to replay. Once
//! `SchemaHeader.source_path` is populated at record time
//! (`21-A-schema-ext-source` landed the field; Dev B's recorder
//! follow-up will populate it), the flag becomes optional and
//! auto-resolves from the trace.

use anyhow::{Context, Result};
use corvid_driver::{run_replay_from_source, ReplayMode, ReplayOutcome};
use corvid_runtime::{
    LlmDivergence, MutationDivergence, RunCompletionDivergence, SubstitutionDivergence,
};
use corvid_trace_schema::{
    read_events_from_path, schema_version_of, validate_supported_schema, TraceEvent,
};
use std::path::Path;

/// Exit code returned when a replay runs cleanly but surfaces
/// divergences (differential mode: any `LlmDivergence`; mutation
/// mode: any `MutationDivergence`). Distinguishes
/// "ran-and-diverged" from "could not run" (typed anyhow errors).
pub const EXIT_DIVERGED: u8 = 1;

/// Exit code returned when a replay runtime path isn't yet wired
/// to the CLI. Kept stable so existing tests (and CI scripts that
/// distinguish "not wired" from "diverged") continue to work.
pub const EXIT_NOT_IMPLEMENTED: u8 = 1;

/// Entry for `corvid replay <trace> [--source <file>] [--model <id>] [--mutate STEP JSON]`.
///
/// `model` and `mutate` are mutually exclusive at the CLI layer
/// (clap enforces this); callers that construct args another way
/// must respect the same invariant. With neither set, runs a
/// plain replay (stub until its wire slice). With `model`, runs
/// a differential model replay (live wire as of 21-inv-B-cli-wire).
/// With `mutate`, runs a counterfactual mutation replay (stub).
pub fn run_replay(
    trace: &Path,
    source: Option<&Path>,
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
        (Some(model_id), None) => differential_replay_live(trace, source, model_id),
        (None, Some(args)) => mutate_replay_live(trace, source, args, &events),
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

/// Live differential replay path (Phase 21 slice 21-inv-B-cli-wire).
///
/// Composes the shipped `21-inv-B-adapter` runtime seam with the
/// CLI layer: loads the recorded trace, compiles the user-supplied
/// `--source`, re-executes the recorded agent with its recorded
/// args through the `differential_replay_from(trace, model)`
/// adapter, and renders the resulting `ReplayDifferentialReport`
/// to stderr. Exit code reflects the divergence outcome:
/// `0` when the live model agreed with the recording on every
/// LLM call (and substitutions + completion matched), `1` when at
/// least one divergence surfaced.
fn differential_replay_live(
    trace: &Path,
    source: Option<&Path>,
    model_id: &str,
) -> Result<u8> {
    let source_path = source.ok_or_else(|| {
        anyhow::anyhow!(
            "`--model` replay requires `--source <FILE>` pointing at the Corvid source the \
             trace was recorded against. Once `SchemaHeader.source_path` is populated at \
             record time, this flag becomes optional."
        )
    })?;

    eprintln!();
    eprintln!("differential replay mode — target model: `{model_id}`");
    eprintln!("    source:   {}", source_path.display());
    eprintln!("    compiling source and dispatching through replay adapter...");
    eprintln!();

    let outcome = run_replay_from_source(
        trace,
        source_path,
        ReplayMode::Differential(model_id.to_string()),
    )?;

    render_differential_report(&outcome, model_id)
}

fn render_differential_report(outcome: &ReplayOutcome, model_id: &str) -> Result<u8> {
    let report = outcome.differential_report.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "replay completed but the runtime produced no differential report — this is a \
             runtime bug; the differential adapter must emit a report for every run"
        )
    })?;

    let llm_count = report.llm_divergences.len();
    let sub_count = report.substitution_divergences.len();
    let completion_diverged = report.run_completion_divergence.is_some();

    eprintln!("differential replay report — agent: `{}`", outcome.agent_name);
    eprintln!(
        "  LLM divergences (live `{model_id}` vs. recorded): {llm_count}"
    );
    eprintln!("  substitution divergences (shape mismatches): {sub_count}");
    eprintln!(
        "  completion divergence (final result / error): {}",
        if completion_diverged { "yes" } else { "no" }
    );
    eprintln!();

    if llm_count > 0 {
        eprintln!("LLM divergences:");
        for div in &report.llm_divergences {
            render_llm_divergence(div);
        }
        eprintln!();
    }
    if sub_count > 0 {
        eprintln!("Substitution divergences:");
        for div in &report.substitution_divergences {
            render_substitution_divergence(div);
        }
        eprintln!();
    }
    if let Some(completion) = &report.run_completion_divergence {
        eprintln!("Completion divergence:");
        render_completion_divergence(completion);
        eprintln!();
    }

    if let Some(err) = &outcome.result_error {
        eprintln!("note: the replay run itself errored: {err}");
        eprintln!(
            "      divergences above (if any) were observed before the error surfaced."
        );
    }

    if llm_count == 0 && sub_count == 0 && !completion_diverged {
        eprintln!(
            "no divergences — `{model_id}` reproduced every recorded LLM result, every \
             substituted call, and the final completion."
        );
        Ok(0)
    } else {
        Ok(EXIT_DIVERGED)
    }
}

fn render_llm_divergence(div: &LlmDivergence) {
    eprintln!(
        "  step {} — prompt `{}`: recorded `{}` vs. live `{}`",
        div.step,
        div.prompt,
        truncate_json(&div.recorded, 60),
        truncate_json(&div.live, 60),
    );
}

fn render_substitution_divergence(div: &SubstitutionDivergence) {
    eprintln!(
        "  step {} — got {} (`{}`) where trace expected a different shape",
        div.step, div.got_kind, div.got_description,
    );
}

fn render_completion_divergence(div: &RunCompletionDivergence) {
    eprintln!(
        "  step {} — recorded(ok={}, result={}, error={}) vs. live(ok={}, result={}, error={})",
        div.step,
        div.recorded_ok,
        render_optional_json(div.recorded_result.as_ref()),
        div.recorded_error.as_deref().unwrap_or("<none>"),
        div.live_ok,
        render_optional_json(div.live_result.as_ref()),
        div.live_error.as_deref().unwrap_or("<none>"),
    );
}

fn truncate_json(value: &serde_json::Value, max_chars: usize) -> String {
    let s = serde_json::to_string(value).unwrap_or_else(|_| "<unrenderable>".into());
    if s.chars().count() <= max_chars {
        s
    } else {
        let mut truncated: String = s.chars().take(max_chars).collect();
        truncated.push_str("...");
        truncated
    }
}

fn render_optional_json(value: Option<&serde_json::Value>) -> String {
    value
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "<unrenderable>".into()))
        .unwrap_or_else(|| "<none>".into())
}

/// Live counterfactual mutation replay (Phase 21 slice
/// 21-inv-D-cli-wire).
///
/// Parses + validates the `(STEP, JSON)` pair against the trace,
/// resolves `--source`, then dispatches through the driver's
/// replay orchestrator with `ReplayMode::Mutation`. Renders the
/// resulting [`corvid_runtime::ReplayMutationReport`] to stderr.
///
/// Exit code reflects the downstream-drift outcome:
/// `0` when the trace's post-STEP events all matched what the
/// mutated run produced (meaning the mutation had no observable
/// downstream effect — rarely the expected outcome but possible
/// when the mutated value flows nowhere), or `EXIT_DIVERGED`
/// (`1`) when at least one downstream step diverged or the
/// agent's final completion changed.
fn mutate_replay_live(
    trace: &Path,
    source: Option<&Path>,
    args: &[String],
    events: &[TraceEvent],
) -> Result<u8> {
    // clap enforces num_args = 2, but be explicit so library-level
    // callers get a clean error rather than an index panic.
    if args.len() != 2 {
        anyhow::bail!(
            "`--mutate` takes exactly two arguments (STEP and JSON); got {}",
            args.len()
        );
    }

    let step_1based: usize = args[0].parse().with_context(|| {
        format!(
            "`--mutate` STEP must be a positive integer; got `{}`",
            args[0]
        )
    })?;
    if step_1based == 0 {
        anyhow::bail!("`--mutate` STEP is 1-based; 0 is not a valid step");
    }

    let replacement: serde_json::Value = serde_json::from_str(&args[1])
        .with_context(|| format!("`--mutate` JSON did not parse: `{}`", args[1]))?;

    // Pre-flight validation against the local event stream:
    // reject out-of-range STEP before building a Runtime, and
    // name the specific recorded event being overridden so the
    // user sees what their replacement will stand in for.
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

    let source_path = source.ok_or_else(|| {
        anyhow::anyhow!(
            "`--mutate` replay requires `--source <FILE>` pointing at the Corvid source the \
             trace was recorded against. Once `SchemaHeader.source_path` is populated at \
             record time, this flag becomes optional."
        )
    })?;

    eprintln!();
    eprintln!(
        "counterfactual replay — step {step_1based} ({kind} `{name}`) overridden"
    );
    eprintln!(
        "    recorded response replaced with: {}",
        serde_json::to_string(&replacement).unwrap_or_else(|_| "<unrenderable>".into())
    );
    eprintln!("    source:   {}", source_path.display());
    eprintln!("    compiling source and dispatching through mutation adapter...");
    eprintln!();

    let outcome = run_replay_from_source(
        trace,
        source_path,
        ReplayMode::Mutation {
            step_1based,
            replacement,
        },
    )?;

    render_mutation_report(&outcome, step_1based, kind, name)
}

fn render_mutation_report(
    outcome: &ReplayOutcome,
    step: usize,
    kind: &str,
    name: &str,
) -> Result<u8> {
    let report = outcome.mutation_report.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "replay completed but the runtime produced no mutation report — this is a \
             runtime bug; the mutation adapter must emit a report for every run"
        )
    })?;

    let divergence_count = report.divergences.len();
    let completion_diverged = report.run_completion_divergence.is_some();

    eprintln!("mutation replay report — agent: `{}`", outcome.agent_name);
    eprintln!(
        "  mutated step: {} ({} `{}`)",
        step, kind, name
    );
    eprintln!("  downstream divergences: {}", divergence_count);
    eprintln!(
        "  completion divergence (final result / error): {}",
        if completion_diverged { "yes" } else { "no" }
    );
    eprintln!();

    if divergence_count > 0 {
        eprintln!("Downstream divergences:");
        for div in &report.divergences {
            render_mutation_divergence(div);
        }
        eprintln!();
    }
    if let Some(completion) = &report.run_completion_divergence {
        eprintln!("Completion divergence:");
        render_completion_divergence(completion);
        eprintln!();
    }

    if let Some(err) = &outcome.result_error {
        eprintln!("note: the replay run itself errored: {err}");
        eprintln!(
            "      divergences above (if any) were observed before the error surfaced."
        );
    }

    if divergence_count == 0 && !completion_diverged {
        eprintln!(
            "no divergences — the mutation at step {step} had no observable downstream \
             effect. The replaced value either flows nowhere that matters, or the code \
             treats it the same as the original."
        );
        Ok(0)
    } else {
        Ok(EXIT_DIVERGED)
    }
}

fn render_mutation_divergence(div: &MutationDivergence) {
    eprintln!(
        "  step {} — {} expected `{}`, got `{}`",
        div.step,
        div.kind,
        truncate_json(&div.recorded, 60),
        truncate_json(&div.got, 60),
    );
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
        | TraceEvent::PromptCache { run_id, .. }
        | TraceEvent::ApprovalRequest { run_id, .. }
        | TraceEvent::ApprovalDecision { run_id, .. }
        | TraceEvent::ApprovalResponse { run_id, .. }
        | TraceEvent::HostEvent { run_id, .. }
        | TraceEvent::SeedRead { run_id, .. }
        | TraceEvent::ClockRead { run_id, .. }
        | TraceEvent::ModelSelected { run_id, .. }
        | TraceEvent::ProgressiveEscalation { run_id, .. }
        | TraceEvent::ProgressiveExhausted { run_id, .. }
        | TraceEvent::StreamUpgrade { run_id, .. }
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
                model_version: None,
                rendered: None,
                args: vec![],
            });
            events.push(TraceEvent::LlmResult {
                ts_ms: 4,
                run_id: run_id.into(),
                prompt: "classify".into(),
                model: Some("claude-opus-4-6".into()),
                model_version: None,
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
        let code = run_replay(&path, None, None, None).unwrap();
        assert_eq!(code, EXIT_NOT_IMPLEMENTED);
    }

    #[test]
    fn differential_replay_without_source_reports_clean_error() {
        // 21-inv-B-cli-wire: the differential path now actually
        // dispatches through the runtime, which requires the
        // Corvid source to compile. Without `--source`, fail
        // fast with a message naming the flag rather than
        // attempting replay.
        let dir = test_dir();
        let path = write_sample(&dir, "run-diff", true);
        let err = run_replay(&path, None, Some("claude-opus-5-0"), None).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("--source"),
            "expected error to mention `--source`, got: {msg}"
        );
    }

    #[test]
    fn empty_trace_is_rejected_with_error() {
        let dir = test_dir();
        let path = dir.join("empty.jsonl");
        std::fs::write(&path, "").unwrap();
        let err = run_replay(&path, None, None, None).unwrap_err();
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
        let err = run_replay(&path, None, None, None).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("failed to load trace"),
            "expected load-failure context, got: {msg}"
        );
    }

    #[test]
    fn differential_replay_without_source_on_no_llm_trace_still_requires_source() {
        // Regression: even when there's nothing to differentially
        // replay (no LlmCall events in the trace), the wire still
        // requires `--source` because the runtime still needs the
        // IR to replay the RunStarted / RunCompleted pair.
        let dir = test_dir();
        let path = write_sample(&dir, "run-no-llm", false);
        let err = run_replay(&path, None, Some("claude-opus-5-0"), None).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("--source"), "got: {msg}");
    }

    fn mutate_args(step: usize, json: &str) -> Vec<String> {
        vec![step.to_string(), json.into()]
    }

    #[test]
    fn mutate_replay_without_source_reports_clean_error() {
        // 21-inv-D-cli-wire: the mutate path now actually
        // dispatches through the runtime, which requires the
        // Corvid source to compile. Without `--source`, fail
        // fast with a message naming the flag rather than
        // attempting replay.
        let dir = test_dir();
        let path = write_sample(&dir, "run-mutate", true);
        let args = mutate_args(1, "\"cancel\"");
        let err =
            run_replay(&path, None, None, Some(args.as_slice())).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("--source"), "got: {msg}");
    }

    #[test]
    fn mutate_rejects_out_of_range_step() {
        let dir = test_dir();
        // One substitutable event (the LlmCall); asking for step 5
        // is out of range.
        let path = write_sample(&dir, "run-mutate-oor", true);
        let args = mutate_args(5, "\"refund\"");
        let err = run_replay(&path, None, None, Some(args.as_slice())).unwrap_err();
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
        let err = run_replay(&path, None, None, Some(args.as_slice())).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("JSON did not parse"), "got: {msg}");
    }

    #[test]
    fn mutate_rejects_non_positive_step() {
        let dir = test_dir();
        let path = write_sample(&dir, "run-mutate-zero", true);
        let args = mutate_args(0, "\"refund\"");
        let err = run_replay(&path, None, None, Some(args.as_slice())).unwrap_err();
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
        let err = run_replay(&path, None, None, Some(args.as_slice())).unwrap_err();
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
            None,
            Some("claude-opus-5-0"),
            Some(args.as_slice()),
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("mutually exclusive"), "got: {msg}");
    }
}
