//! Pipeline orchestration: parse → resolve → typecheck → lower → codegen.
//!
//! Driver is the CLI's library. The `corvid` binary thinly wraps these
//! functions. Kept small so it's easy to embed elsewhere (IDE, LSP, tests).
//!
//! See `ARCHITECTURE.md` §4.

#![allow(dead_code)]

pub mod add_dimension;
pub mod effect_diff;
pub mod meta_verify;
mod native_ability;
mod native_cache;
mod render;
pub mod spec_check;

pub use add_dimension::{add_dimension as install_dimension, AddDimensionOutcome};
pub use effect_diff::{
    diff_snapshots, render_effect_diff, snapshot_revision, AgentDiff, AgentSnapshot,
    DimensionChange, EffectDiff, RevisionSnapshot,
};
pub use meta_verify::{
    render_meta_report, verify_counterexample_corpus, Counterexample, MetaKind, MetaVerdict, CORPUS,
};
pub use native_ability::{native_ability, NotNativeReason};
pub use render::{render_all_pretty, render_pretty};
pub use spec_check::{
    extract_spec_examples, render_spec_report, verify_spec_examples, Expectation, SpecExample,
    SpecVerdict, VerdictKind,
};

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

use corvid_ast::{CompositionRule as AstCompositionRule, DimensionSchema as AstDimensionSchema, DimensionValue as AstDimensionValue, Span};
use corvid_codegen_py::emit;
use corvid_ir::{lower, IrFile};
use corvid_resolve::{resolve, ResolveError};
use corvid_syntax::{lex, parse_file, LexError, ParseError};
pub use corvid_types::{Verdict as LawVerdict, DEFAULT_SAMPLES};
use corvid_types::{
    check_dimension, typecheck_with_config, CorvidConfig, DimensionUnderTest, LawCheckResult,
    TypeError, Verdict,
};

/// A unified diagnostic from any compiler stage, with a span that can be
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
    compile_with_config(source, None)
}

/// Run the archetype law-check suite against every built-in dimension
/// and every custom dimension declared in `corvid.toml`.
///
/// For each dimension, proptests the claimed composition archetype's
/// algebraic laws (associativity, commutativity, identity, plus
/// idempotence + monotonicity for semilattices) with `samples` cases
/// per law. Pass `DEFAULT_SAMPLES` (10,000) for the production run.
///
/// Returns one `LawCheckResult` per (dimension × law) pair. A single
/// counter-example short-circuits that law's check; the remaining
/// laws still run so users see the complete verdict in one report.
pub fn run_law_checks(
    config: Option<&CorvidConfig>,
    samples: usize,
) -> Vec<LawCheckResult> {
    let mut results = Vec::new();
    for dim in builtin_dimensions_under_test() {
        results.extend(check_dimension(&dim, samples));
    }
    if let Some(cfg) = config {
        if let Ok(schemas) = cfg.into_dimension_schemas() {
            for (schema, meta) in schemas {
                let dim = DimensionUnderTest::from_custom(schema, &meta);
                results.extend(check_dimension(&dim, samples));
            }
        }
    }
    results
}

/// Render a law-check report as human-readable text.
pub fn render_law_check_report(results: &[LawCheckResult]) -> String {
    use std::collections::BTreeMap;
    let mut by_dim: BTreeMap<String, Vec<&LawCheckResult>> = BTreeMap::new();
    for r in results {
        by_dim.entry(r.dimension.clone()).or_default().push(r);
    }
    let mut out = String::new();
    for (name, entries) in &by_dim {
        let rule = entries.first().map(|r| r.rule).unwrap_or(AstCompositionRule::Sum);
        out.push_str(&format!("\n  {name} ({rule:?})\n"));
        for r in entries {
            let status = match &r.verdict {
                Verdict::Pass => format!("ok ({} cases)", r.samples),
                Verdict::NotApplicable { reason } => format!("n/a — {reason}"),
                Verdict::CounterExample { note, .. } => format!("FAIL — {note}"),
            };
            out.push_str(&format!(
                "    {:<16} {status}\n",
                r.law.as_str(),
            ));
        }
    }
    let failures = results
        .iter()
        .filter(|r| matches!(r.verdict, Verdict::CounterExample { .. }))
        .count();
    if failures > 0 {
        out.push_str(&format!(
            "\n{failures} dimension(s) failed a law. See counter-examples above.\n"
        ));
    } else {
        out.push_str(&format!(
            "\nall {} dimensions satisfy their archetype's laws.\n",
            by_dim.len()
        ));
    }
    out
}

