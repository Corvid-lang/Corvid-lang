//! Pipeline orchestration: parse → resolve → typecheck → lower → codegen.
//!
//! Driver is the CLI's library. The `corvid` binary thinly wraps these
//! functions. Kept small so it's easy to embed elsewhere (IDE, LSP, tests).
//!
//! See `ARCHITECTURE.md` §4.

#![allow(dead_code)]

mod render;

pub use render::{render_all_pretty, render_pretty};

// Re-export the runtime + interpreter surface so consumers (CLI, demo
// runner binaries, embedding hosts) only need to depend on the driver.
pub use corvid_runtime::{
    fresh_run_id, load_dotenv_walking, AnthropicAdapter, ApprovalDecision, ApprovalRequest,
    Approver, MockAdapter, OpenAiAdapter, ProgrammaticApprover, RedactionSet, Runtime,
    RuntimeBuilder, RuntimeError, StdinApprover, Tracer,
};
pub use corvid_vm::{build_struct, InterpError, InterpErrorKind, StructValue, Value};

use std::fmt;
use std::path::{Path, PathBuf};

use corvid_ast::Span;
use corvid_codegen_py::emit;
use corvid_ir::{lower, IrFile};
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

/// Compile `source_path` to a native binary under `<project>/target/bin/`.
///
/// Layout convention mirrors `build_to_disk`: if the source is inside a
/// `src/` directory, output goes to a sibling `target/bin/<stem>[.exe]`.
/// Otherwise, output goes alongside the source in `./target/bin/`.
pub fn build_native_to_disk(source_path: &Path) -> anyhow::Result<NativeBuildOutput> {
    let source = std::fs::read_to_string(source_path).map_err(|e| {
        anyhow::anyhow!("cannot read `{}`: {}", source_path.display(), e)
    })?;

    match compile_to_ir(&source) {
        Err(diagnostics) => Ok(NativeBuildOutput {
            source,
            output_path: None,
            diagnostics,
        }),
        Ok(ir) => {
            let bin_dir = native_output_dir_for(source_path);
            let stem = source_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("program")
                .to_string();
            let requested = bin_dir.join(&stem);
            let produced = corvid_codegen_cl::build_native_to_disk(&ir, &stem, &requested)
                .map_err(|e| anyhow::anyhow!("native codegen failed: {e}"))?;
            Ok(NativeBuildOutput {
                source,
                output_path: Some(produced),
                diagnostics: Vec::new(),
            })
        }
    }
}

pub struct NativeBuildOutput {
    pub source: String,
    pub output_path: Option<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
}

