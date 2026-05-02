//! Live counterfactual mutation replay (Phase 21 slice
//! `21-inv-D-cli-wire`).
//!
//! `corvid replay --mutate <STEP> <JSON> --source <file> <trace>`
//! replays the trace with exactly one recorded response
//! overridden at position `<STEP>` and reports the downstream
//! behavior diff. Used to answer counterfactual questions: "if
//! the LLM had returned X instead of Y at step 5, would the
//! agent's final completion change?"
//!
//! `mutate_replay_live` is the entry point; `render_mutation_report`
//! and `render_mutation_divergence` format the resulting
//! `ReplayMutationReport` to stderr. `is_substitutable` and
//! `describe_substitutable` are the pre-flight validators that
//! reject `--mutate STEP` pointing at a non-call event before a
//! Runtime is even constructed.

use std::path::Path;

use anyhow::{Context, Result};

use corvid_driver::{run_replay_from_source, ReplayMode, ReplayOutcome};
use corvid_runtime::MutationDivergence;
use corvid_trace_schema::TraceEvent;

use super::{render_completion_divergence, truncate_json, EXIT_DIVERGED};

pub(super) fn mutate_replay_live(
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
    eprintln!("counterfactual replay — step {step_1based} ({kind} `{name}`) overridden");
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
    eprintln!("  mutated step: {} ({} `{}`)", step, kind, name);
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
        eprintln!("      divergences above (if any) were observed before the error surfaced.");
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
