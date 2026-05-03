use super::load::LoadedTrace;
use super::TestFromTracesArgs;
use corvid_runtime::{TestFromTracesReport, TraceOutcome, Verdict};
use std::collections::BTreeSet;
use std::path::Path;

pub(super) fn render_report(report: &TestFromTracesReport) {
    println!();
    println!("Regression harness report");
    println!("=========================");
    for outcome in &report.per_trace {
        render_outcome(outcome);
    }
    println!();
    let s = &report.summary;
    println!(
        "Summary: {} total — {} passed, {} diverged, {} flaky, {} promoted, {} errored",
        s.total, s.passed, s.diverged, s.flaky, s.promoted, s.errored
    );
    if report.aborted {
        println!(
            "note: harness aborted — user quit during promotion prompts; some traces may not \
             have been evaluated."
        );
    }
}

fn render_outcome(outcome: &TraceOutcome) {
    let glyph = match outcome.verdict {
        Verdict::Passed => "  ok  ",
        Verdict::Diverged => "DIVERG",
        Verdict::Flaky => "FLAKY ",
        Verdict::Promoted => "PROMOT",
        Verdict::Error => "ERROR ",
    };
    println!("[{glyph}] {}", outcome.path.display());
    if !outcome.divergences.is_empty() {
        println!("        divergences: {}", outcome.divergences.len());
    }
    if let Some(flake) = &outcome.flake_rank {
        println!(
            "        flake-rank: {}/{} runs diverged",
            flake.divergent_runs, flake.total_runs
        );
    }
    if let Some(model_swap) = &outcome.model_swap {
        let llm_count = model_swap.report.llm_divergences.len();
        println!(
            "        model-swap (vs. `{}`): {} LLM divergence(s)",
            model_swap.model, llm_count
        );
    }
    if let Some(err) = &outcome.error {
        println!("        error: {err}");
    }
}

pub(super) fn print_preview(
    dir: &Path,
    initial_count: usize,
    applied_filters: &[(&'static str, String, usize)],
    filtered: &[&LoadedTrace],
    args: &TestFromTracesArgs<'_>,
) {
    println!("corvid test --from-traces {}", dir.display());
    println!();
    println!("Scanning traces in `{}`...", dir.display());
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
        Some(id) => format!("differential vs. `{id}` (divergences will be reported per trace)"),
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

pub(super) fn aggregate_coverage(
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
        format!("{{{}}}", set.iter().cloned().collect::<Vec<_>>().join(", "))
    }
}

#[allow(dead_code)]
fn print_not_implemented_note() {
    eprintln!(
        "note: `corvid test --from-traces` is not yet available. The regression \
         harness ships in Phase 21 slice 21-inv-G-harness (Dev B); this CLI will \
         wire into it once landed. Trace load + schema validation + filtering + \
         coverage preview succeeded above."
    );
}
