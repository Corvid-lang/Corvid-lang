//! `corvid eval` source-level evals and retrospective model migration tooling.
//!
//! Source eval execution is owned by `corvid-driver`; this module keeps CLI
//! routing separate from the reusable runner. `--swap-model` remains the
//! Phase 20h retrospective migration mode.

use crate::{replay, test_from_traces};
use anyhow::{Context, Result};
use corvid_driver::{
    default_eval_options, load_dotenv_walking, render_eval_report, run_evals_at_path_with_options,
};
use corvid_runtime::{
    promote_lineage_events_to_eval, LineageEvent, LineageRedactionPolicy,
    LINEAGE_EVAL_FIXTURE_SCHEMA,
};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn run_eval(
    inputs: &[PathBuf],
    source: Option<&Path>,
    swap_model: Option<&str>,
    max_spend: Option<f64>,
    golden_traces: Option<&Path>,
    promote_out: Option<&Path>,
) -> Result<u8> {
    if golden_traces.is_some() && swap_model.is_some() {
        anyhow::bail!("`corvid eval --golden-traces` and `--swap-model` are separate modes");
    }
    if let Some(trace_dir) = golden_traces {
        return run_golden_trace_evals(inputs, source, trace_dir);
    }
    let Some(model) = swap_model else {
        return run_source_evals(inputs, max_spend, promote_out);
    };

    if inputs.is_empty() {
        anyhow::bail!("`corvid eval --swap-model` requires at least one trace file or directory");
    }

    eprintln!("eval model-swap mode - target model: `{model}`");
    let mut exit_code = 0_u8;
    for input in inputs {
        let code = if input.is_dir() {
            eprintln!(
                "running trace-suite migration analysis: {}",
                input.display()
            );
            test_from_traces::run_test_from_traces(test_from_traces::TestFromTracesArgs {
                trace_dir: input,
                source,
                replay_model: Some(model),
                only_dangerous: false,
                only_prompt: None,
                only_tool: None,
                since: None,
                promote: false,
                flake_detect: None,
            })
            .with_context(|| format!("failed to evaluate trace directory `{}`", input.display()))?
        } else {
            eprintln!("running trace migration analysis: {}", input.display());
            replay::run_replay(input, source, Some(model), None)
                .with_context(|| format!("failed to evaluate trace `{}`", input.display()))?
        };
        exit_code = exit_code.max(code);
    }

    Ok(exit_code)
}

fn run_golden_trace_evals(
    inputs: &[PathBuf],
    source: Option<&Path>,
    trace_dir: &Path,
) -> Result<u8> {
    let mut sources = inputs.to_vec();
    if sources.is_empty() {
        if let Some(source) = source {
            sources.push(source.to_path_buf());
        }
    }
    if sources.is_empty() {
        eprintln!("usage: `corvid eval --golden-traces <DIR> <source.cor>`");
        return Ok(1);
    }

    let mut exit_code = 0_u8;
    for source in &sources {
        eprintln!(
            "golden-trace eval: source `{}` against `{}`",
            source.display(),
            trace_dir.display()
        );
        let code = test_from_traces::run_test_from_traces(test_from_traces::TestFromTracesArgs {
            trace_dir,
            source: Some(source.as_path()),
            replay_model: None,
            only_dangerous: false,
            only_prompt: None,
            only_tool: None,
            since: None,
            promote: false,
            flake_detect: None,
        })
        .with_context(|| {
            format!(
                "failed golden-trace eval for `{}` against `{}`",
                source.display(),
                trace_dir.display()
            )
        })?;
        exit_code = exit_code.max(code);
    }
    Ok(exit_code)
}