fn builtin_dimensions_under_test() -> Vec<DimensionUnderTest> {
    vec![
        DimensionUnderTest::from_schema(AstDimensionSchema {
            name: "cost".into(),
            composition: AstCompositionRule::Sum,
            default: AstDimensionValue::Cost(0.0),
        }),
        DimensionUnderTest::from_schema(AstDimensionSchema {
            name: "tokens".into(),
            composition: AstCompositionRule::Sum,
            default: AstDimensionValue::Number(0.0),
        }),
        DimensionUnderTest::from_schema(AstDimensionSchema {
            name: "latency_ms".into(),
            composition: AstCompositionRule::Sum,
            default: AstDimensionValue::Number(0.0),
        }),
        DimensionUnderTest::from_schema(AstDimensionSchema {
            name: "trust".into(),
            composition: AstCompositionRule::Max,
            default: AstDimensionValue::Name("autonomous".into()),
        }),
        DimensionUnderTest::from_schema(AstDimensionSchema {
            name: "confidence".into(),
            composition: AstCompositionRule::Min,
            default: AstDimensionValue::Number(f64::INFINITY),
        }),
        DimensionUnderTest::from_schema(AstDimensionSchema {
            name: "data".into(),
            composition: AstCompositionRule::Union,
            default: AstDimensionValue::Name("none".into()),
        }),
        DimensionUnderTest::from_schema(AstDimensionSchema {
            name: "reversible".into(),
            composition: AstCompositionRule::LeastReversible,
            default: AstDimensionValue::Bool(true),
        }),
    ]
}

/// Walk upward from `source_path.parent()` looking for `corvid.toml`.
/// Returns `None` when no file is found or when parsing fails — a
/// malformed file doesn't crash the compile; instead it surfaces
/// through `typecheck_with_config` as an `InvalidCustomDimension`
/// diagnostic at the source file's top span.
pub fn load_corvid_config_for(source_path: &Path) -> Option<CorvidConfig> {
    let start = source_path.parent()?;
    CorvidConfig::load_walking(start).ok().flatten()
}

