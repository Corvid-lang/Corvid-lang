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
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn run_eval(
    inputs: &[PathBuf],
    source: Option<&Path>,
    swap_model: Option<&str>,
) -> Result<u8> {
    let Some(model) = swap_model else {
        return run_source_evals(inputs);
    };

    if inputs.is_empty() {
        anyhow::bail!("`corvid eval --swap-model` requires at least one trace file or directory");
    }

    eprintln!("eval model-swap mode - target model: `{model}`");
    let mut exit_code = 0_u8;
    for input in inputs {
        let code = if input.is_dir() {
            eprintln!("running trace-suite migration analysis: {}", input.display());
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

fn run_source_evals(inputs: &[PathBuf]) -> Result<u8> {
    if inputs
        .first()
        .and_then(|input| input.to_str())
        .is_some_and(|input| input == "compare")
    {
        return run_compare(&inputs[1..]);
    }

    if inputs.is_empty() {
        eprintln!("usage: `corvid eval <file.cor> [more.cor ...]`");
        eprintln!("       `corvid eval compare <base>..<head>`");
        eprintln!(
            "For model migration analysis, use `corvid eval --swap-model <MODEL> --source <FILE> <TRACE_OR_DIR>...`."
        );
        return Ok(1);
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
    model_routes: Vec<String>,
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
    files.into_iter().map(|file| read_summary_file(&file)).collect()
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
    assertion_changes: Vec<AssertionChange>,
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

    Ok(CompareReport {
        base: base.into(),
        head: head.into(),
        base_passed: base_summaries.iter().map(count_passed_evals).sum(),
        base_total: base_summaries.iter().map(|summary| summary.evals.len()).sum(),
        head_passed: head_summaries.iter().map(count_passed_evals).sum(),
        head_total: head_summaries.iter().map(|summary| summary.evals.len()).sum(),
        base_cost: base_summaries.iter().map(|summary| summary.trace.total_cost_usd).sum(),
        head_cost: head_summaries.iter().map(|summary| summary.trace.total_cost_usd).sum(),
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
        assertion_changes,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_without_inputs_prints_usage() {
        let code = run_eval(&[], None, None).expect("usage returns an exit code");
        assert_eq!(code, 1);
    }

    #[test]
    fn eval_swap_model_requires_inputs() {
        let err = run_eval(&[], None, Some("candidate")).unwrap_err();
        assert!(
            err.to_string().contains("requires at least one trace"),
            "{err:#}"
        );
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
                model_routes: vec!["draft:strong".into()],
                ..StoredTraceSummary::default()
            },
        };

        let report = build_compare_report("base", "head", &[base], &[head]).expect("compare");
        assert!(report.has_regression());
        let rendered = report.render();
        assert!(rendered.contains("pass rate: 1/1 (100.0%) -> 0/1 (0.0%)"), "{rendered}");
        assert!(rendered.contains("cost: $0.010000 -> $0.030000"), "{rendered}");
        assert!(rendered.contains("latency: 10 ms -> 25 ms"), "{rendered}");
        assert!(rendered.contains("prompts: +review -none"), "{rendered}");
        assert!(rendered.contains("model routes: +draft:strong -draft:cheap"), "{rendered}");
        assert!(rendered.contains("suite.cor :: quality :: assert <expr>: Passed -> Failed"), "{rendered}");
    }
}
