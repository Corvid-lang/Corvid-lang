//! The `corvid` CLI.
//!
//! Subcommands:
//!   corvid new <name>         scaffold a new project
//!   corvid check <file>       type-check a source file
//!   corvid build <file>       compile to target/py/<name>.py
//!   corvid run <file>         build + invoke python on the output
//!   corvid repl               start the interactive REPL
//!   corvid test <what>        run verification suites (dimensions, spec, adversarial)
//!   corvid verify             cross-tier effect-profile verification
//!   corvid effect-diff        diff composed effect profiles between two revisions
//!   corvid add-dimension      install a dimension from the effect registry

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use corvid_differential_verify::{
    render_corpus_grid, render_report, shrink_program, verify_corpus,
};

#[allow(unused_imports)]
use corvid_driver::{
    build_native_to_disk, build_to_disk, compile, compile_with_config, load_corvid_config_for,
    load_dotenv_walking, render_all_pretty, run_native, run_with_target, scaffold_new, RunTarget,
};

#[derive(Parser)]
#[command(name = "corvid", version, about = "The Corvid language compiler")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Scaffold a new Corvid project.
    New { name: String },
    /// Type-check a Corvid source file.
    Check { file: PathBuf },
    /// Compile a Corvid source file. Default target is Python (target/py/);
    /// `--target=native` emits a machine-code binary under target/bin/.
    Build {
        file: PathBuf,
        /// Output target. `python` (default) or `native`.
        #[arg(long, default_value = "python")]
        target: String,
    },
    /// Build and run a Corvid source file. Picks the native AOT tier
    /// when the program stays within the current native command-line
    /// boundary; falls back to the interpreter otherwise with a one-line
    /// notice. Override with `--target`.
    Run {
        file: PathBuf,
        /// Execution tier. `auto` (default) tries native first, falls
        /// back to interpreter when a feature isn't native-able yet.
        /// `native` requires native and errors out otherwise. `interp`
        /// / `interpreter` forces the interpreter tier.
        #[arg(long, default_value = "auto")]
        target: String,
        /// Path to a compiled `#[tool]` staticlib. When
        /// provided, tool-using programs compile and run natively; the
        /// linker resolves `__corvid_tool_<name>` symbols against this
        /// lib. Without it, tool-using programs fall back to the
        /// interpreter (auto) or error out (native).
        #[arg(long, value_name = "PATH")]
        with_tools_lib: Option<PathBuf>,
    },
    /// Run verification suites.
    ///
    /// Targets:
    ///   `dimensions`    algebraic-law proptest over every custom dimension
    ///                   declared in corvid.toml
    ///   `spec`          recompile every .cor example in docs/effects-spec/
    ///                   against the current toolchain; with --meta, run the
    ///                   self-verifying verification harness
    ///   `adversarial`   LLM-driven bypass generation against the effect
    ///                   checker
    ///
    /// Without a target, acts as a placeholder for the future unit-test
    /// runner.
    Test {
        /// What to verify. Omit for the legacy placeholder behavior.
        target: Option<String>,
        /// For `spec`: run the meta-verification harness (mutate the
        /// verifier, confirm each counter-example is still caught).
        #[arg(long)]
        meta: bool,
        /// For `adversarial`: number of bypass programs to generate.
        #[arg(long, default_value = "100")]
        count: u32,
        /// For `adversarial`: model to drive the generator.
        #[arg(long, default_value = "opus")]
        model: String,
    },
    /// Cross-verify effect profiles across checker, interpreter,
    /// native, and replay tiers.
    Verify {
        /// Verify every `.cor` file under this directory recursively.
        #[arg(long, value_name = "DIR", conflicts_with = "shrink")]
        corpus: Option<PathBuf>,
        /// Shrink a divergent program to a smaller reproducer.
        #[arg(long, value_name = "FILE", conflicts_with = "corpus")]
        shrink: Option<PathBuf>,
        /// Emit the structured report as JSON to stderr.
        #[arg(long)]
        json: bool,
    },
    /// Diff the composed effect profile between two revisions.
    /// Reports dimension-value drift per agent and constraints that
    /// newly fire or release because of the change.
    EffectDiff {
        /// Before revision (git ref or file path).
        before: String,
        /// After revision (git ref or file path).
        after: String,
    },
    /// Install a dimension from the Corvid effect registry.
    /// Verifies the dimension's signature, replays its algebraic-law
    /// proofs against the current toolchain, adds it to corvid.toml.
    AddDimension {
        /// Dimension spec in `name@version` form (e.g. `fairness@1.2`).
        spec: String,
    },
    /// Start the interactive Corvid REPL.
    Repl,
    /// Check the local environment for required tools.
    Doctor,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match cli.command {
        Some(Command::New { name }) => cmd_new(&name),
        Some(Command::Check { file }) => cmd_check(&file),
        Some(Command::Build { file, target }) => cmd_build(&file, &target),
        Some(Command::Run {
            file,
            target,
            with_tools_lib,
        }) => cmd_run(&file, &target, with_tools_lib.as_deref()),
        Some(Command::Test {
            target,
            meta,
            count,
            model,
        }) => cmd_test(target.as_deref(), meta, count, &model),
        Some(Command::Verify { corpus, shrink, json }) => {
            cmd_verify(corpus.as_deref(), shrink.as_deref(), json)
        }
        Some(Command::EffectDiff { before, after }) => cmd_effect_diff(&before, &after),
        Some(Command::AddDimension { spec }) => cmd_add_dimension(&spec),
        Some(Command::Repl) => cmd_repl(),
        Some(Command::Doctor) => cmd_doctor(),
        None => {
            println!("corvid — the AI-native language compiler");
            println!("Run `corvid --help` for usage.");
            Ok(0)
        }
    };

    match result {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

// ------------------------------------------------------------
// Commands
// ------------------------------------------------------------

fn cmd_new(name: &str) -> Result<u8> {
    let root = scaffold_new(name).context("failed to scaffold project")?;
    println!("created new Corvid project at `{}`", root.display());
    println!("\nNext steps:");
    println!("  cd {name}");
    println!("  pip install corvid-runtime");
    println!("  corvid run src/main.cor");
    Ok(0)
}

fn cmd_check(file: &Path) -> Result<u8> {
    let source = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read `{}`", file.display()))?;
    let config = load_corvid_config_for(file);
    let result = compile_with_config(&source, config.as_ref());
    if result.ok() {
        println!("ok: {} — no errors", file.display());
        Ok(0)
    } else {
        eprint!("{}", render_all_pretty(&result.diagnostics, file, &source));
        Ok(1)
    }
}

fn cmd_build(file: &Path, target: &str) -> Result<u8> {
    match target {
        "python" | "py" => {
            let out = build_to_disk(file)
                .with_context(|| format!("failed to build `{}`", file.display()))?;
            if let Some(path) = out.output_path {
                println!("built: {} -> {}", file.display(), path.display());
                Ok(0)
            } else {
                eprint!("{}", render_all_pretty(&out.diagnostics, file, &out.source));
                Ok(1)
            }
        }
        "native" => {
            let out = build_native_to_disk(file)
                .with_context(|| format!("failed to build `{}` (native)", file.display()))?;
            if let Some(path) = out.output_path {
                println!("built: {} -> {}", file.display(), path.display());
                Ok(0)
            } else {
                eprint!("{}", render_all_pretty(&out.diagnostics, file, &out.source));
                Ok(1)
            }
        }
        other => {
            anyhow::bail!(
                "unknown target `{other}`; valid: `python` (default), `native`"
            )
        }
    }
}

fn cmd_run(file: &Path, target: &str, tools_lib: Option<&Path>) -> Result<u8> {
    let rt = match target {
        "auto" => RunTarget::Auto,
        "native" => RunTarget::Native,
        "interp" | "interpreter" => RunTarget::Interpreter,
        other => anyhow::bail!(
            "unknown target `{other}`; valid: `auto` (default), `native`, `interpreter`"
        ),
    };
    if let Some(lib) = tools_lib {
        if !lib.exists() {
            anyhow::bail!(
                "--with-tools-lib `{}` does not exist — build the tools crate first (`cargo build -p <your-tools-crate> --release`)",
                lib.display()
            );
        }
    }
    // Auto: native AOT tier when the IR is tool-free and uses only
    // supported command-line boundary types, or when tool-using code
    // has a companion tools staticlib provided.
    // Interpreter otherwise (with a stderr notice). Native-required
    // and interpreter-forced are the explicit overrides. See
    // `RunTarget` docs in corvid-driver for the exact semantics.
    run_with_target(file, rt, tools_lib)
        .with_context(|| format!("failed to run `{}`", file.display()))
}

fn cmd_repl() -> Result<u8> {
    corvid_repl::Repl::run_stdio().context("failed to run `corvid repl`")?;
    Ok(0)
}

fn cmd_verify(corpus: Option<&Path>, shrink: Option<&Path>, json: bool) -> Result<u8> {
    match (corpus, shrink) {
        (Some(dir), None) => {
            let reports = verify_corpus(dir)?;
            let divergent: Vec<_> = reports
                .iter()
                .filter(|report| !report.divergences.is_empty())
                .collect();
            println!("{}", render_corpus_grid(&reports));
            if !divergent.is_empty() {
                println!();
                for (index, report) in divergent.iter().enumerate() {
                    if index > 0 {
                        println!();
                    }
                    println!("{}", render_report(report));
                }
            }
            if json {
                eprintln!("{}", serde_json::to_string_pretty(&reports)?);
            }
            Ok(if divergent.is_empty() { 0 } else { 1 })
        }
        (None, Some(file)) => {
            let result = shrink_program(file)?;
            println!(
                "shrunk reproducer: {} -> {} (removed {} line(s))",
                result.original.display(),
                result.output.display(),
                result.removed_lines
            );
            if json {
                eprintln!("{}", serde_json::to_string_pretty(&result)?);
            }
            Ok(0)
        }
        (None, None) => anyhow::bail!(
            "use `corvid verify --corpus <dir>` or `corvid verify --shrink <file>`"
        ),
        (Some(_), Some(_)) => unreachable!("clap enforces conflicts"),
    }
}

// ------------------------------------------------------------
// Verification suites — effect-system spec, custom dimensions,
// adversarial bypass generation
// ------------------------------------------------------------

fn cmd_test(
    target: Option<&str>,
    meta: bool,
    count: u32,
    model: &str,
) -> Result<u8> {
    match target {
        None => {
            eprintln!("`corvid test` with no target is the legacy placeholder.");
            eprintln!("Use one of: `corvid test dimensions`, `corvid test spec`,");
            eprintln!("`corvid test spec --meta`, `corvid test adversarial --count <N>`.");
            Ok(0)
        }
        Some("dimensions") => cmd_test_dimensions(),
        Some("spec") if meta => cmd_test_spec_meta(),
        Some("spec") => cmd_test_spec(),
        Some("adversarial") => cmd_test_adversarial(count, model),
        Some(other) => {
            anyhow::bail!(
                "unknown test target `{other}`; valid: `dimensions`, `spec`, `spec --meta`, `adversarial`"
            )
        }
    }
}

fn cmd_test_dimensions() -> Result<u8> {
    println!("corvid test dimensions — algebraic-law checks on custom dimensions\n");
    println!("This command reads corvid.toml, loads each [effect-system.dimensions.*]");
    println!("entry, then proptests the archetype's laws (associativity, commutativity,");
    println!("identity, idempotence, monotonicity) with 10,000 cases per law.");
    println!();
    println!("Implementation tracked in ROADMAP Phase 20g — not yet wired to the checker.");
    println!("See docs/effects-spec/01-dimensional-syntax.md §5 for the spec.");
    Ok(0)
}

fn cmd_test_spec() -> Result<u8> {
    println!("corvid test spec — re-compile every example in docs/effects-spec/\n");
    println!("Walks docs/effects-spec/examples/, runs `corvid check` on each .cor file,");
    println!("reports any example whose compile result no longer matches the spec's claim.");
    println!();
    println!("Implementation tracked in ROADMAP Phase 20g — not yet wired to CI.");
    Ok(0)
}

fn cmd_test_spec_meta() -> Result<u8> {
    println!("corvid test spec --meta — self-verifying verification\n");
    println!("Mutates the composition-algebra checker, confirms each historical");
    println!("counter-example (docs/effects-spec/counterexamples/) is caught, then");
    println!("restores the checker and re-verifies.");
    println!();
    println!("Implementation tracked in ROADMAP Phase 20g — harness not yet built.");
    println!("See docs/effects-spec/02-composition-algebra.md §11.");
    Ok(0)
}

fn cmd_test_adversarial(count: u32, model: &str) -> Result<u8> {
    println!("corvid test adversarial --count {count} --model {model}\n");
    println!("Drives an LLM to generate programs designed to bypass the dimensional");
    println!("effect checker. Every generated program runs through `corvid check`;");
    println!("any that compiles is a real bypass and is filed as an issue.");
    println!();
    println!("Implementation tracked in ROADMAP Phase 20g — generator not yet wired.");
    println!("See docs/effects-spec/README.md for the verification guarantees.");
    Ok(0)
}

// ------------------------------------------------------------
// Effect-diff tool
// ------------------------------------------------------------

fn cmd_effect_diff(before: &str, after: &str) -> Result<u8> {
    println!("corvid effect-diff {before} {after}\n");
    println!("Reports dimension-value drift per agent and constraints that newly");
    println!("fire or release between the two revisions.");
    println!();
    println!("Implementation tracked in ROADMAP Phase 20g — diff pipeline not yet");
    println!("wired. See docs/effects-spec/02-composition-algebra.md §9.");
    Ok(0)
}

// ------------------------------------------------------------
// Dimension registry client
// ------------------------------------------------------------

fn cmd_add_dimension(spec: &str) -> Result<u8> {
    let (name, version) = spec
        .split_once('@')
        .ok_or_else(|| anyhow::anyhow!("expected `name@version`, got `{spec}`"))?;
    if name.is_empty() || version.is_empty() {
        anyhow::bail!("expected `name@version`, got `{spec}`");
    }
    println!("corvid add-dimension {name}@{version}\n");
    println!("Resolves the dimension from the Corvid effect registry, verifies its");
    println!("signature, replays its algebraic-law proofs against the current");
    println!("toolchain, and — on success — adds it to corvid.toml.");
    println!();
    println!("Implementation tracked in ROADMAP Phase 20g — registry client not yet");
    println!("wired. See docs/effects-spec/02-composition-algebra.md §10.");
    Ok(0)
}

// ------------------------------------------------------------
// corvid doctor
// ------------------------------------------------------------

fn cmd_doctor() -> Result<u8> {
    use corvid_driver::load_dotenv_walking;

    println!("corvid doctor — checking local environment...\n");

    // Try loading .env first so the rest of the checks see what programs would.
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match load_dotenv_walking(&cwd) {
        Some(p) => println!("  ✓ .env loaded from {}", p.display()),
        None => println!("  · no .env file found from cwd upward (optional)"),
    }

    // CORVID_MODEL
    let model = std::env::var("CORVID_MODEL").ok();
    match &model {
        Some(v) => println!("  ✓ CORVID_MODEL = {v}"),
        None => println!(
            "  · CORVID_MODEL not set. Set one (e.g. `export CORVID_MODEL=gpt-4o-mini` or\n    `claude-opus-4-6`) or put `default_model = \"...\"` in corvid.toml under [llm]."
        ),
    }

    // Anthropic
    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        println!("  ✓ ANTHROPIC_API_KEY set (Claude models available)");
    } else {
        println!("  · ANTHROPIC_API_KEY not set — Claude calls will error at the prompt site");
    }

    // OpenAI
    if std::env::var("OPENAI_API_KEY").is_ok() {
        println!("  ✓ OPENAI_API_KEY set (GPT / o-series models available)");
    } else {
        println!("  · OPENAI_API_KEY not set — OpenAI calls will error at the prompt site");
    }

    // Cross-check: model prefix vs. available keys.
    if let Some(m) = &model {
        if m.starts_with("claude-") && std::env::var("ANTHROPIC_API_KEY").is_err() {
            println!(
                "  ✗ CORVID_MODEL is `{m}` but ANTHROPIC_API_KEY is not set"
            );
        }
        let openai_prefixes = ["gpt-", "o1-", "o3-", "o4-"];
        if openai_prefixes.iter().any(|p| m.starts_with(p))
            && std::env::var("OPENAI_API_KEY").is_err()
        {
            println!(
                "  ✗ CORVID_MODEL is `{m}` but OPENAI_API_KEY is not set"
            );
        }
    }

    // Python (legacy `--target=python` users only)
    let has_python = std::process::Command::new("python3")
        .arg("--version")
        .output()
        .ok()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if has_python {
        println!("  · python3 detected (legacy `--target=python` available)");
    } else {
        println!("  · python3 not detected (only needed for `--target=python`)");
    }

    println!();
    println!("native `corvid run` works without Python. Configure CORVID_MODEL + the");
    println!("matching API key and prompt-only programs run end-to-end.");
    Ok(0)
}