/// Compile with an explicit `corvid.toml` configuration (for user-defined
/// effect dimensions). Callers with a source-file path usually prefer
/// `compile_at_path` which walks for `corvid.toml` automatically.
pub fn compile_with_config(source: &str, config: Option<&CorvidConfig>) -> CompileResult {
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
    let checked = typecheck_with_config(&file, &resolved, config);
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

    let config = load_corvid_config_for(source_path);
    let result = compile_with_config(&source, config.as_ref());

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

    let config = load_corvid_config_for(source_path);
    match compile_to_ir_with_config(&source, config.as_ref()) {
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
            // Production users pass `--with-tools-lib` to the CLI;
            // this path is the one hit by that flow and by tool-free
            // `corvid build --target=native`.
            // Empty tools-lib list = no user tool crates linked — tool-using
            // programs fail at link time with an unresolved-symbol
            // error that surfaces the missing tool by name.
            let produced =
                corvid_codegen_cl::build_native_to_disk(&ir, &stem, &requested, &[])
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
    compile_to_ir_with_config(source, None)
}

/// IR-lowering variant that consumes an explicit `corvid.toml` config
/// so user-defined effect dimensions are visible to the type checker.
pub fn compile_to_ir_with_config(
    source: &str,
    config: Option<&CorvidConfig>,
) -> Result<IrFile, Vec<Diagnostic>> {
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
    let checked = typecheck_with_config(&file, &resolved, config);
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
    let config = load_corvid_config_for(path);
    let ir = compile_to_ir_with_config(&source, config.as_ref()).map_err(RunError::Compile)?;
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

/// Which execution tier `corvid run` should use.
///
/// - `Auto` (default): try the native AOT tier; fall back to the
///   interpreter when the program uses features native doesn't support
///   yet (tool calls, prompts, `approve`, Python imports). A one-line
///   stderr message announces the fallback so the user can reason about
///   which tier actually ran.
/// - `Native`: require the native tier. Programs that need the
///   interpreter fail with a clean error naming the missing feature.
/// - `Interpreter`: force the interpreter, even when native would work.
///   Useful for debugging, trace capture, and comparing tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunTarget {
    Auto,
    Native,
    Interpreter,
}

/// `corvid run <file>` with auto-dispatch — native tier where possible,
/// interpreter fallback with an announced-on-stderr reason otherwise.
/// Equivalent to `run_with_target(path, RunTarget::Auto, None)`.
pub fn run_native(path: &Path) -> Result<u8, anyhow::Error> {
    run_with_target(path, RunTarget::Auto, None)
}

/// `corvid run <file> [--target=...] [--with-tools-lib <path>]`
/// entry point. Dispatches by tier per `target`; when `tools_lib`
/// is `Some`, tool-using programs gain access to the native tier
/// (their tool implementations live in that staticlib). Without a tools_lib, tool calls still route
/// to the interpreter fallback (auto) or hard-fail (native).
///
/// Common setup (env, tracer config) lives in the per-tier helpers
/// since only the interpreter needs the async runtime.
pub fn run_with_target(
    path: &Path,
    target: RunTarget,
    tools_lib: Option<&Path>,
) -> Result<u8, anyhow::Error> {
    // Env is loaded for both tiers: the native binary may read it via
    // libc `getenv` (the entry shim's leak-counter toggle does), and the
    // interpreter needs API keys from it.
    if let Some(parent) = path.parent() {
        let _ = load_dotenv_walking(parent);
    }
    let _ = load_dotenv_walking(&std::env::current_dir().unwrap_or_else(|_| Path::new(".").into()));

    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read `{}`: {e}", path.display());
            return Ok(1);
        }
    };
    let config = load_corvid_config_for(path);
    let ir = match compile_to_ir_with_config(&source, config.as_ref()) {
        Ok(ir) => ir,
        Err(diags) => {
            eprint!("{}", render_all_pretty(&diags, path, &source));
            return Ok(1);
        }
    };

    // Tool calls are native-able only when the caller supplied a
    // tools staticlib. The `native_ability` scan reports ToolCall
    // unconditionally (it doesn't know about the lib); the dispatcher
    // here decides whether to treat that reason as a blocker. Other
    // reasons (python imports, prompt calls) still block until their
    // respective feature gaps.
    let scan = native_ability(&ir);
    let tools_satisfy = |r: &NotNativeReason| -> bool {
        matches!(r, NotNativeReason::ToolCall { .. }) && tools_lib.is_some()
    };

    match target {
        RunTarget::Native => match &scan {
            Ok(()) => run_via_native_tier(path, &source, &ir, tools_lib),
            Err(reason) if tools_satisfy(reason) => {
                run_via_native_tier(path, &source, &ir, tools_lib)
            }
            Err(reason) => {
                eprintln!(
                    "error: `--target=native` refused: {reason}. Run without `--target` to fall back to the interpreter."
                );
                Ok(1)
            }
        },
        RunTarget::Interpreter => run_via_interpreter_tier(path, &ir),
        RunTarget::Auto => match &scan {
            Ok(()) => run_via_native_tier(path, &source, &ir, tools_lib),
            Err(reason) if tools_satisfy(reason) => {
                run_via_native_tier(path, &source, &ir, tools_lib)
            }
            Err(reason) => {
                eprintln!("↻ running via interpreter: {reason}");
                run_via_interpreter_tier(path, &ir)
            }
        },
    }
}

