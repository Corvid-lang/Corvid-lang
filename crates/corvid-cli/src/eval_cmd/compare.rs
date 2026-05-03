use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) fn run_compare(args: &[PathBuf]) -> Result<u8> {
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

pub(super) fn read_summary_cost_usd(path: &Path) -> Result<f64> {
    Ok(read_summary_file(path)?.trace.total_cost_usd)
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