fn run_source_evals(
    inputs: &[PathBuf],
    max_spend: Option<f64>,
    promote_out: Option<&Path>,
) -> Result<u8> {
    if inputs
        .first()
        .and_then(|input| input.to_str())
        .is_some_and(|input| input == "compare")
    {
        if promote_out.is_some() {
            anyhow::bail!("`corvid eval compare` does not accept `--promote-out`");
        }
        return run_compare(&inputs[1..]);
    }
    if inputs
        .first()
        .and_then(|input| input.to_str())
        .is_some_and(|input| input == "promote")
    {
        return run_promote_lineage(&inputs[1..], promote_out);
    }
    if promote_out.is_some() {
        anyhow::bail!("`--promote-out` is only valid with `corvid eval promote <trace>`");
    }

    if inputs.is_empty() {
        eprintln!("usage: `corvid eval <file.cor> [more.cor ...]`");
        eprintln!("       `corvid eval compare <base>..<head>`");
        eprintln!("       `corvid eval promote <trace.lineage.jsonl> [--promote-out DIR]`");
        eprintln!(
            "For model migration analysis, use `corvid eval --swap-model <MODEL> --source <FILE> <TRACE_OR_DIR>...`."
        );
        return Ok(1);
    }

    if let Some(max_spend) = configured_max_spend(max_spend)? {
        if !max_spend.is_finite() || max_spend < 0.0 {
            anyhow::bail!("eval budget must be a finite non-negative USD amount");
        }
        let planned = planned_eval_spend(inputs)?;
        if planned > max_spend {
            eprintln!(
                "eval budget exceeded before running: planned ${planned:.6} > max ${max_spend:.6}"
            );
            return Ok(1);
        }
        eprintln!("eval budget: planned ${planned:.6} <= max ${max_spend:.6}");
    }

    let mut exit_code = 0_u8;
    for input in inputs {
        let dotenv_start = input.parent().unwrap_or_else(|| Path::new("."));
        load_dotenv_walking(dotenv_start);
        let runtime = corvid_driver::Runtime::builder().build();
        let source = std::fs::read_to_string(input).ok();
        let tokio = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("failed to initialize async eval runtime")?;
        let report = tokio
            .block_on(run_evals_at_path_with_options(
                input,
                &runtime,
                default_eval_options(input),
            ))
            .map_err(anyhow::Error::new)?;
        print!("{}", render_eval_report(&report, source.as_deref()));
        exit_code = exit_code.max(report.exit_code());
    }
    Ok(exit_code)
}

fn run_promote_lineage(inputs: &[PathBuf], out_dir: Option<&Path>) -> Result<u8> {
    if inputs.is_empty() {
        eprintln!(
            "usage: `corvid eval promote <trace.lineage.jsonl> [more...] [--promote-out DIR]`"
        );
        return Ok(1);
    }
    let out_dir = out_dir.unwrap_or_else(|| Path::new("target/eval/lineage"));
    fs::create_dir_all(out_dir)
        .with_context(|| format!("creating eval fixture directory `{}`", out_dir.display()))?;
    let policy = LineageRedactionPolicy::production_default();
    for input in inputs {
        let events = read_lineage_events(input)
            .with_context(|| format!("reading lineage trace `{}`", input.display()))?;
        let fixture = promote_lineage_events_to_eval(&events, &policy)
            .with_context(|| format!("promoting lineage trace `{}`", input.display()))?;
        let file_name = format!(
            "{}.lineage-eval.json",
            sanitize_file_stem(&fixture.trace_id)
        );
        let out_path = out_dir.join(file_name);
        let json = serde_json::to_string_pretty(&fixture)
            .context("serializing promoted lineage eval fixture")?;
        fs::write(&out_path, format!("{json}\n"))
            .with_context(|| format!("writing eval fixture `{}`", out_path.display()))?;
        println!(
            "promoted: {} -> {} ({}, events={}, fixture_hash={})",
            input.display(),
            out_path.display(),
            LINEAGE_EVAL_FIXTURE_SCHEMA,
            fixture.events.len(),
            fixture.fixture_hash
        );
    }
    Ok(0)
}

fn read_lineage_events(path: &Path) -> Result<Vec<LineageEvent>> {
    let text = fs::read_to_string(path)?;
    let mut events = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let event: LineageEvent = serde_json::from_str(trimmed)
            .with_context(|| format!("line {} is not a lineage event", index + 1))?;
        events.push(event);
    }
    Ok(events)
}

fn sanitize_file_stem(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "trace".to_string()
    } else {
        sanitized
    }
}

fn configured_max_spend(cli: Option<f64>) -> Result<Option<f64>> {
    if cli.is_some() {
        return Ok(cli);
    }
    match std::env::var("CORVID_EVAL_MAX_SPEND_USD") {
        Ok(raw) => raw
            .parse::<f64>()
            .map(Some)
            .with_context(|| "CORVID_EVAL_MAX_SPEND_USD must be a number"),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error).context("failed to read CORVID_EVAL_MAX_SPEND_USD"),
    }
}

