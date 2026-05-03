use crate::{compile_to_ir_with_config_at_path, load_corvid_config_for, render_all_pretty};
use corvid_ir::{IrFile, IrTest};
use corvid_runtime::Runtime;
use corvid_trace_schema::{read_events_from_path, validate_supported_schema, TraceEvent};
use corvid_vm::{
    run_all_tests_with_options, SnapshotOptions, TestAssertionStatus, TestExecution,
    TestRunOptions, TraceFixtureOptions,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

mod report;

pub use report::{
    CorvidEvalReport, EvalPromptRender, EvalRegression, EvalRegressionReport, EvalRunnerError,
    EvalTraceReport,
};

pub async fn run_evals_at_path(
    path: &Path,
    runtime: &Runtime,
) -> Result<CorvidEvalReport, EvalRunnerError> {
    run_evals_at_path_with_options(path, runtime, default_eval_options(path)).await
}

pub async fn run_evals_at_path_with_options(
    path: &Path,
    runtime: &Runtime,
    options: TestRunOptions,
) -> Result<CorvidEvalReport, EvalRunnerError> {
    let source = std::fs::read_to_string(path).map_err(|error| EvalRunnerError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    let html_report_path = html_report_path(path);
    let latest_path = latest_results_path(path);
    let prior_summary = read_eval_summary(&latest_path);
    let config = load_corvid_config_for(path);
    let ir = match compile_to_ir_with_config_at_path(&source, path, config.as_ref()) {
        Ok(ir) => ir,
        Err(diagnostics) => {
            let trace = collect_trace_report(path, &[]);
            let summary = EvalSummary::from_compile_error(path, &trace);
            let regression = persist_and_compare_summary(path, prior_summary.as_ref(), &summary)
                .map_err(|error| EvalRunnerError::Io {
                    path: latest_path.clone(),
                    error,
                })?;
            let report = CorvidEvalReport {
                source_path: path.to_path_buf(),
                compile_diagnostics: diagnostics,
                evals: Vec::new(),
                html_report_path,
                regression,
                trace,
            };
            write_eval_html_report(&report).map_err(|error| EvalRunnerError::Io {
                path: report.html_report_path.clone(),
                error,
            })?;
            return Ok(report);
        }
    };
    let eval_ir = evals_as_tests(&ir);
    let evals = run_all_tests_with_options(&eval_ir, runtime, options).await;
    let trace = collect_trace_report(path, &evals);
    let summary = EvalSummary::from_evals(path, &evals, &trace);
    let regression =
        persist_and_compare_summary(path, prior_summary.as_ref(), &summary).map_err(|error| {
            EvalRunnerError::Io {
                path: latest_path,
                error,
            }
        })?;
    let report = CorvidEvalReport {
        source_path: path.to_path_buf(),
        compile_diagnostics: Vec::new(),
        trace,
        evals,
        html_report_path,
        regression,
    };
    write_eval_html_report(&report).map_err(|error| EvalRunnerError::Io {
        path: report.html_report_path.clone(),
        error,
    })?;
    Ok(report)
}

pub fn default_eval_options(path: &Path) -> TestRunOptions {
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("suite");
    TestRunOptions {
        snapshots: Some(SnapshotOptions {
            root: eval_output_dir(path).join("snapshots"),
            update: std::env::var("CORVID_UPDATE_SNAPSHOTS")
                .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
                .unwrap_or(false),
        }),
        trace_fixtures: Some(TraceFixtureOptions {
            root: base
                .join("target")
                .join("eval")
                .join(sanitize_path_segment(stem)),
        }),
    }
}

pub fn render_eval_report(report: &CorvidEvalReport, source: Option<&str>) -> String {
    let mut out = String::new();
    if !report.compile_diagnostics.is_empty() {
        if let Some(source) = source {
            out.push_str(&render_all_pretty(
                &report.compile_diagnostics,
                &report.source_path,
                source,
            ));
        } else {
            for diagnostic in &report.compile_diagnostics {
                out.push_str(&diagnostic.render(&report.source_path, ""));
                out.push('\n');
            }
        }
        out.push_str(&format!(
            "\nHTML report: {}\n",
            report.html_report_path.display()
        ));
        return out;
    }

    out.push_str(&format!("corvid eval {}\n", report.source_path.display()));
    if report.evals.is_empty() {
        out.push_str("  no eval declarations found\n");
        out.push_str("\n0 passed, 0 failed\n");
        out.push_str(&format!(
            "HTML report: {}\n",
            report.html_report_path.display()
        ));
        return out;
    }

    let mut passed = 0_usize;
    let mut failed = 0_usize;
    for eval in &report.evals {
        if eval.passed() {
            passed += 1;
            out.push_str(&format!("  PASS {}\n", eval.name));
        } else {
            failed += 1;
            out.push_str(&format!("  FAIL {}\n", eval.name));
            if let Some(error) = &eval.setup_error {
                out.push_str(&format!("    setup error: {error}\n"));
            }
            for assertion in &eval.assertions {
                if assertion.status == TestAssertionStatus::Passed {
                    continue;
                }
                out.push_str(&format!("    {:?}: {}", assertion.status, assertion.label));
                if let Some(message) = &assertion.message {
                    out.push_str(&format!(" - {message}"));
                }
                out.push('\n');
            }
        }
    }
    out.push_str(&format!("\n{passed} passed, {failed} failed\n"));
    render_trace_summary(report, &mut out);
    render_regression_summary(report, &mut out);
    out.push_str(&format!(
        "HTML report: {}\n",
        report.html_report_path.display()
    ));
    out
}

fn render_trace_summary(report: &CorvidEvalReport, out: &mut String) {
    let trace = &report.trace;
    out.push_str(&format!(
        "Trace report: {} trace{}, {} replay-compatible",
        trace.trace_count,
        if trace.trace_count == 1 { "" } else { "s" },
        trace.replay_compatible_count
    ));
    if !trace.invalid_traces.is_empty() {
        out.push_str(&format!(", {} invalid", trace.invalid_traces.len()));
    }
    out.push('\n');
    out.push_str(&format!(
        "  values: {}/{} passed; process: {}/{} passed; approvals: {}/{} passed\n",
        trace.value_assertions_passed,
        trace.value_assertions_total,
        trace.process_assertions_passed,
        trace.process_assertions_total,
        trace.approval_assertions_passed,
        trace.approval_assertions_total
    ));
    out.push_str(&format!(
        "  calls: {} tool, {} prompt, {} approval; grounded edges: {}; cost: ${:.6}; latency: {} ms\n",
        trace.tool_calls,
        trace.prompt_calls,
        trace.approval_events,
        trace.grounded_edges,
        trace.total_cost_usd,
        trace.total_latency_ms
    ));
    if trace.model_routes.is_empty() {
        out.push_str("  model routes: none recorded\n");
    } else {
        out.push_str(&format!(
            "  model routes: {}\n",
            trace.model_routes.join(", ")
        ));
    }
}

fn render_regression_summary(report: &CorvidEvalReport, out: &mut String) {
    if report.regression.current_path.as_os_str().is_empty() {
        return;
    }
    if report.regression.regressions.is_empty() {
        out.push_str("Regressions: 0\n");
        return;
    }
    out.push_str(&format!(
        "Regressions: {} new failure{}\n",
        report.regression.regressions.len(),
        if report.regression.regressions.len() == 1 {
            ""
        } else {
            "s"
        }
    ));
    for regression in &report.regression.regressions {
        let target = regression
            .assertion
            .as_deref()
            .map(|assertion| format!("{} :: {assertion}", regression.eval))
            .unwrap_or_else(|| regression.eval.clone());
        out.push_str(&format!(
            "  {target}: {} -> {}\n",
            regression.before, regression.after
        ));
    }
}

fn evals_as_tests(ir: &IrFile) -> IrFile {
    let mut out = ir.clone();
    out.tests = ir
        .evals
        .iter()
        .map(|eval| IrTest {
            id: eval.id,
            name: eval.name.clone(),
            trace_fixture: None,
            body: eval.body.clone(),
            assertions: eval.assertions.clone(),
            span: eval.span,
        })
        .collect();
    out
}

fn write_eval_html_report(report: &CorvidEvalReport) -> std::io::Result<()> {
    if let Some(parent) = report.html_report_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&report.html_report_path, render_eval_html_report(report))
}

fn render_eval_html_report(report: &CorvidEvalReport) -> String {
    let mut out = String::new();
    out.push_str("<!doctype html><html><head><meta charset=\"utf-8\">");
    out.push_str("<title>Corvid eval report</title>");
    out.push_str("<style>body{font-family:system-ui,sans-serif;margin:2rem;line-height:1.45;color:#17202a}table{border-collapse:collapse;width:100%;margin-top:1rem}th,td{border:1px solid #d7dde5;padding:.5rem;text-align:left;vertical-align:top}.pass{color:#146c2e}.fail{color:#a51d2d}.meta{color:#5c6570}</style>");
    out.push_str("</head><body>");
    out.push_str(&format!(
        "<h1>Corvid eval</h1><p class=\"meta\">{}</p>",
        escape_html(&report.source_path.display().to_string())
    ));
    if !report.compile_diagnostics.is_empty() {
        out.push_str("<h2>Compile diagnostics</h2><ul>");
        for diagnostic in &report.compile_diagnostics {
            out.push_str(&format!("<li>{}</li>", escape_html(&diagnostic.message)));
        }
        out.push_str("</ul></body></html>");
        return out;
    }
    if report.evals.is_empty() {
        out.push_str("<p class=\"fail\">No eval declarations found.</p></body></html>");
        return out;
    }
    if report.regression.regressions.is_empty() {
        out.push_str("<p class=\"meta\">Regressions: 0</p>");
    } else {
        out.push_str("<h2>Regressions</h2><ul>");
        for regression in &report.regression.regressions {
            let target = regression
                .assertion
                .as_deref()
                .map(|assertion| format!("{} :: {assertion}", regression.eval))
                .unwrap_or_else(|| regression.eval.clone());
            out.push_str(&format!(
                "<li><strong>{}</strong>: {} -&gt; {}</li>",
                escape_html(&target),
                escape_html(&regression.before),
                escape_html(&regression.after)
            ));
        }
        out.push_str("</ul>");
    }
    out.push_str("<h2>Trace report</h2>");
    out.push_str(&format!(
        "<p>{} trace(s), {} replay-compatible, {} invalid.</p>",
        report.trace.trace_count,
        report.trace.replay_compatible_count,
        report.trace.invalid_traces.len()
    ));
    out.push_str("<ul>");
    out.push_str(&format!(
        "<li>Value assertions: {}/{}</li>",
        report.trace.value_assertions_passed, report.trace.value_assertions_total
    ));
    out.push_str(&format!(
        "<li>Process assertions: {}/{}</li>",
        report.trace.process_assertions_passed, report.trace.process_assertions_total
    ));
    out.push_str(&format!(
        "<li>Approval assertions: {}/{}</li>",
        report.trace.approval_assertions_passed, report.trace.approval_assertions_total
    ));
    out.push_str(&format!(
        "<li>Calls: {} tool, {} prompt, {} approval</li>",
        report.trace.tool_calls, report.trace.prompt_calls, report.trace.approval_events
    ));
    out.push_str(&format!(
        "<li>Groundedness: {} provenance edge(s)</li>",
        report.trace.grounded_edges
    ));
    out.push_str(&format!(
        "<li>Cost: ${:.6}; latency: {} ms</li>",
        report.trace.total_cost_usd, report.trace.total_latency_ms
    ));
    out.push_str(&format!(
        "<li>Model routes: {}</li>",
        escape_html(&if report.trace.model_routes.is_empty() {
            "none recorded".into()
        } else {
            report.trace.model_routes.join(", ")
        })
    ));
    out.push_str("</ul>");
    out.push_str(
        "<table><thead><tr><th>Eval</th><th>Status</th><th>Assertions</th></tr></thead><tbody>",
    );
    for eval in &report.evals {
        let class = if eval.passed() { "pass" } else { "fail" };
        let status = if eval.passed() { "PASS" } else { "FAIL" };
        out.push_str(&format!(
            "<tr><td>{}</td><td class=\"{}\">{}</td><td>",
            escape_html(&eval.name),
            class,
            status
        ));
        if let Some(error) = &eval.setup_error {
            out.push_str(&format!(
                "<div class=\"fail\">setup error: {}</div>",
                escape_html(error)
            ));
        }
        out.push_str("<ul>");
        for assertion in &eval.assertions {
            out.push_str(&format!(
                "<li><strong>{:?}</strong> {}",
                assertion.status,
                escape_html(&assertion.label)
            ));
            if let Some(message) = &assertion.message {
                out.push_str(&format!(" - {}", escape_html(message)));
            }
            out.push_str("</li>");
        }
        out.push_str("</ul></td></tr>");
    }
    out.push_str("</tbody></table></body></html>");
    out
}

fn html_report_path(path: &Path) -> PathBuf {
    eval_output_dir(path).join("report.html")
}

fn latest_results_path(path: &Path) -> PathBuf {
    eval_output_dir(path).join("latest.json")
}

fn previous_results_path(path: &Path) -> PathBuf {
    eval_output_dir(path).join("previous.json")
}

fn eval_output_dir(path: &Path) -> PathBuf {
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("suite");
    base.join("target")
        .join("eval")
        .join(sanitize_path_segment(stem))
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

fn escape_html(raw: &str) -> String {
    raw.chars()
        .flat_map(|ch| match ch {
            '&' => "&amp;".chars().collect::<Vec<_>>(),
            '<' => "&lt;".chars().collect(),
            '>' => "&gt;".chars().collect(),
            '"' => "&quot;".chars().collect(),
            '\'' => "&#39;".chars().collect(),
            other => vec![other],
        })
        .collect()
}

fn collect_trace_report(path: &Path, evals: &[TestExecution]) -> EvalTraceReport {
    let mut report = EvalTraceReport::default();
    for eval in evals {
        for assertion in &eval.assertions {
            let passed = assertion.status == TestAssertionStatus::Passed;
            if assertion.label == "assert <expr>" {
                report.value_assertions_total += 1;
                if passed {
                    report.value_assertions_passed += 1;
                }
            }
            if is_process_assertion(&assertion.label) {
                report.process_assertions_total += 1;
                if passed {
                    report.process_assertions_passed += 1;
                }
            }
            if assertion.label.starts_with("assert approved ") {
                report.approval_assertions_total += 1;
                if passed {
                    report.approval_assertions_passed += 1;
                }
            }
        }
    }

    let mut prompts = BTreeSet::new();
    let mut prompt_renders = BTreeSet::new();
    let mut routes = BTreeSet::new();
    for trace_path in find_trace_artifacts(&eval_output_dir(path)) {
        report.trace_count += 1;
        match read_events_from_path(&trace_path)
            .map_err(|error| error.to_string())
            .and_then(|events| {
                validate_supported_schema(&events)
                    .map_err(|error| error.to_string())
                    .map(|()| events)
            }) {
            Ok(events) => {
                if is_replay_compatible_trace(&events) {
                    report.replay_compatible_count += 1;
                }
                summarize_trace_events(
                    &events,
                    &mut report,
                    &mut prompts,
                    &mut prompt_renders,
                    &mut routes,
                );
            }
            Err(error) => report
                .invalid_traces
                .push(format!("{}: {error}", trace_path.display())),
        }
    }
    report.prompts = prompts.into_iter().collect();
    report.prompt_renders = prompt_renders
        .into_iter()
        .map(|(prompt, rendered)| EvalPromptRender { prompt, rendered })
        .collect();
    report.model_routes = routes.into_iter().collect();
    report
}

fn is_process_assertion(label: &str) -> bool {
    label.starts_with("assert called ") || label.starts_with("assert cost < ")
}

fn find_trace_artifacts(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_trace_artifacts(root, &mut out);
    out.sort();
    out
}

fn collect_trace_artifacts(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_trace_artifacts(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

fn is_replay_compatible_trace(events: &[TraceEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, TraceEvent::RunStarted { .. }))
        && events
            .iter()
            .any(|event| matches!(event, TraceEvent::RunCompleted { .. }))
}

fn summarize_trace_events(
    events: &[TraceEvent],
    report: &mut EvalTraceReport,
    prompts: &mut BTreeSet<String>,
    prompt_renders: &mut BTreeSet<(String, String)>,
    routes: &mut BTreeSet<String>,
) {
    let mut started_at = None;
    let mut completed_at = None;
    for event in events {
        match event {
            TraceEvent::RunStarted { ts_ms, .. } => started_at = Some(*ts_ms),
            TraceEvent::RunCompleted { ts_ms, .. } => completed_at = Some(*ts_ms),
            TraceEvent::ToolCall { .. } => report.tool_calls += 1,
            TraceEvent::LlmCall {
                prompt,
                model,
                model_version,
                rendered,
                ..
            } => {
                report.prompt_calls += 1;
                prompts.insert(prompt.clone());
                if let Some(rendered) = rendered {
                    prompt_renders.insert((prompt.clone(), rendered.clone()));
                }
                if let Some(model) = model {
                    routes.insert(model_route_label(prompt, model, model_version.as_deref()));
                }
            }
            TraceEvent::ApprovalRequest { .. }
            | TraceEvent::ApprovalDecision { .. }
            | TraceEvent::ApprovalResponse { .. } => report.approval_events += 1,
            TraceEvent::ModelSelected {
                prompt,
                model,
                model_version,
                cost_estimate,
                ..
            } => {
                prompts.insert(prompt.clone());
                if cost_estimate.is_finite() && *cost_estimate > 0.0 {
                    report.total_cost_usd += *cost_estimate;
                }
                routes.insert(model_route_label(prompt, model, model_version.as_deref()));
            }
            TraceEvent::HostEvent { payload, .. } => {
                if let Some(cost) = payload
                    .get("cost_usd")
                    .and_then(|value| value.as_f64())
                    .filter(|value| value.is_finite() && *value > 0.0)
                {
                    report.total_cost_usd += cost;
                }
            }
            TraceEvent::ProvenanceEdge { .. } => report.grounded_edges += 1,
            _ => {}
        }
    }
    if let (Some(start), Some(end)) = (started_at, completed_at) {
        report.total_latency_ms += end.saturating_sub(start);
    }
}

fn model_route_label(prompt: &str, model: &str, version: Option<&str>) -> String {
    match version {
        Some(version) if !version.is_empty() => format!("{prompt}:{model}@{version}"),
        _ => format!("{prompt}:{model}"),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EvalSummary {
    source_path: String,
    evals: Vec<EvalSummaryEntry>,
    compile_ok: bool,
    trace: EvalTraceSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EvalSummaryEntry {
    name: String,
    status: String,
    assertions: Vec<EvalAssertionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EvalAssertionSummary {
    label: String,
    status: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct EvalTraceSummary {
    trace_count: usize,
    replay_compatible_count: usize,
    value_assertions_passed: usize,
    value_assertions_total: usize,
    process_assertions_passed: usize,
    process_assertions_total: usize,
    approval_assertions_passed: usize,
    approval_assertions_total: usize,
    tool_calls: usize,
    prompt_calls: usize,
    approval_events: usize,
    grounded_edges: usize,
    total_cost_usd: f64,
    total_latency_ms: u64,
    prompts: Vec<String>,
    prompt_renders: Vec<EvalPromptRenderSummary>,
    model_routes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EvalPromptRenderSummary {
    prompt: String,
    rendered: String,
}

impl EvalSummary {
    fn from_compile_error(path: &Path, trace: &EvalTraceReport) -> Self {
        Self {
            source_path: path.display().to_string(),
            evals: Vec::new(),
            compile_ok: false,
            trace: EvalTraceSummary::from(trace),
        }
    }

    fn from_evals(path: &Path, evals: &[TestExecution], trace: &EvalTraceReport) -> Self {
        Self {
            source_path: path.display().to_string(),
            evals: evals
                .iter()
                .map(|eval| EvalSummaryEntry {
                    name: eval.name.clone(),
                    status: eval_status(eval).into(),
                    assertions: eval
                        .assertions
                        .iter()
                        .map(|assertion| EvalAssertionSummary {
                            label: assertion.label.clone(),
                            status: format!("{:?}", assertion.status),
                        })
                        .collect(),
                })
                .collect(),
            compile_ok: true,
            trace: EvalTraceSummary::from(trace),
        }
    }
}

impl From<&EvalTraceReport> for EvalTraceSummary {
    fn from(trace: &EvalTraceReport) -> Self {
        Self {
            trace_count: trace.trace_count,
            replay_compatible_count: trace.replay_compatible_count,
            value_assertions_passed: trace.value_assertions_passed,
            value_assertions_total: trace.value_assertions_total,
            process_assertions_passed: trace.process_assertions_passed,
            process_assertions_total: trace.process_assertions_total,
            approval_assertions_passed: trace.approval_assertions_passed,
            approval_assertions_total: trace.approval_assertions_total,
            tool_calls: trace.tool_calls,
            prompt_calls: trace.prompt_calls,
            approval_events: trace.approval_events,
            grounded_edges: trace.grounded_edges,
            total_cost_usd: trace.total_cost_usd,
            total_latency_ms: trace.total_latency_ms,
            prompts: trace.prompts.clone(),
            prompt_renders: trace
                .prompt_renders
                .iter()
                .map(|render| EvalPromptRenderSummary {
                    prompt: render.prompt.clone(),
                    rendered: render.rendered.clone(),
                })
                .collect(),
            model_routes: trace.model_routes.clone(),
        }
    }
}

fn eval_status(eval: &TestExecution) -> &'static str {
    if eval.passed() {
        "Passed"
    } else if eval.setup_error.is_some() {
        "Error"
    } else {
        "Failed"
    }
}

fn read_eval_summary(path: &Path) -> Option<EvalSummary> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn persist_and_compare_summary(
    source_path: &Path,
    prior: Option<&EvalSummary>,
    current: &EvalSummary,
) -> std::io::Result<EvalRegressionReport> {
    let current_path = latest_results_path(source_path);
    let prior_path = previous_results_path(source_path);
    if let Some(parent) = current_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if current_path.exists() {
        std::fs::copy(&current_path, &prior_path)?;
    }
    let bytes = serde_json::to_vec_pretty(current)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    std::fs::write(&current_path, bytes)?;
    Ok(EvalRegressionReport {
        prior_path,
        current_path,
        regressions: prior
            .map(|prior| compare_eval_summaries(prior, current))
            .unwrap_or_default(),
    })
}

fn compare_eval_summaries(prior: &EvalSummary, current: &EvalSummary) -> Vec<EvalRegression> {
    if !prior.compile_ok || !current.compile_ok {
        return Vec::new();
    }
    let mut regressions = Vec::new();
    for current_eval in &current.evals {
        let Some(prior_eval) = prior
            .evals
            .iter()
            .find(|candidate| candidate.name == current_eval.name)
        else {
            continue;
        };
        if prior_eval.status == "Passed" && current_eval.status != "Passed" {
            regressions.push(EvalRegression {
                eval: current_eval.name.clone(),
                assertion: None,
                before: prior_eval.status.clone(),
                after: current_eval.status.clone(),
            });
        }
        for current_assertion in &current_eval.assertions {
            let Some(prior_assertion) = prior_eval
                .assertions
                .iter()
                .find(|candidate| candidate.label == current_assertion.label)
            else {
                continue;
            };
            if prior_assertion.status == "Passed" && current_assertion.status != "Passed" {
                regressions.push(EvalRegression {
                    eval: current_eval.name.clone(),
                    assertion: Some(current_assertion.label.clone()),
                    before: prior_assertion.status.clone(),
                    after: current_assertion.status.clone(),
                });
            }
        }
    }
    regressions
}

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_runtime::Runtime;
    use corvid_trace_schema::{write_events_to_path, SCHEMA_VERSION, WRITER_INTERPRETER};

    #[tokio::test]
    async fn run_evals_at_path_reports_pass_and_fail_and_writes_html() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("suite.cor");
        std::fs::write(
            &path,
            r#"
eval math_pass:
    x = 20 + 22
    assert x == 42

eval math_fail:
    x = 1
    assert x == 2
"#,
        )
        .expect("write");

        let runtime = Runtime::builder().build();
        let report = run_evals_at_path(&path, &runtime).await.expect("run");

        assert_eq!(report.evals.len(), 2);
        assert!(report.evals[0].passed());
        assert!(!report.evals[1].passed());
        assert_eq!(report.exit_code(), 1);
        assert!(report.html_report_path.exists());
        let rendered = render_eval_report(&report, None);
        assert!(rendered.contains("corvid eval"), "{rendered}");
        assert!(rendered.contains("HTML report:"), "{rendered}");
    }

    #[tokio::test]
    async fn run_evals_at_path_fails_when_no_evals_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("empty.cor");
        std::fs::write(
            &path,
            r#"
agent answer() -> Int:
    return 42
"#,
        )
        .expect("write");

        let runtime = Runtime::builder().build();
        let report = run_evals_at_path(&path, &runtime).await.expect("run");

        assert!(report.evals.is_empty());
        assert_eq!(report.exit_code(), 1);
        assert!(report.html_report_path.exists());
    }

    #[tokio::test]
    async fn run_evals_at_path_detects_regressions_against_latest_result() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("suite.cor");
        std::fs::write(
            &path,
            r#"
eval math:
    x = 42
    assert x == 42
"#,
        )
        .expect("write");

        let runtime = Runtime::builder().build();
        let first = run_evals_at_path(&path, &runtime).await.expect("first run");
        assert!(first.regression.regressions.is_empty());
        assert!(first.regression.current_path.exists());

        std::fs::write(
            &path,
            r#"
eval math:
    x = 41
    assert x == 42
"#,
        )
        .expect("rewrite");

        let second = run_evals_at_path(&path, &runtime)
            .await
            .expect("second run");
        assert_eq!(second.regression.regressions.len(), 2);
        assert!(second.regression.prior_path.exists());
        let rendered = render_eval_report(&second, None);
        assert!(rendered.contains("Regressions: 2"), "{rendered}");
    }

    #[tokio::test]
    async fn run_evals_at_path_summarizes_trace_artifacts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("suite.cor");
        std::fs::write(
            &path,
            r#"
eval math:
    x = 42
    assert x == 42
"#,
        )
        .expect("write");
        let trace_dir = eval_output_dir(&path).join("traces");
        std::fs::create_dir_all(&trace_dir).expect("trace dir");
        write_events_to_path(
            &trace_dir.join("run.jsonl"),
            &[
                TraceEvent::SchemaHeader {
                    version: SCHEMA_VERSION,
                    writer: WRITER_INTERPRETER.into(),
                    commit_sha: None,
                    source_path: Some("suite.cor".into()),
                    ts_ms: 0,
                    run_id: "r".into(),
                },
                TraceEvent::RunStarted {
                    ts_ms: 10,
                    run_id: "r".into(),
                    agent: "answer".into(),
                    args: vec![],
                },
                TraceEvent::ModelSelected {
                    ts_ms: 20,
                    run_id: "r".into(),
                    prompt: "draft".into(),
                    model: "fast".into(),
                    model_version: Some("1".into()),
                    capability_required: None,
                    capability_picked: None,
                    output_format_required: None,
                    output_format_picked: None,
                    cost_estimate: 0.01,
                    arm_index: None,
                    stage_index: None,
                },
                TraceEvent::LlmCall {
                    ts_ms: 21,
                    run_id: "r".into(),
                    prompt: "draft".into(),
                    model: Some("fast".into()),
                    model_version: Some("1".into()),
                    rendered: Some("draft body".into()),
                    args: vec![],
                },
                TraceEvent::ToolCall {
                    ts_ms: 30,
                    run_id: "r".into(),
                    tool: "lookup".into(),
                    args: vec![],
                },
                TraceEvent::ApprovalRequest {
                    ts_ms: 40,
                    run_id: "r".into(),
                    label: "Ship".into(),
                    args: vec![],
                },
                TraceEvent::ProvenanceEdge {
                    ts_ms: 45,
                    run_id: "r".into(),
                    node_id: "tool:1".into(),
                    parents: vec![],
                    op: "tool_call:lookup".into(),
                    label: None,
                },
                TraceEvent::RunCompleted {
                    ts_ms: 55,
                    run_id: "r".into(),
                    ok: true,
                    result: Some(serde_json::json!(42)),
                    error: None,
                },
            ],
        )
        .expect("write trace");

        let runtime = Runtime::builder().build();
        let report = run_evals_at_path(&path, &runtime).await.expect("run");

        assert_eq!(report.trace.trace_count, 1);
        assert_eq!(report.trace.replay_compatible_count, 1);
        assert_eq!(report.trace.tool_calls, 1);
        assert_eq!(report.trace.prompt_calls, 1);
        assert_eq!(report.trace.approval_events, 1);
        assert_eq!(report.trace.grounded_edges, 1);
        assert_eq!(report.trace.total_latency_ms, 45);
        assert!(report.trace.prompts.contains(&"draft".into()));
        assert!(report
            .trace
            .prompt_renders
            .iter()
            .any(|render| render.prompt == "draft"));
        assert!(report.trace.model_routes.contains(&"draft:fast@1".into()));
        let rendered = render_eval_report(&report, None);
        assert!(rendered.contains("Trace report: 1 trace"), "{rendered}");
        assert!(
            rendered.contains("model routes: draft:fast@1"),
            "{rendered}"
        );
    }
}