fn native_output_dir_for(source_path: &Path) -> PathBuf {
    let mut ancestor: Option<&Path> = source_path.parent();
    while let Some(dir) = ancestor {
        if dir.file_name().map(|n| n == "src").unwrap_or(false) {
            if let Some(project_root) = dir.parent() {
                return project_root.join("target").join("bin");
            }
        }
        ancestor = dir.parent();
    }
    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    parent.join("target").join("bin")
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
// Native run: compile + interpret with a Runtime.
// ------------------------------------------------------------

/// Errors `run_with_runtime` and friends can produce.
#[derive(Debug)]
pub enum RunError {
    /// IO failed reading the source file.
    Io { path: PathBuf, error: std::io::Error },
    /// Frontend produced diagnostics; nothing to run.
    Compile(Vec<Diagnostic>),
    /// The IR contains no agents.
    NoAgents,
    /// The caller didn't pick an agent and there are several to choose from.
    AmbiguousAgent { available: Vec<String> },
    /// The named agent doesn't exist in the IR.
    UnknownAgent { name: String, available: Vec<String> },
    /// The caller asked for default args but the agent expects parameters.
    NeedsArgs { agent: String, expected: usize },
    /// The interpreter aborted.
    Interp(InterpError),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "cannot read `{}`: {}", path.display(), error),
            Self::Compile(d) => write!(f, "{} compile error(s)", d.len()),
            Self::NoAgents => write!(f, "no agents declared in this file"),
            Self::AmbiguousAgent { available } => write!(
                f,
                "multiple agents declared; pick one with --agent. available: {}",
                available.join(", ")
            ),
            Self::UnknownAgent { name, available } => write!(
                f,
                "no agent named `{name}`. available: {}",
                available.join(", ")
            ),
            Self::NeedsArgs { agent, expected } => write!(
                f,
                "agent `{agent}` expects {expected} argument(s); `corvid run` cannot supply them yet — use a runner binary that calls `run_with_runtime` with arguments"
            ),
            Self::Interp(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RunError {}

/// Compile a source string to IR. Returns the IR or the full diagnostic list.
pub fn compile_to_ir(source: &str) -> Result<IrFile, Vec<Diagnostic>> {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let tokens = match lex(source) {
        Ok(t) => t,
        Err(errs) => {
            diagnostics.extend(errs.into_iter().map(Diagnostic::from));
            return Err(diagnostics);
        }
    };
    let (file, parse_errs) = parse_file(&tokens);
    diagnostics.extend(parse_errs.into_iter().map(Diagnostic::from));
    let resolved = resolve(&file);
    diagnostics.extend(resolved.errors.iter().cloned().map(Diagnostic::from));
    let checked = typecheck(&file, &resolved);
    diagnostics.extend(checked.errors.iter().cloned().map(Diagnostic::from));
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    Ok(lower(&file, &resolved, &checked))
}

/// Compile a `.cor` file and run the chosen agent against `runtime`.
///
/// `agent` selects which agent to invoke. Pass `None` to run the file's
/// only agent (errors if there's more than one). `args` are passed as
/// the agent's parameters; pass an empty vec for parameter-less agents.
pub async fn run_with_runtime(
    path: &Path,
    agent: Option<&str>,
    args: Vec<Value>,
    runtime: &Runtime,
) -> Result<Value, RunError> {
    let source = std::fs::read_to_string(path).map_err(|e| RunError::Io {
        path: path.to_path_buf(),
        error: e,
    })?;
    let ir = compile_to_ir(&source).map_err(RunError::Compile)?;
    run_ir_with_runtime(&ir, agent, args, runtime).await
}

/// Like `run_with_runtime`, but takes already-lowered IR. Useful for
/// embedding hosts that compile once and run many times.
pub async fn run_ir_with_runtime(
    ir: &IrFile,
    agent: Option<&str>,
    args: Vec<Value>,
    runtime: &Runtime,
) -> Result<Value, RunError> {
    if ir.agents.is_empty() {
        return Err(RunError::NoAgents);
    }
    let chosen_name = match agent {
        Some(name) => {
            if !ir.agents.iter().any(|a| a.name == name) {
                return Err(RunError::UnknownAgent {
                    name: name.to_string(),
                    available: ir.agents.iter().map(|a| a.name.clone()).collect(),
                });
            }
            name.to_string()
        }
        None => {
            if ir.agents.len() == 1 {
                ir.agents[0].name.clone()
            } else {
                // Prefer an agent named `main` if one exists.
                if let Some(main) = ir.agents.iter().find(|a| a.name == "main") {
                    main.name.clone()
                } else {
                    return Err(RunError::AmbiguousAgent {
                        available: ir.agents.iter().map(|a| a.name.clone()).collect(),
                    });
                }
            }
        }
    };
    let chosen = ir
        .agents
        .iter()
        .find(|a| a.name == chosen_name)
        .expect("agent presence checked above");
    if args.is_empty() && !chosen.params.is_empty() {
        return Err(RunError::NeedsArgs {
            agent: chosen.name.clone(),
            expected: chosen.params.len(),
        });
    }
    corvid_vm::run_agent(ir, &chosen.name, args, runtime)
        .await
        .map_err(RunError::Interp)
}

/// `corvid run <file>` entry point. Loads `.env` (walking from the
/// source's project root), opens a default runtime with stdin approver,
/// JSONL trace under `<project>/target/trace/`, and any LLM adapter the
/// environment signals: Anthropic when `ANTHROPIC_API_KEY` is set,
/// OpenAI when `OPENAI_API_KEY` is set. Trace events get the
/// `RedactionSet::from_env()` applied so secrets stay out of the file.
///
/// Tool-using agents still need a runner binary — `corvid run` cannot
/// load user tool implementations yet (see Phase 14). Prompt-only
/// agents work end-to-end as long as the matching API key is present.
pub fn run_native(path: &Path) -> Result<u8, anyhow::Error> {
    // Load `.env` from the source file's neighborhood. Real env vars
    // win, so this only fills in unset values.
    if let Some(parent) = path.parent() {
        let _ = load_dotenv_walking(parent);
    }
    // Also try the cwd in case the source path is bare.
    let _ = load_dotenv_walking(&std::env::current_dir().unwrap_or_else(|_| Path::new(".").into()));

    let trace_dir = trace_dir_for(path);
    let tracer = Tracer::open(&trace_dir, corvid_runtime::fresh_run_id())
        .with_redaction(RedactionSet::from_env());

    let mut builder = Runtime::builder()
        .approver(std::sync::Arc::new(StdinApprover::new()))
        .tracer(tracer);

    if let Ok(model) = std::env::var("CORVID_MODEL") {
        builder = builder.default_model(&model);
    }
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        builder = builder.llm(std::sync::Arc::new(AnthropicAdapter::new(key)));
    }
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        builder = builder.llm(std::sync::Arc::new(OpenAiAdapter::new(key)));
    }
    let rt = builder.build();

    let tokio_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let result = tokio_rt.block_on(run_with_runtime(path, None, vec![], &rt));

    match result {
        Ok(value) => {
            println!("{value}");
            Ok(0)
        }
        Err(RunError::Compile(diags)) => {
            let source = std::fs::read_to_string(path).unwrap_or_default();
            eprint!("{}", render_all_pretty(&diags, path, &source));
            Ok(1)
        }
        Err(other) => {
            eprintln!("error: {other}");
            Ok(1)
        }
    }
}