fn planned_eval_spend(inputs: &[PathBuf]) -> Result<f64> {
    inputs.iter().try_fold(0.0, |total, input| {
        Ok(total
            + prior_eval_cost(input).with_context(|| {
                format!("failed to estimate eval spend for `{}`", input.display())
            })?)
    })
}

fn prior_eval_cost(source: &Path) -> Result<f64> {
    let summary_path = latest_summary_path_for_source(source);
    if !summary_path.exists() {
        return Ok(0.0);
    }
    let summary = read_summary_file(&summary_path)?;
    Ok(summary.trace.total_cost_usd)
}

fn latest_summary_path_for_source(source: &Path) -> PathBuf {
    let base = source.parent().unwrap_or_else(|| Path::new("."));
    let stem = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("suite");
    base.join("target")
        .join("eval")
        .join(sanitize_path_segment(stem))
        .join("latest.json")
}

fn sanitize_path_segment(raw: &str) -> String {
    let sanitized: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "suite".into()
    } else {
        sanitized
    }
}

fn run_compare(args: &[PathBuf]) -> Result<u8> {
    let (base, head) = parse_compare_args(args)?;
    let base_summaries = load_compare_summaries(&base)
        .with_context(|| format!("failed to load base eval summary `{base}`"))?;
    let head_summaries = load_compare_summaries(&head)
        .with_context(|| format!("failed to load head eval summary `{head}`"))?;
    let report = build_compare_report(&base, &head, &base_summaries, &head_summaries)?;
    print!("{}", report.render());
    Ok(if report.has_regression() { 1 } else { 0 })
}

fn parse_compare_args(args: &[PathBuf]) -> Result<(String, String)> {
    match args {
        [range] => {
            let range = range.to_string_lossy();
            let Some((base, head)) = range.split_once("..") else {
                anyhow::bail!("expected `corvid eval compare <base>..<head>`");
            };
            if base.is_empty() || head.is_empty() {
                anyhow::bail!("compare range must include both base and head refs");
            }
            Ok((base.into(), head.into()))
        }
        [base, head] => Ok((base.to_string_lossy().into(), head.to_string_lossy().into())),
        _ => anyhow::bail!("expected `corvid eval compare <base>..<head>`"),
    }
}

#[derive(Debug, Clone, Deserialize)]
struct StoredEvalSummary {
    source_path: String,
    evals: Vec<StoredEval>,
    #[serde(default)]
    trace: StoredTraceSummary,
}

#[derive(Debug, Clone, Deserialize)]
struct StoredEval {
    name: String,
    status: String,
    #[serde(default)]
    assertions: Vec<StoredAssertion>,
}

#[derive(Debug, Clone, Deserialize)]
struct StoredAssertion {
    label: String,
    status: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct StoredTraceSummary {
    #[serde(default)]
    total_cost_usd: f64,
    #[serde(default)]
    total_latency_ms: u64,
    #[serde(default)]
    prompts: Vec<String>,
    #[serde(default)]
    prompt_renders: Vec<StoredPromptRender>,
    #[serde(default)]
    model_routes: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct StoredPromptRender {
    prompt: String,
    rendered: String,
}

fn load_compare_summaries(spec: &str) -> Result<Vec<StoredEvalSummary>> {
    let path = Path::new(spec);
    if path.exists() {
        return load_summaries_from_path(path);
    }
    load_summaries_from_git_ref(spec)
}

fn load_summaries_from_path(path: &Path) -> Result<Vec<StoredEvalSummary>> {
    if path.is_file() {
        return Ok(vec![read_summary_file(path)?]);
    }
    let mut files = Vec::new();
    collect_latest_summary_files(path, &mut files);
    files.sort();
    files
        .into_iter()
        .map(|file| read_summary_file(&file))
        .collect()
}

fn read_summary_file(path: &Path) -> Result<StoredEvalSummary> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read `{}`", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("failed to parse `{}`", path.display()))
}

fn collect_latest_summary_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_latest_summary_files(&path, out);
        } else if path.file_name().and_then(|name| name.to_str()) == Some("latest.json") {
            out.push(path);
        }
    }
}

