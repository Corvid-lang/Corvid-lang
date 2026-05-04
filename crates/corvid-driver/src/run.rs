//! Run execution — from source path to exit code.
//!
//! `corvid run <file>` picks a tier (auto / native / interpreter),
//! orchestrates the build, and invokes the runtime. Errors from any
//! stage are surfaced as `RunError`. The native tier also feeds a
//! per-binary `CachedNativeBinary` so repeated runs skip recompile.
//!
//! Extracted from `lib.rs` as part of Phase 20i responsibility
//! decomposition (20i-audit-driver-e).

use super::native_cache;
use super::{
    compile_to_ir_with_config_at_path, load_corvid_config_for, native_ability, render_all_pretty,
    run_ir_with_runtime, Diagnostic, NotNativeReason,
};
use corvid_ir::IrFile;
use corvid_runtime::{
    load_dotenv_walking, AnthropicAdapter, EnvVarMockAdapter, OllamaAdapter, OpenAiAdapter,
    RedactionSet, Runtime, StdinApprover, Tracer,
};
use corvid_vm::InterpError;
use std::fmt;
use std::path::{Path, PathBuf};

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
    let ir = match compile_to_ir_with_config_at_path(&source, path, config.as_ref()) {
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
    if std::env::var("CORVID_TEST_MOCK_LLM").ok().as_deref() == Some("1") {
        builder = builder.llm(std::sync::Arc::new(EnvVarMockAdapter::from_env()));
    }
    builder = builder.env_mock_tools_from_env();
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        builder = builder.llm(std::sync::Arc::new(AnthropicAdapter::new(key)));
    }
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        builder = builder.llm(std::sync::Arc::new(OpenAiAdapter::new(key)));
    }
    builder = builder.llm(std::sync::Arc::new(OllamaAdapter::new()));
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
