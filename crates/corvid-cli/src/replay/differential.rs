//! Live differential model replay (Phase 21 slice
//! `21-inv-B-cli-wire`).
//!
//! `corvid replay --model <id> --source <file> <trace>` swaps
//! the replay adapter's response source from "recorded value"
//! to "live call against a different model" and produces a
//! divergence report listing the steps whose output differs
//! from the recorded one.
//!
//! `differential_replay_live` is the entry point; the
//! `render_*` helpers format the resulting
//! `ReplayDifferentialReport` to stderr (LLM divergences,
//! substitution divergences, run-completion divergence). The
//! exit code is `0` when the live model agreed with the
//! recording on every LLM call (and substitutions + completion
//! matched), `EXIT_DIVERGED` (`1`) when at least one divergence
//! surfaced.

use std::path::Path;

use anyhow::{Context, Result};

use corvid_driver::{run_replay_from_source, ReplayMode, ReplayOutcome};
use corvid_runtime::{LlmDivergence, SubstitutionDivergence};

use super::{render_completion_divergence, truncate_json, EXIT_DIVERGED};

pub(super) fn differential_replay_live(
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

    eprintln!(
        "differential replay report — agent: `{}`",
        outcome.agent_name
    );
    eprintln!("  LLM divergences (live `{model_id}` vs. recorded): {llm_count}");
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
        eprintln!("      divergences above (if any) were observed before the error surfaced.");
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