fn load_summaries_from_git_ref(reference: &str) -> Result<Vec<StoredEvalSummary>> {
    let output = Command::new("git")
        .args(["ls-tree", "-r", "--name-only", reference, "target/eval"])
        .output()
        .with_context(|| format!("failed to inspect git ref `{reference}`"))?;
    if !output.status.success() {
        anyhow::bail!("`{reference}` is not a path and git could not read it as a ref");
    }
    let paths = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.ends_with("/latest.json"))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if paths.is_empty() {
        anyhow::bail!("git ref `{reference}` has no target/eval/**/latest.json summaries");
    }
    paths
        .into_iter()
        .map(|path| {
            let spec = format!("{reference}:{path}");
            let output = Command::new("git")
                .args(["show", &spec])
                .output()
                .with_context(|| format!("failed to read `{spec}`"))?;
            if !output.status.success() {
                anyhow::bail!("git could not read `{spec}`");
            }
            serde_json::from_slice(&output.stdout)
                .with_context(|| format!("failed to parse `{spec}`"))
        })
        .collect()
}

struct CompareReport {
    base: String,
    head: String,
    base_passed: usize,
    base_total: usize,
    head_passed: usize,
    head_total: usize,
    base_cost: f64,
    head_cost: f64,
    base_latency_ms: u64,
    head_latency_ms: u64,
    prompt_added: Vec<String>,
    prompt_removed: Vec<String>,
    route_added: Vec<String>,
    route_removed: Vec<String>,
    prompt_changes: Vec<PromptChange>,
    regression_clusters: Vec<RegressionCluster>,
    assertion_changes: Vec<AssertionChange>,
}

struct PromptChange {
    prompt: String,
    before: String,
    after: String,
}

struct RegressionCluster {
    cause: String,
    count: usize,
}

struct AssertionChange {
    source: String,
    eval: String,
    assertion: Option<String>,
    before: String,
    after: String,
}

impl CompareReport {
    fn has_regression(&self) -> bool {
        self.assertion_changes
            .iter()
            .any(|change| change.before == "Passed" && change.after != "Passed")
    }

    fn render(&self) -> String {
        let base_rate = percent(self.base_passed, self.base_total);
        let head_rate = percent(self.head_passed, self.head_total);
        let mut out = format!("corvid eval compare {}..{}\n", self.base, self.head);
        out.push_str(&format!(
            "pass rate: {}/{} ({base_rate:.1}%) -> {}/{} ({head_rate:.1}%) ({:+.1} pp)\n",
            self.base_passed,
            self.base_total,
            self.head_passed,
            self.head_total,
            head_rate - base_rate
        ));
        out.push_str(&format!(
            "cost: ${:.6} -> ${:.6} ({:+.6})\n",
            self.base_cost,
            self.head_cost,
            self.head_cost - self.base_cost
        ));
        out.push_str(&format!(
            "latency: {} ms -> {} ms ({:+} ms)\n",
            self.base_latency_ms,
            self.head_latency_ms,
            self.head_latency_ms as i128 - self.base_latency_ms as i128
        ));
        out.push_str(&format!(
            "prompts: +{} -{}\n",
            join_or_none(&self.prompt_added),
            join_or_none(&self.prompt_removed)
        ));
        out.push_str(&format!(
            "model routes: +{} -{}\n",
            join_or_none(&self.route_added),
            join_or_none(&self.route_removed)
        ));
        if self.prompt_changes.is_empty() {
            out.push_str("prompt diffs: none\n");
        } else {
            out.push_str("prompt diffs:\n");
            for change in &self.prompt_changes {
                out.push_str(&format!(
                    "  {}:\n    before: {}\n    after: {}\n",
                    change.prompt,
                    compact_text(&change.before),
                    compact_text(&change.after)
                ));
            }
        }
        if self.regression_clusters.is_empty() {
            out.push_str("regression clusters: none\n");
        } else {
            out.push_str("regression clusters:\n");
            for cluster in &self.regression_clusters {
                out.push_str(&format!("  {}: {}\n", cluster.cause, cluster.count));
            }
        }
        if self.assertion_changes.is_empty() {
            out.push_str("assertion changes: none\n");
        } else {
            out.push_str("assertion changes:\n");
            for change in &self.assertion_changes {
                let target = change
                    .assertion
                    .as_deref()
                    .map(|assertion| format!("{} :: {} :: {assertion}", change.source, change.eval))
                    .unwrap_or_else(|| format!("{} :: {}", change.source, change.eval));
                out.push_str(&format!(
                    "  {target}: {} -> {}\n",
                    change.before, change.after
                ));
            }
        }
        out
    }
}