/// Interpreter tier: build a `Runtime` with stdin approver + env-driven
/// LLM adapters + JSONL tracer, run the entry agent under the async
/// interpreter, print its return value. Matches prior `run_native`
/// semantics exactly — this is the only path that existed before 12j.
fn run_via_interpreter_tier(path: &Path, ir: &IrFile) -> Result<u8, anyhow::Error> {
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
    let result = tokio_rt.block_on(run_ir_with_runtime(ir, None, vec![], &rt));

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

/// Native tier: produce a binary (via cache when possible) and exec it.
/// The codegen-emitted `main` handles argv decoding and result printing,
/// so we inherit stdin/stdout/stderr and let the binary
/// own the user interaction directly.
fn run_via_native_tier(
    path: &Path,
    source: &str,
    ir: &IrFile,
    tools_lib: Option<&Path>,
) -> Result<u8, anyhow::Error> {
    let binary = build_or_get_cached_native(path, source, ir, tools_lib)?.path;
    let status = std::process::Command::new(&binary)
        .status()
        .map_err(|e| anyhow::anyhow!("spawn native binary `{}`: {e}", binary.display()))?;
    Ok(status.code().unwrap_or(1) as u8)
}

/// Result of asking the cache for a compiled binary — used by tests to
/// verify cache hits without re-timing the whole pipeline.
#[derive(Debug, Clone)]
pub struct CachedNativeBinary {
    pub path: PathBuf,
    /// `true` if the binary already existed in the cache (no recompile
    /// happened this call); `false` if we compiled it now.
    pub from_cache: bool,
}

/// Core compile-or-reuse path. Hashes the inputs to pick a cache slot,
/// uses the existing binary if it's there, otherwise invokes codegen
/// + link and stores the result keyed by that hash.
///
/// Does NOT run the binary — that's the caller's job. Exposed as `pub`
/// so tests + future `corvid build --cache` tooling can observe the
/// cache state without executing.
pub fn build_or_get_cached_native(
    path: &Path,
    source: &str,
    ir: &IrFile,
    tools_lib: Option<&Path>,
) -> anyhow::Result<CachedNativeBinary> {
    let cache_dir = native_cache::cache_dir_for(path);
    // Tools-lib path participates in the cache key: if the user
    // swaps between `--with-tools-lib A` and `--with-tools-lib B`,
    // they get distinct cached binaries. Re-linking against the same
    // lib re-uses. Users who modify A in place and keep the same
    // path get stale cache — a `cargo clean` fixes it; a future
    // future polish work could hash the lib contents.
    let tools_lib_str = tools_lib
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let key = native_cache::cache_key_with_tools(source, &tools_lib_str);
    let cached = native_cache::cached_binary_path(&cache_dir, &key);
    if cached.exists() {
        return Ok(CachedNativeBinary {
            path: cached,
            from_cache: true,
        });
    }
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| anyhow::anyhow!("create cache dir `{}`: {e}", cache_dir.display()))?;
    // `build_native_to_disk` takes the final bin_path and derives parent
    // + stem from it — passing `<cache_dir>/<key>` produces
    // `<cache_dir>/<key>[.exe]` which is exactly where we want it.
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("program")
        .to_string();
    let module_name = format!("corvid_native_{key}");
    let target_bin = cache_dir.join(&key);
    // Forward the tools lib (if any) to the linker so
    // `__corvid_tool_<name>` symbols resolve against the user's
    // compiled `#[tool]` implementations.
    let extra_libs_owned: Vec<&Path> = tools_lib.iter().copied().collect();
    let produced = corvid_codegen_cl::build_native_to_disk(
        ir,
        &module_name,
        &target_bin,
        &extra_libs_owned,
    )
    .map_err(|e| anyhow::anyhow!("native codegen failed for `{stem}`: {e}"))?;
    Ok(CachedNativeBinary {
        path: produced,
        from_cache: false,
    })
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
                assert_eq!(s.type_name(), "Decision");
                assert_eq!(s.get_field("should_refund").unwrap(), Value::Bool(true));
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

    // ========================================================
    // Native-tier dispatch plus compile cache.
    // ========================================================

    const NATIVE_ABLE_SRC: &str = "agent main() -> Int:\n    return 7 * 6\n";

    const TOOL_USING_SRC: &str = r#"
tool lookup(id: String) -> Int
agent main(id: String) -> Int:
    return lookup(id)
"#;

    const PYTHON_IMPORT_SRC: &str = r#"
import python "math" as math

agent main() -> Int:
    return 1
"#;

    const PROMPT_USING_SRC: &str = r#"
prompt greet(name: String) -> String:
    """
    Say hi to {name}.
    """

agent main() -> String:
    return greet("world")
"#;

    const NULLABLE_OPTION_STRING_SRC: &str = r#"
agent maybe(flag: Bool) -> Option<String>:
    if flag:
        return Some("hi")
    return None

agent main() -> Bool:
    return maybe(true) != None
"#;

    const WIDE_OPTION_INT_SRC: &str = r#"
agent maybe(flag: Bool) -> Option<Int>:
    if flag:
        return Some(7)
    return None

agent main() -> Bool:
    return maybe(true) != None
"#;

    const WIDE_OPTION_INT_TRY_SRC: &str = r#"
agent maybe(flag: Bool) -> Option<Int>:
    if flag:
        return Some(7)
    return None

agent unwrap(flag: Bool) -> Option<Int>:
    value = maybe(flag)?
    return Some(value + 1)

agent main() -> Bool:
    return unwrap(true) != None
"#;

    const WIDE_OPTION_INT_TRY_WIDEN_SRC: &str = r#"
agent maybe(flag: Bool) -> Option<Int>:
    if flag:
        return Some(7)
    return None

agent widen(flag: Bool) -> Option<Bool>:
    value = maybe(flag)?
    return Some(value > 0)

agent main() -> Bool:
    return widen(true) != None
"#;

    const NULLABLE_OPTION_TRY_WIDEN_SRC: &str = r#"
agent maybe(flag: Bool) -> Option<String>:
    if flag:
        return Some("hi")
    return None

agent widen(flag: Bool) -> Option<Bool>:
    value = maybe(flag)?
    return Some(value == "hi")

agent main() -> Bool:
    return widen(true) != None
"#;

    const NULLABLE_OPTION_TRY_SRC: &str = r#"
agent maybe(flag: Bool) -> Option<String>:
    if flag:
        return Some("hi")
    return None

agent unwrap(flag: Bool) -> Option<String>:
    value = maybe(flag)?
    return Some(value)

agent main() -> Bool:
    return unwrap(true) != None
"#;

    const NATIVE_RESULT_STRING_SRC: &str = r#"
agent fetch(flag: Bool) -> Result<String, String>:
    if flag:
        return Ok("hi")
    return Err("no")

agent main() -> Bool:
    first = fetch(true)
    second = fetch(false)
    return true
"#;

    const NATIVE_RESULT_TRY_SRC: &str = r#"
agent fetch(flag: Bool) -> Result<String, String>:
    if flag:
        return Ok("hi")
    return Err("no")

agent forward(flag: Bool) -> Result<String, String>:
    value = fetch(flag)?
    return Ok(value)

agent main() -> Bool:
    first = forward(true)
    second = forward(false)
    return true
"#;

    const NATIVE_RESULT_TRY_WIDEN_SRC: &str = r#"
agent fetch(flag: Bool) -> Result<String, String>:
    if flag:
        return Ok("hi")
    return Err("no")

agent widen(flag: Bool) -> Result<Bool, String>:
    value = fetch(flag)?
    return Ok(true)

agent main() -> Bool:
    first = widen(true)
    second = widen(false)
    return true
"#;

    const NATIVE_RESULT_RETRY_SRC: &str = r#"
agent fetch(flag: Bool) -> Result<String, String>:
    if flag:
        return Ok("hi")
    return Err("no")

agent retrying(flag: Bool) -> Result<String, String>:
    return try fetch(flag) on error retry 3 times backoff linear 0

agent main() -> Bool:
    first = retrying(true)
    second = retrying(false)
    return true
"#;

    const NATIVE_OPTION_RETRY_SRC: &str = r#"
agent fetch(flag: Bool) -> Option<Int>:
    if flag:
        return Some(7)
    return None

agent retrying(flag: Bool) -> Option<Int>:
    return try fetch(flag) on error retry 3 times backoff linear 0

agent main() -> Bool:
    first = retrying(true)
    second = retrying(false)
    return true
"#;

    const NATIVE_NESTED_OPTION_INT_SRC: &str = r#"
agent fetch(mode: Int) -> Option<Option<Int>>:
    if mode == 0:
        return None
    if mode == 1:
        return Some(None)
    return Some(Some(7))

agent main() -> Bool:
    first = fetch(0)
    second = fetch(1)
    third = fetch(2)
    return first == None and second != None and third != None
"#;

    const NATIVE_NESTED_OPTION_INT_TRY_SRC: &str = r#"
agent fetch(mode: Int) -> Option<Option<Int>>:
    if mode == 0:
        return None
    if mode == 1:
        return Some(None)
    return Some(Some(7))

agent inspect(mode: Int) -> Option<Bool>:
    value = fetch(mode)?
    return Some(value == None or value != None)

agent main() -> Bool:
    return inspect(0) == None and inspect(1) != None and inspect(2) != None
"#;

    const NATIVE_RESULT_OPTION_INT_SRC: &str = r#"
agent fetch(flag: Bool) -> Result<Option<Int>, String>:
    if flag:
        return Ok(Some(7))
    return Err("no")

agent main() -> Bool:
    first = fetch(true)
    second = fetch(false)
    return true
"#;

    const NATIVE_RESULT_OPTION_INT_TRY_SRC: &str = r#"
agent fetch(flag: Bool) -> Result<Option<Int>, String>:
    if flag:
        return Ok(Some(7))
    return Err("no")

agent forward(flag: Bool) -> Result<Option<Int>, String>:
    value = fetch(flag)?
    return Ok(value)

agent main() -> Bool:
    first = forward(true)
    second = forward(false)
    return true
"#;

    const NATIVE_RESULT_OPTION_INT_RETRY_SRC: &str = r#"
prompt probe() -> String:
    """
    Probe
    """

agent fetch() -> Result<Option<Int>, String>:
    value = probe()
    if value == "ok":
        return Ok(Some(7))
    return Err(value)

agent retrying() -> Result<Option<Int>, String>:
    return try fetch() on error retry 3 times backoff linear 0

agent main() -> Bool:
    first = retrying()
    return probe() == "marker"
"#;

    const NATIVE_RESULT_STRUCT_SRC: &str = r#"
type Boxed:
    value: Int

agent fetch(flag: Bool) -> Result<Boxed, String>:
    if flag:
        return Ok(Boxed(7))
    return Err("no")

agent main() -> Bool:
    first = fetch(true)
    second = fetch(false)
    return true
"#;

    const NATIVE_RESULT_STRUCT_TRY_SRC: &str = r#"
type Boxed:
    value: Int

agent fetch(flag: Bool) -> Result<Boxed, String>:
    if flag:
        return Ok(Boxed(7))
    return Err("no")

agent forward(flag: Bool) -> Result<Boxed, String>:
    value = fetch(flag)?
    return Ok(value)

agent main() -> Bool:
    first = forward(true)
    second = forward(false)
    return true
"#;

    const NATIVE_RESULT_LIST_INT_SRC: &str = r#"
agent fetch(flag: Bool) -> Result<List<Int>, String>:
    if flag:
        return Ok([1, 2, 3])
    return Err("no")

agent main() -> Bool:
    first = fetch(true)
    second = fetch(false)
    return true
"#;

    const NATIVE_RESULT_LIST_INT_TRY_SRC: &str = r#"
agent fetch(flag: Bool) -> Result<List<Int>, String>:
    if flag:
        return Ok([1, 2, 3])
    return Err("no")

agent forward(flag: Bool) -> Result<List<Int>, String>:
    value = fetch(flag)?
    return Ok(value)

agent main() -> Bool:
    first = forward(true)
    second = forward(false)
    return true
"#;

    const NATIVE_RESULT_NESTED_OK_SRC: &str = r#"
agent fetch(flag: Bool) -> Result<Result<Int, String>, String>:
    if flag:
        return Ok(Ok(7))
    return Err("no")

agent main() -> Bool:
    first = fetch(true)
    second = fetch(false)
    return true
"#;

    const NATIVE_RESULT_NESTED_OK_TRY_SRC: &str = r#"
agent fetch(flag: Bool) -> Result<Result<Int, String>, String>:
    if flag:
        return Ok(Ok(7))
    return Err("no")

agent forward(flag: Bool) -> Result<Result<Int, String>, String>:
    value = fetch(flag)?
    return Ok(value)

agent main() -> Bool:
    first = forward(true)
    second = forward(false)
    return true
"#;

    const NATIVE_RESULT_NESTED_ERR_TRY_SRC: &str = r#"
agent inner_error() -> Result<String, Bool>:
    return Err(false)

agent fetch(flag: Bool) -> Result<Int, Result<String, Bool>>:
    if flag:
        return Ok(7)
    return Err(inner_error())

agent widen(flag: Bool) -> Result<Bool, Result<String, Bool>>:
    value = fetch(flag)?
    return Ok(value == 7)

agent main() -> Bool:
    first = widen(true)
    second = widen(false)
    return true
"#;

    const NATIVE_STRING_RETRY_REJECTED_SRC: &str = r#"
prompt lookup(id: String) -> String:
    """
    Lookup {id}
    """

agent load(id: String) -> String:
    return try lookup(id) on error retry 3 times backoff exponential 40
"#;

    #[test]
    fn native_ability_accepts_pure_computation() {
        let ir = compile_to_ir(NATIVE_ABLE_SRC).expect("compile");
        assert!(native_ability(&ir).is_ok());
    }

    #[test]
    fn native_ability_rejects_tool_call() {
        let ir = compile_to_ir(TOOL_USING_SRC).expect("compile");
        match native_ability(&ir) {
            Err(NotNativeReason::ToolCall { name }) => assert_eq!(name, "lookup"),
            other => panic!("expected ToolCall rejection, got {other:?}"),
        }
    }

    #[test]
    fn native_ability_rejects_python_import() {
        let ir = compile_to_ir(PYTHON_IMPORT_SRC).expect("compile");
        match native_ability(&ir) {
            Err(NotNativeReason::PythonImport { module }) => assert_eq!(module, "math"),
            other => panic!("expected PythonImport rejection, got {other:?}"),
        }
    }

    #[test]
    fn native_ability_accepts_prompt_calls() {
        // Prompt calls compile and run natively against the runtime's
        // bundled LLM adapters.
        let ir = compile_to_ir(PROMPT_USING_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "prompt support is native now; scan should accept prompt-using IRs"
        );
    }

    #[test]
    fn native_ability_accepts_nullable_option_with_refcounted_payload() {
        let ir = compile_to_ir(NULLABLE_OPTION_STRING_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "nullable-pointer Option<String> should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_wide_scalar_option_payloads() {
        let ir = compile_to_ir(WIDE_OPTION_INT_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "wide scalar Option<Int> should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_nullable_option_try_propagation() {
        let ir = compile_to_ir(NULLABLE_OPTION_TRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "nullable Option<String> `?` should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_wide_scalar_option_try_propagation() {
        let ir = compile_to_ir(WIDE_OPTION_INT_TRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "wide scalar Option<Int> `?` should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_wide_scalar_option_try_with_different_payload_type() {
        let ir = compile_to_ir(WIDE_OPTION_INT_TRY_WIDEN_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Option<Int> `?` inside Option<Bool> should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_nullable_option_try_with_wide_outer_payload() {
        let ir = compile_to_ir(NULLABLE_OPTION_TRY_WIDEN_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Option<String> `?` inside Option<Bool> should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_subset() {
        let ir = compile_to_ir(NATIVE_RESULT_STRING_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "one-word Result<String, String> should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_try_propagation() {
        let ir = compile_to_ir(NATIVE_RESULT_TRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "same-shape Result<String, String> `?` should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_try_with_different_ok_type() {
        let ir = compile_to_ir(NATIVE_RESULT_TRY_WIDEN_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Result<A, E> `?` inside Result<B, E> should now compile natively when the error type matches"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_retry_subset() {
        let ir = compile_to_ir(NATIVE_RESULT_RETRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "retry over the native Result<T, E> subset should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_option_retry_subset() {
        let ir = compile_to_ir(NATIVE_OPTION_RETRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "retry over the native Option<T> subset should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_nested_option_payloads() {
        let ir = compile_to_ir(NATIVE_NESTED_OPTION_INT_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Option<Option<Int>> should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_nested_option_try_propagation() {
        let ir = compile_to_ir(NATIVE_NESTED_OPTION_INT_TRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Option<Option<Int>> `?` should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_with_wide_option_payload() {
        let ir = compile_to_ir(NATIVE_RESULT_OPTION_INT_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Result<Option<Int>, String> should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_with_wide_option_try_propagation() {
        let ir = compile_to_ir(NATIVE_RESULT_OPTION_INT_TRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Result<Option<Int>, String> `?` should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_with_wide_option_retry() {
        let ir = compile_to_ir(NATIVE_RESULT_OPTION_INT_RETRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "retry over Result<Option<Int>, String> should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_with_struct_payload() {
        let ir = compile_to_ir(NATIVE_RESULT_STRUCT_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Result<Struct, String> should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_with_struct_try_propagation() {
        let ir = compile_to_ir(NATIVE_RESULT_STRUCT_TRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Result<Struct, String> `?` should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_with_list_payload() {
        let ir = compile_to_ir(NATIVE_RESULT_LIST_INT_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Result<List<Int>, String> should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_with_list_try_propagation() {
        let ir = compile_to_ir(NATIVE_RESULT_LIST_INT_TRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Result<List<Int>, String> `?` should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_with_nested_ok_payload() {
        let ir = compile_to_ir(NATIVE_RESULT_NESTED_OK_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Result<Result<Int, String>, String> should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_with_nested_ok_try_propagation() {
        let ir = compile_to_ir(NATIVE_RESULT_NESTED_OK_TRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Result<Result<Int, String>, String> `?` should now compile natively"
        );
    }

    #[test]
    fn native_ability_accepts_native_result_with_nested_error_try_widening() {
        let ir = compile_to_ir(NATIVE_RESULT_NESTED_ERR_TRY_SRC).expect("compile");
        assert!(
            native_ability(&ir).is_ok(),
            "Result<A, Result<B, C>> `?` inside Result<D, Result<B, C>> should now compile natively"
        );
    }

    #[test]
    fn native_ability_rejects_retry_over_non_result_or_option_body() {
        let ir = compile_to_ir(NATIVE_STRING_RETRY_REJECTED_SRC).expect("compile");
        match native_ability(&ir) {
            Err(NotNativeReason::TaggedUnionRetryNotNative) => {}
            other => panic!("expected retry subset rejection, got {other:?}"),
        }
    }

    /// Second compilation of the same source hits the cache: no
    /// recompile, binary is the same path, mtime doesn't advance.
    #[test]
    fn native_cache_hits_on_second_call() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src_path = tmp.path().join("hello.cor");
        std::fs::write(&src_path, NATIVE_ABLE_SRC).expect("write");
        let ir = compile_to_ir(NATIVE_ABLE_SRC).expect("compile");

        let first = build_or_get_cached_native(&src_path, NATIVE_ABLE_SRC, &ir, None).expect("first");
        assert!(!first.from_cache, "first call must compile (not cached yet)");
        assert!(first.path.exists(), "first build should produce a binary");
        let first_mtime = std::fs::metadata(&first.path).unwrap().modified().unwrap();

        let second = build_or_get_cached_native(&src_path, NATIVE_ABLE_SRC, &ir, None).expect("second");
        assert!(second.from_cache, "second call must reuse cached binary");
        assert_eq!(first.path, second.path, "same cache key => same path");
        let second_mtime = std::fs::metadata(&second.path).unwrap().modified().unwrap();
        assert_eq!(
            first_mtime, second_mtime,
            "cache hit must not rewrite the binary"
        );
    }

    /// Auto-dispatch on a native-able program runs via native and
    /// produces the binary under `target/cache/native/`. The exit code
    /// from `run_with_target` comes from the spawned binary itself.
    #[test]
    fn run_with_target_auto_uses_native_for_pure_program() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src_path = tmp.path().join("pure.cor");
        std::fs::write(&src_path, NATIVE_ABLE_SRC).expect("write");

        let code = run_with_target(&src_path, RunTarget::Auto, None).expect("run");
        assert_eq!(code, 0, "pure program should exit 0");
        // Cache populated under <tmpdir>/target/cache/native/.
        let cache_dir = tmp.path().join("target").join("cache").join("native");
        assert!(
            cache_dir.exists(),
            "native cache dir should exist after auto-run, got missing: {}",
            cache_dir.display()
        );
        let entries: Vec<_> = std::fs::read_dir(&cache_dir).unwrap().collect();
        assert!(
            !entries.is_empty(),
            "native cache dir should contain at least one binary"
        );
    }

    /// `--target=native` on a tool-using program must NOT silently fall
    /// back — it must exit non-zero with the reason printed to stderr.
    /// Verified by checking `run_with_target` returns exit 1 and the
    /// program never runs. We don't capture stderr here (Rust tests
    /// don't expose a clean way without a process boundary), but the
    /// exit code is the contract this helper promises.
    #[test]
    fn run_with_target_native_required_errors_on_tool_use() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src_path = tmp.path().join("tooly.cor");
        std::fs::write(&src_path, TOOL_USING_SRC).expect("write");

        let code = run_with_target(&src_path, RunTarget::Native, None).expect("run");
        assert_eq!(
            code, 1,
            "native-required on a tool-using program must exit 1"
        );
    }
}
