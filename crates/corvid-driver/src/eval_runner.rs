use crate::{
    compile_to_ir_with_config_at_path, load_corvid_config_for, render_all_pretty, Diagnostic,
};
use corvid_ir::{IrFile, IrTest};
use corvid_runtime::Runtime;
use corvid_vm::{
    run_all_tests_with_options, SnapshotOptions, TestAssertionStatus, TestExecution,
    TestRunOptions, TraceFixtureOptions,
};
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum EvalRunnerError {
    Io { path: PathBuf, error: std::io::Error },
}

impl fmt::Display for EvalRunnerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => {
                write!(f, "failed to access `{}`: {error}", path.display())
            }
        }
    }
}

impl std::error::Error for EvalRunnerError {}

#[derive(Debug, Clone)]
pub struct CorvidEvalReport {
    pub source_path: PathBuf,
    pub compile_diagnostics: Vec<Diagnostic>,
    pub evals: Vec<TestExecution>,
    pub html_report_path: PathBuf,
}

impl CorvidEvalReport {
    pub fn passed(&self) -> bool {
        self.compile_diagnostics.is_empty()
            && !self.evals.is_empty()
            && self.evals.iter().all(TestExecution::passed)
    }

    pub fn exit_code(&self) -> u8 {
        if self.passed() { 0 } else { 1 }
    }
}

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
    let config = load_corvid_config_for(path);
    let ir = match compile_to_ir_with_config_at_path(&source, path, config.as_ref()) {
        Ok(ir) => ir,
        Err(diagnostics) => {
            let report = CorvidEvalReport {
                source_path: path.to_path_buf(),
                compile_diagnostics: diagnostics,
                evals: Vec::new(),
                html_report_path,
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
    let report = CorvidEvalReport {
        source_path: path.to_path_buf(),
        compile_diagnostics: Vec::new(),
        evals,
        html_report_path,
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
            root: base.join("target").join("eval").join(sanitize_path_segment(stem)),
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
                out.push_str(&format!(
                    "    {:?}: {}",
                    assertion.status, assertion.label
                ));
                if let Some(message) = &assertion.message {
                    out.push_str(&format!(" - {message}"));
                }
                out.push('\n');
            }
        }
    }
    out.push_str(&format!("\n{passed} passed, {failed} failed\n"));
    out.push_str(&format!(
        "HTML report: {}\n",
        report.html_report_path.display()
    ));
    out
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
    out.push_str("<table><thead><tr><th>Eval</th><th>Status</th><th>Assertions</th></tr></thead><tbody>");
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

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_runtime::Runtime;

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
}