fn build_compare_report(
    base: &str,
    head: &str,
    base_summaries: &[StoredEvalSummary],
    head_summaries: &[StoredEvalSummary],
) -> Result<CompareReport> {
    if base_summaries.is_empty() || head_summaries.is_empty() {
        anyhow::bail!("both compare sides must contain at least one eval summary");
    }
    let base_index = index_summaries(base_summaries);
    let head_index = index_summaries(head_summaries);
    let mut prompt_base = BTreeSet::new();
    let mut prompt_head = BTreeSet::new();
    let mut route_base = BTreeSet::new();
    let mut route_head = BTreeSet::new();
    let mut assertion_changes = Vec::new();
    let base_prompts = prompt_render_index(base_summaries);
    let head_prompts = prompt_render_index(head_summaries);

    for summary in base_summaries {
        prompt_base.extend(summary.trace.prompts.iter().cloned());
        route_base.extend(summary.trace.model_routes.iter().cloned());
    }
    for summary in head_summaries {
        prompt_head.extend(summary.trace.prompts.iter().cloned());
        route_head.extend(summary.trace.model_routes.iter().cloned());
    }

    for ((source, eval, assertion), after) in &head_index {
        if let Some(before) = base_index.get(&(source.clone(), eval.clone(), assertion.clone())) {
            if before != after {
                assertion_changes.push(AssertionChange {
                    source: source.clone(),
                    eval: eval.clone(),
                    assertion: assertion.clone(),
                    before: before.clone(),
                    after: after.clone(),
                });
            }
        }
    }
    let prompt_changes = prompt_render_changes(&base_prompts, &head_prompts);
    let regression_clusters = cluster_regressions(
        &assertion_changes,
        !prompt_changes.is_empty(),
        !route_base.is_empty() && route_base != route_head,
        head_summaries
            .iter()
            .map(|summary| summary.trace.total_cost_usd)
            .sum::<f64>()
            > base_summaries
                .iter()
                .map(|summary| summary.trace.total_cost_usd)
                .sum::<f64>(),
    );

    Ok(CompareReport {
        base: base.into(),
        head: head.into(),
        base_passed: base_summaries.iter().map(count_passed_evals).sum(),
        base_total: base_summaries
            .iter()
            .map(|summary| summary.evals.len())
            .sum(),
        head_passed: head_summaries.iter().map(count_passed_evals).sum(),
        head_total: head_summaries
            .iter()
            .map(|summary| summary.evals.len())
            .sum(),
        base_cost: base_summaries
            .iter()
            .map(|summary| summary.trace.total_cost_usd)
            .sum(),
        head_cost: head_summaries
            .iter()
            .map(|summary| summary.trace.total_cost_usd)
            .sum(),
        base_latency_ms: base_summaries
            .iter()
            .map(|summary| summary.trace.total_latency_ms)
            .sum(),
        head_latency_ms: head_summaries
            .iter()
            .map(|summary| summary.trace.total_latency_ms)
            .sum(),
        prompt_added: set_diff(&prompt_head, &prompt_base),
        prompt_removed: set_diff(&prompt_base, &prompt_head),
        route_added: set_diff(&route_head, &route_base),
        route_removed: set_diff(&route_base, &route_head),
        prompt_changes,
        regression_clusters,
        assertion_changes,
    })
}

fn prompt_render_index(summaries: &[StoredEvalSummary]) -> BTreeMap<String, String> {
    let mut index = BTreeMap::new();
    for summary in summaries {
        for render in &summary.trace.prompt_renders {
            index
                .entry(render.prompt.clone())
                .or_insert_with(|| render.rendered.clone());
        }
    }
    index
}

