//! The `corvid` CLI.
//!
//! Subcommands:
//!   corvid new <name>         scaffold a new project
//!   corvid check <file>       type-check a source file
//!   corvid build <file>       compile to target/py/<name>.py
//!   corvid run <file>         build + invoke python on the output

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[allow(unused_imports)]
use corvid_driver::{
    build_native_to_disk, build_to_disk, compile, load_dotenv_walking, render_all_pretty,
    run_native, scaffold_new,
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
    /// Build a Corvid source file and run the generated Python.
    Run { file: PathBuf },
    /// Run tests (not yet implemented).
    Test,
    /// Check the local environment for required tools.
    Doctor,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match cli.command {
        Some(Command::New { name }) => cmd_new(&name),
        Some(Command::Check { file }) => cmd_check(&file),
        Some(Command::Build { file, target }) => cmd_build(&file, &target),
        Some(Command::Run { file }) => cmd_run(&file),
        Some(Command::Test) => {
            eprintln!("`corvid test` is not implemented yet (v0.2).");
            Ok(0)
        }
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
    let result = compile(&source);
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

fn cmd_run(file: &Path) -> Result<u8> {
    // Native dispatch via the interpreter + corvid-runtime — Python is no
    // longer on the path. Tools and prompts that need handlers must be
    // registered through a runner binary that calls `run_with_runtime`;
    // see `examples/refund_bot_demo/runner.rs`. `corvid run` itself uses
    // an empty runtime suitable for tool-free agents.
    run_native(file)
        .with_context(|| format!("failed to run `{}`", file.display()))
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