/// Pick a trace directory next to the source file's project root.
fn trace_dir_for(source_path: &Path) -> PathBuf {
    let mut ancestor: Option<&Path> = source_path.parent();
    while let Some(dir) = ancestor {
        if dir.file_name().map(|n| n == "src").unwrap_or(false) {
            if let Some(project_root) = dir.parent() {
                return project_root.join("target").join("trace");
            }
        }
        ancestor = dir.parent();
    }
    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    parent.join("target").join("trace")
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

    // ----------------------------------------------------------
    // Native run integration: drive the compiler + runtime end-to-end
    // ----------------------------------------------------------

    use serde_json::json;
    use std::sync::Arc;

    const REFUND_BOT_SRC: &str = r#"
type Ticket:
    order_id: String
    reason: String

type Order:
    id: String
    amount: Float

type Decision:
    should_refund: Bool

type Receipt:
    refund_id: String

tool get_order(id: String) -> Order
tool issue_refund(id: String, amount: Float) -> Receipt dangerous

prompt decide_refund(ticket: Ticket, order: Order) -> Decision:
    """Decide whether to refund: ticket={ticket} order={order}."""

agent refund_bot(ticket: Ticket) -> Decision:
    order = get_order(ticket.order_id)
    decision = decide_refund(ticket, order)
    if decision.should_refund:
        approve IssueRefund(order.id, order.amount)
        issue_refund(order.id, order.amount)
    return decision
"#;

    fn refund_bot_runtime(trace_dir: &Path) -> Runtime {
        Runtime::builder()
            .tool("get_order", |args| async move {
                let id = args[0].as_str().unwrap_or("");
                Ok(json!({ "id": id, "amount": 49.99 }))
            })
            .tool("issue_refund", |args| async move {
                let id = args[0].as_str().unwrap_or("");
                Ok(json!({ "refund_id": format!("rf_{id}") }))
            })
            .approver(Arc::new(ProgrammaticApprover::always_yes()))
            .llm(Arc::new(
                MockAdapter::new("mock-1")
                    .reply("decide_refund", json!({ "should_refund": true })),
            ))
            .default_model("mock-1")
            .trace_to(trace_dir)
            .build()
    }

    #[tokio::test]
    async fn refund_bot_runs_end_to_end_via_driver() {
        let tmp = tempfile::tempdir().unwrap();
        let src_path = tmp.path().join("refund_bot.cor");
        std::fs::write(&src_path, REFUND_BOT_SRC).unwrap();
        let trace_dir = tmp.path().join("trace");

        let rt = refund_bot_runtime(&trace_dir);

        // Build a Ticket struct as the agent's input.
        let ir = compile_to_ir(REFUND_BOT_SRC).expect("clean compile");
        let ticket_id = ir.types.iter().find(|t| t.name == "Ticket").unwrap().id;
        let ticket = corvid_vm::build_struct(
            ticket_id,
            "Ticket",
            [
                ("order_id".to_string(), Value::String(Arc::from("ord_42"))),
                ("reason".to_string(), Value::String(Arc::from("damaged"))),
            ],
        );

        let v = run_with_runtime(&src_path, Some("refund_bot"), vec![ticket], &rt)
            .await
            .expect("run");

        match v {
            Value::Struct(s) => {
                assert_eq!(s.type_name, "Decision");
                assert_eq!(s.fields.get("should_refund").unwrap(), &Value::Bool(true));
            }
            other => panic!("expected Decision struct, got {other:?}"),
        }

        // A trace file should have been written.
        let traces: Vec<_> = std::fs::read_dir(&trace_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .map(|x| x == "jsonl")
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(traces.len(), 1, "expected exactly one .jsonl trace file");
        let body = std::fs::read_to_string(traces[0].path()).unwrap();
        assert!(body.contains("\"kind\":\"run_started\""));
        assert!(body.contains("\"kind\":\"tool_call\""));
        assert!(body.contains("\"kind\":\"approval_response\""));
        assert!(body.contains("\"approved\":true"));
        assert!(body.contains("\"kind\":\"run_completed\""));
    }

    #[tokio::test]
    async fn run_errors_when_no_agent_selected_among_many() {
        let src = "agent a() -> Int:\n    return 1\nagent b() -> Int:\n    return 2\n";
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("two.cor");
        std::fs::write(&path, src).unwrap();
        let rt = Runtime::builder().build();
        let err = run_with_runtime(&path, None, vec![], &rt).await.unwrap_err();
        assert!(matches!(err, RunError::AmbiguousAgent { .. }));
    }

    #[tokio::test]
    async fn run_picks_main_when_present() {
        let src = "agent helper() -> Int:\n    return 1\nagent main() -> Int:\n    return 99\n";
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("main.cor");
        std::fs::write(&path, src).unwrap();
        let rt = Runtime::builder().build();
        let v = run_with_runtime(&path, None, vec![], &rt).await.unwrap();
        assert_eq!(v, Value::Int(99));
    }

    #[tokio::test]
    async fn run_rejects_agent_needing_args_with_clear_error() {
        let src = "agent needs(n: Int) -> Int:\n    return n + 1\n";
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("needs.cor");
        std::fs::write(&path, src).unwrap();
        let rt = Runtime::builder().build();
        let err = run_with_runtime(&path, None, vec![], &rt).await.unwrap_err();
        match err {
            RunError::NeedsArgs { agent, expected } => {
                assert_eq!(agent, "needs");
                assert_eq!(expected, 1);
            }
            other => panic!("expected NeedsArgs, got {other:?}"),
        }
    }
}
