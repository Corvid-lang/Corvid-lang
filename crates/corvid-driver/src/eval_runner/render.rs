use super::CorvidEvalReport;
use crate::render_all_pretty;
use corvid_vm::TestAssertionStatus;

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

pub(super) fn write_eval_html_report(report: &CorvidEvalReport) -> std::io::Result<()> {
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