fn prompt_render_changes(
    base: &BTreeMap<String, String>,
    head: &BTreeMap<String, String>,
) -> Vec<PromptChange> {
    head.iter()
        .filter_map(|(prompt, after)| {
            let before = base.get(prompt)?;
            (before != after).then(|| PromptChange {
                prompt: prompt.clone(),
                before: before.clone(),
                after: after.clone(),
            })
        })
        .collect()
}

fn cluster_regressions(
    changes: &[AssertionChange],
    prompt_changed: bool,
    route_changed: bool,
    cost_increased: bool,
) -> Vec<RegressionCluster> {
    let mut clusters: BTreeMap<String, usize> = BTreeMap::new();
    for change in changes {
        if !(change.before == "Passed" && change.after != "Passed") {
            continue;
        }
        let cause = if prompt_changed {
            "prompt change"
        } else if route_changed {
            "route change"
        } else if cost_increased {
            "budget regression"
        } else if change
            .assertion
            .as_deref()
            .is_some_and(|label| label.starts_with("assert approved "))
        {
            "approval-path change"
        } else if change
            .assertion
            .as_deref()
            .is_some_and(|label| label.starts_with("assert called "))
        {
            "tool-output/process change"
        } else {
            "assertion regression"
        };
        *clusters.entry(cause.into()).or_default() += 1;
    }
    clusters
        .into_iter()
        .map(|(cause, count)| RegressionCluster { cause, count })
        .collect()
}

fn index_summaries(
    summaries: &[StoredEvalSummary],
) -> BTreeMap<(String, String, Option<String>), String> {
    let mut index = BTreeMap::new();
    for summary in summaries {
        for eval in &summary.evals {
            index.insert(
                (summary.source_path.clone(), eval.name.clone(), None),
                eval.status.clone(),
            );
            for assertion in &eval.assertions {
                index.insert(
                    (
                        summary.source_path.clone(),
                        eval.name.clone(),
                        Some(assertion.label.clone()),
                    ),
                    assertion.status.clone(),
                );
            }
        }
    }
    index
}

fn count_passed_evals(summary: &StoredEvalSummary) -> usize {
    summary
        .evals
        .iter()
        .filter(|eval| eval.status == "Passed")
        .count()
}

fn set_diff(left: &BTreeSet<String>, right: &BTreeSet<String>) -> Vec<String> {
    left.difference(right).cloned().collect()
}

fn percent(passed: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        passed as f64 * 100.0 / total as f64
    }
}

fn join_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "none".into()
    } else {
        values.join(", ")
    }
}

