//! Pipeline orchestration: parse → resolve → typecheck → lower → codegen.
//!
//! Driver is the CLI's library. The `corvid` binary thinly wraps these
//! functions. Kept small so it's easy to embed elsewhere (IDE, LSP, tests).
//!
//! See `ARCHITECTURE.md` §4.

#![allow(dead_code)]

mod render;

pub use render::{render_all_pretty, render_pretty};

use std::fmt;
use std::path::{Path, PathBuf};

use corvid_ast::Span;
use corvid_codegen_py::emit;
use corvid_ir::lower;
use corvid_resolve::{resolve, ResolveError};
use corvid_syntax::{lex, parse_file, LexError, ParseError};
use corvid_types::{typecheck, TypeError};

/// A unified diagnostic from any compiler phase, with a span that can be
/// rendered against the original source.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub span: Span,
    pub message: String,
    pub hint: Option<String>,
}

impl Diagnostic {
    pub fn render(&self, source_path: &Path, source: &str) -> String {
        let (line, col) = line_col_of(source, self.span.start);
        let mut out = format!(
            "{}:{}:{}: error: {}",
            source_path.display(),
            line,
            col,
            self.message
        );
        if let Some(h) = &self.hint {
            out.push_str("\n  help: ");
            out.push_str(h);
        }
        out
    }
}

impl From<LexError> for Diagnostic {
    fn from(e: LexError) -> Self {
        Diagnostic {
            span: e.span,
            message: e.kind.to_string(),
            hint: None,
        }
    }
}

impl From<ParseError> for Diagnostic {
    fn from(e: ParseError) -> Self {
        Diagnostic {
            span: e.span,
            message: e.kind.to_string(),
            hint: None,
        }
    }
}

impl From<ResolveError> for Diagnostic {
    fn from(e: ResolveError) -> Self {
        Diagnostic {
            span: e.span,
            message: e.kind.to_string(),
            hint: None,
        }
    }
}

impl From<TypeError> for Diagnostic {
    fn from(e: TypeError) -> Self {
        let hint = e.hint();
        let message = e.message();
        Diagnostic {
            span: e.span,
            message,
            hint,
        }
    }
}

