use crate::{
    compile_to_ir_with_config_at_path, load_corvid_config_for, render_all_pretty, Diagnostic,
};
use corvid_runtime::Runtime;
use corvid_vm::{run_all_tests, TestAssertionStatus, TestExecution};
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum TestRunnerError {
    Io { path: PathBuf, error: std::io::Error },
}

impl fmt::Display for TestRunnerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => {
                write!(f, "failed to read `{}`: {error}", path.display())
            }
        }
    }
}

impl std::error::Error for TestRunnerError {}

#[derive(Debug, Clone)]
pub struct CorvidTestReport {
    pub source_path: PathBuf,
    pub compile_diagnostics: Vec<Diagnostic>,
    pub tests: Vec<TestExecution>,
}

impl CorvidTestReport {
    pub fn passed(&self) -> bool {
        self.compile_diagnostics.is_empty()
            && !self.tests.is_empty()
            && self.tests.iter().all(TestExecution::passed)
    }

    pub fn exit_code(&self) -> u8 {
        if self.passed() {
            0
        } else {
            1
        }
    }
}

pub async fn run_tests_at_path(
    path: &Path,
    runtime: &Runtime,
) -> Result<CorvidTestReport, TestRunnerError> {
    let source = std::fs::read_to_string(path).map_err(|error| TestRunnerError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    let config = load_corvid_config_for(path);
    let ir = match compile_to_ir_with_config_at_path(&source, path, config.as_ref()) {
        Ok(ir) => ir,
        Err(diagnostics) => {
            return Ok(CorvidTestReport {
                source_path: path.to_path_buf(),
                compile_diagnostics: diagnostics,
                tests: Vec::new(),
            });
        }
    };
    Ok(CorvidTestReport {
        source_path: path.to_path_buf(),
        compile_diagnostics: Vec::new(),
        tests: run_all_tests(&ir, runtime).await,
    })
}

pub fn render_test_report(report: &CorvidTestReport, source: Option<&str>) -> String {
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
        return out;
    }

    out.push_str(&format!("corvid test {}\n", report.source_path.display()));
    if report.tests.is_empty() {
        out.push_str("  no test declarations found\n");
        out.push_str("\n0 passed, 0 failed\n");
        return out;
    }

    let mut passed = 0_usize;
    let mut failed = 0_usize;
    for test in &report.tests {
        if test.passed() {
            passed += 1;
            out.push_str(&format!("  PASS {}\n", test.name));
        } else {
            failed += 1;
            out.push_str(&format!("  FAIL {}\n", test.name));
            if let Some(error) = &test.setup_error {
                out.push_str(&format!("    setup error: {error}\n"));
            }
            for assertion in &test.assertions {
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
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use corvid_runtime::{ProgrammaticApprover, Runtime};
    use std::sync::Arc;

    fn runtime() -> Runtime {
        Runtime::builder()
            .approver(Arc::new(ProgrammaticApprover::always_yes()))
            .build()
    }

    #[tokio::test]
    async fn run_tests_at_path_reports_pass_and_fail() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("suite.cor");
        std::fs::write(
            &path,
            r#"
test math_pass:
    x = 20 + 22
    assert x == 42

test math_fail:
    x = 1
    assert x == 2
"#,
        )
        .expect("write");

        let report = run_tests_at_path(&path, &runtime()).await.expect("run");
        assert_eq!(report.tests.len(), 2);
        assert!(report.tests[0].passed());
        assert!(!report.tests[1].passed());
        assert_eq!(report.exit_code(), 1);
    }

    #[tokio::test]
    async fn run_tests_at_path_fails_when_no_tests_exist() {
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

        let report = run_tests_at_path(&path, &runtime()).await.expect("run");
        assert!(report.tests.is_empty());
        assert_eq!(report.exit_code(), 1);
    }

    #[tokio::test]
    async fn run_tests_at_path_uses_fixture_and_mock_declarations() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("mocked.cor");
        std::fs::write(
            &path,
            r#"
tool lookup_score(id: String) -> Int

fixture order_id() -> String:
    return "ord_42"

mock lookup_score(id: String) -> Int:
    if id == "ord_42":
        return 42
    return 0

test mocked_tool_contract:
    score = lookup_score(order_id())
    assert score == 42
"#,
        )
        .expect("write");

        let report = run_tests_at_path(&path, &runtime()).await.expect("run");
        assert_eq!(report.tests.len(), 1);
        assert!(report.tests[0].passed(), "report: {report:?}");
        assert_eq!(report.exit_code(), 0);
    }
}