fn compact_text(value: &str) -> String {
    const LIMIT: usize = 120;
    let flattened = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if flattened.len() <= LIMIT {
        flattened
    } else {
        format!("{}...", &flattened[..LIMIT])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_without_inputs_prints_usage() {
        let code = run_eval(&[], None, None, None, None, None).expect("usage returns an exit code");
        assert_eq!(code, 1);
    }

    #[test]
    fn eval_swap_model_requires_inputs() {
        let err = run_eval(&[], None, Some("candidate"), None, None, None).unwrap_err();
        assert!(
            err.to_string().contains("requires at least one trace"),
            "{err:#}"
        );
    }

    #[test]
    fn eval_budget_fails_before_running_when_prior_cost_exceeds_max() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = dir.path().join("suite.cor");
        std::fs::write(&source, "eval math:\n    assert true\n").expect("source");
        let summary_path = latest_summary_path_for_source(&source);
        std::fs::create_dir_all(summary_path.parent().unwrap()).expect("summary dir");
        std::fs::write(
            &summary_path,
            r#"{
  "source_path": "suite.cor",
  "evals": [],
  "compile_ok": true,
  "trace": { "total_cost_usd": 0.25, "total_latency_ms": 0, "prompts": [], "model_routes": [] }
}"#,
        )
        .expect("summary");

        let code = run_eval(&[source], None, None, Some(0.10), None, None).expect("budget result");
        assert_eq!(code, 1);
    }

    #[test]
    fn eval_golden_traces_and_swap_model_are_exclusive() {
        let err = run_eval(
            &[],
            None,
            Some("candidate"),
            None,
            Some(Path::new("traces")),
            None,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("separate modes"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn eval_promote_writes_redacted_lineage_fixture() {
        let dir = tempfile::tempdir().expect("tempdir");
        let trace_path = dir.path().join("trace-1.lineage.jsonl");
        let out_dir = dir.path().join("fixtures");
        let mut route = corvid_runtime::LineageEvent::root(
            "trace-1",
            corvid_runtime::LineageKind::Route,
            "POST /send",
            1,
        )
        .finish(corvid_runtime::LineageStatus::Ok, 10);
        route.replay_key = "replay-secret".to_string();
        let mut tool = corvid_runtime::LineageEvent::child(
            &route,
            corvid_runtime::LineageKind::Tool,
            "email alice@example.com",
            0,
            2,
        )
        .finish(corvid_runtime::LineageStatus::Failed, 8);
        tool.guarantee_id = "approval.reachable_entrypoints_require_contract".to_string();
        tool.effect_ids = vec!["send_email".to_string()];
        let body = [route, tool]
            .iter()
            .map(|event| serde_json::to_string(event).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&trace_path, format!("{body}\n")).expect("trace");

        let code = run_eval(
            &[PathBuf::from("promote"), trace_path.clone()],
            None,
            None,
            None,
            None,
            Some(&out_dir),
        )
        .expect("promote");
        assert_eq!(code, 0);
        let fixture_path = out_dir.join("trace-1.lineage-eval.json");
        let json = std::fs::read_to_string(fixture_path).expect("fixture");
        assert!(json.contains(LINEAGE_EVAL_FIXTURE_SCHEMA));
        assert!(json.contains("fixture_hash"));
        assert!(!json.contains("alice@example.com"));
        assert!(!json.contains("replay-secret"));
    }

    #[test]
    fn eval_compare_reports_pass_cost_latency_route_and_assertion_deltas() {
        let base = StoredEvalSummary {
            source_path: "suite.cor".into(),
            evals: vec![StoredEval {
                name: "quality".into(),
                status: "Passed".into(),
                assertions: vec![StoredAssertion {
                    label: "assert <expr>".into(),
                    status: "Passed".into(),
                }],
            }],
            trace: StoredTraceSummary {
                total_cost_usd: 0.01,
                total_latency_ms: 10,
                prompts: vec!["draft".into()],
                prompt_renders: vec![StoredPromptRender {
                    prompt: "draft".into(),
                    rendered: "old prompt body".into(),
                }],
                model_routes: vec!["draft:cheap".into()],
                ..StoredTraceSummary::default()
            },
        };
        let head = StoredEvalSummary {
            source_path: "suite.cor".into(),
            evals: vec![StoredEval {
                name: "quality".into(),
                status: "Failed".into(),
                assertions: vec![StoredAssertion {
                    label: "assert <expr>".into(),
                    status: "Failed".into(),
                }],
            }],
            trace: StoredTraceSummary {
                total_cost_usd: 0.03,
                total_latency_ms: 25,
                prompts: vec!["draft".into(), "review".into()],
                prompt_renders: vec![StoredPromptRender {
                    prompt: "draft".into(),
                    rendered: "new prompt body".into(),
                }],
                model_routes: vec!["draft:strong".into()],
                ..StoredTraceSummary::default()
            },
        };

        let report = build_compare_report("base", "head", &[base], &[head]).expect("compare");
        assert!(report.has_regression());
        let rendered = report.render();
        assert!(
            rendered.contains("pass rate: 1/1 (100.0%) -> 0/1 (0.0%)"),
            "{rendered}"
        );
        assert!(
            rendered.contains("cost: $0.010000 -> $0.030000"),
            "{rendered}"
        );
        assert!(rendered.contains("latency: 10 ms -> 25 ms"), "{rendered}");
        assert!(rendered.contains("prompts: +review -none"), "{rendered}");
        assert!(
            rendered.contains("model routes: +draft:strong -draft:cheap"),
            "{rendered}"
        );
        assert!(rendered.contains("prompt diffs:"), "{rendered}");
        assert!(rendered.contains("regression clusters:"), "{rendered}");
        assert!(rendered.contains("prompt change: 2"), "{rendered}");
        assert!(
            rendered.contains("suite.cor :: quality :: assert <expr>: Passed -> Failed"),
            "{rendered}"
        );
    }
}