/// Convert a byte offset into 1-based (line, column) coordinates.
///
/// Columns count Unicode characters, not bytes. Lines split on `\n`.
fn line_col_of(source: &str, offset: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut col = 1usize;
    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Outcome of a compile. Always contains the Python source (even partial)
/// when possible, and any diagnostics found.
pub struct CompileResult {
    pub python_source: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
}

impl CompileResult {
    pub fn ok(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

/// Run the full frontend on `source`. Stops collecting output when errors
/// before codegen would make it misleading.
pub fn compile(source: &str) -> CompileResult {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();

    // 1. Lex
    let tokens = match lex(source) {
        Ok(t) => t,
        Err(errs) => {
            diagnostics.extend(errs.into_iter().map(Diagnostic::from));
            return CompileResult {
                python_source: None,
                diagnostics,
            };
        }
    };

    // 2. Parse (collects errors, may still produce a partial AST)
    let (file, parse_errs) = parse_file(&tokens);
    diagnostics.extend(parse_errs.into_iter().map(Diagnostic::from));

    // 3. Resolve (collects errors)
    let resolved = resolve(&file);
    diagnostics.extend(
        resolved
            .errors
            .iter()
            .cloned()
            .map(Diagnostic::from),
    );

    // 4. Typecheck (collects errors — this is where the killer feature lives)
    let checked = typecheck(&file, &resolved);
    diagnostics.extend(
        checked
            .errors
            .iter()
            .cloned()
            .map(Diagnostic::from),
    );

    if !diagnostics.is_empty() {
        return CompileResult {
            python_source: None,
            diagnostics,
        };
    }

    // 5. Lower + 6. Codegen. Only when everything before is clean.
    let ir = lower(&file, &resolved, &checked);
    let py = emit(&ir);

    CompileResult {
        python_source: Some(py),
        diagnostics: Vec::new(),
    }
}

/// Compile `source_path` and write the generated Python to disk.
///
/// Layout convention:
///   * If the source is inside a `src/` directory, output goes to a sibling
///     `target/py/<stem>.py` relative to that `src/`.
///   * Otherwise, output goes alongside the source in `./target/py/<stem>.py`.
pub fn build_to_disk(source_path: &Path) -> anyhow::Result<BuildOutput> {
    let source = std::fs::read_to_string(source_path).map_err(|e| {
        anyhow::anyhow!("cannot read `{}`: {}", source_path.display(), e)
    })?;

    let result = compile(&source);

    if !result.ok() {
        return Ok(BuildOutput {
            source,
            output_path: None,
            diagnostics: result.diagnostics,
        });
    }

    let out_path = output_path_for(source_path);
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let py = result.python_source.expect("codegen produced no source");
    std::fs::write(&out_path, &py)?;

    Ok(BuildOutput {
        source,
        output_path: Some(out_path),
        diagnostics: Vec::new(),
    })
}

pub struct BuildOutput {
    pub source: String,
    pub output_path: Option<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
}

fn output_path_for(source_path: &Path) -> PathBuf {
    let stem = source_path.file_stem().unwrap_or_default();
    let py_name = format!("{}.py", stem.to_string_lossy());

    // Find the nearest enclosing `src` directory by walking up.
    let mut ancestor: Option<&Path> = source_path.parent();
    while let Some(dir) = ancestor {
        if dir.file_name().map(|n| n == "src").unwrap_or(false) {
            if let Some(project_root) = dir.parent() {
                return project_root.join("target").join("py").join(py_name);
            }
        }
        ancestor = dir.parent();
    }

    // Default: alongside the source, in a `target/py/` subdir.
    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    parent.join("target").join("py").join(py_name)
}

/// Scaffold a new Corvid project at `<name>/` under the current directory.
pub fn scaffold_new(name: &str) -> anyhow::Result<PathBuf> {
    scaffold_new_in(&std::env::current_dir()?, name)
}

/// Scaffold a new Corvid project named `<name>` under `parent`.
pub fn scaffold_new_in(parent: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let root = parent.join(name);
    if root.exists() {
        anyhow::bail!("directory `{}` already exists", root.display());
    }
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::write(
        root.join("corvid.toml"),
        format!(
            r#"name = "{name}"
version = "0.1.0"

[llm]
# No default model is set. Pick one explicitly:
#   default_model = "claude-opus-4-6"
"#
        ),
    )?;
    std::fs::write(
        root.join(".gitignore"),
        "/target\n__pycache__/\n*.pyc\n",
    )?;
    std::fs::write(
        root.join("src").join("main.cor"),
        r#"# Your first Corvid agent.

tool echo(message: String) -> String

agent greet(name: String) -> String:
    message = echo(name)
    return message
"#,
    )?;
    std::fs::write(
        root.join("tools.py"),
        r#"# Implement your Corvid tools here.
from corvid_runtime import tool


@tool("echo")
async def echo(message: str) -> str:
    return message
"#,
    )?;
    Ok(root)
}

// ------------------------------------------------------------
// Summary printer for CLI use.
// ------------------------------------------------------------

pub fn summarize_diagnostics(
    diags: &[Diagnostic],
    source_path: &Path,
    source: &str,
) -> String {
    let mut out = String::new();
    for d in diags {
        out.push_str(&d.render(source_path, source));
        out.push('\n');
    }
    out.push_str(&format!("\n{} error(s) found.\n", diags.len()));
    out
}

// ------------------------------------------------------------
// fmt helpers for consumer displays.
// ------------------------------------------------------------

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}..{}] {}", self.span.start, self.span.end, self.message)?;
        if let Some(h) = &self.hint {
            write!(f, "\n  help: {h}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK_SRC: &str = r#"
tool get_order(id: String) -> Order
type Order:
    id: String

agent fetch(id: String) -> Order:
    return get_order(id)
"#;

    const BAD_EFFECT_SRC: &str = r#"
tool issue_refund(id: String, amount: Float) -> Receipt dangerous
type Receipt:
    id: String

agent bad(id: String, amount: Float) -> Receipt:
    return issue_refund(id, amount)
"#;

    #[test]
    fn clean_source_produces_python() {
        let r = compile(OK_SRC);
        assert!(r.diagnostics.is_empty(), "diagnostics: {:?}", r.diagnostics);
        assert!(r.python_source.is_some());
        let py = r.python_source.unwrap();
        assert!(py.contains("async def fetch(id):"));
    }

    #[test]
    fn missing_approve_surfaces_as_diagnostic() {
        let r = compile(BAD_EFFECT_SRC);
        assert!(r.python_source.is_none());
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.message.contains("dangerous") && d.message.contains("issue_refund")),
            "diagnostics: {:?}",
            r.diagnostics
        );
        let hint = r
            .diagnostics
            .iter()
            .find_map(|d| d.hint.clone())
            .expect("expected a hint for the UnapprovedDangerousCall");
        assert!(hint.contains("approve IssueRefund"), "hint was: {hint}");
    }

    #[test]
    fn build_to_disk_writes_file() {
        let tmp = tempfile::tempdir().unwrap();
        let src_path = tmp.path().join("hello.cor");
        std::fs::write(&src_path, OK_SRC).unwrap();

        let out = build_to_disk(&src_path).unwrap();
        let path = out.output_path.expect("expected output path");
        assert!(path.exists(), "expected {} to exist", path.display());
        let py = std::fs::read_to_string(&path).unwrap();
        assert!(py.contains("async def fetch"));
    }

    #[test]
    fn build_to_disk_with_src_dir_places_output_in_sibling_target() {
        let tmp = tempfile::tempdir().unwrap();
        let src_dir = tmp.path().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        let src_path = src_dir.join("main.cor");
        std::fs::write(&src_path, OK_SRC).unwrap();

        let out = build_to_disk(&src_path).unwrap();
        let path = out.output_path.expect("expected output path");
        let expected = tmp.path().join("target").join("py").join("main.py");
        assert_eq!(path, expected);
    }

    #[test]
    fn build_emits_no_file_when_diagnostics_present() {
        let tmp = tempfile::tempdir().unwrap();
        let src_path = tmp.path().join("bad.cor");
        std::fs::write(&src_path, BAD_EFFECT_SRC).unwrap();

        let out = build_to_disk(&src_path).unwrap();
        assert!(out.output_path.is_none());
        assert!(!out.diagnostics.is_empty());
    }

    #[test]
    fn scaffold_new_creates_expected_structure() {
        let tmp = tempfile::tempdir().unwrap();
        let root = scaffold_new_in(tmp.path(), "my_bot").unwrap();
        assert!(root.join("corvid.toml").exists());
        assert!(root.join("src/main.cor").exists());
        assert!(root.join("tools.py").exists());
        assert!(root.join(".gitignore").exists());
    }

    #[test]
    fn scaffold_rejects_existing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("already_there")).unwrap();
        let err = scaffold_new_in(tmp.path(), "already_there").unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn line_col_translation() {
        let src = "abc\ndef\nghi";
        assert_eq!(line_col_of(src, 0), (1, 1));
        assert_eq!(line_col_of(src, 2), (1, 3));
        assert_eq!(line_col_of(src, 4), (2, 1));
        assert_eq!(line_col_of(src, 8), (3, 1));
    }
}
