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

use corvid_driver::{build_to_disk, compile, render_all_pretty, scaffold_new};

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
    /// Compile a Corvid source file to Python (target/py/).
    Build { file: PathBuf },
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
        Some(Command::Build { file }) => cmd_build(&file),
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

fn cmd_build(file: &Path) -> Result<u8> {
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

fn cmd_run(file: &Path) -> Result<u8> {
    let out = build_to_disk(file)
        .with_context(|| format!("failed to build `{}`", file.display()))?;

    let Some(path) = out.output_path else {
        eprint!("{}", render_all_pretty(&out.diagnostics, file, &out.source));
        return Ok(1);
    };

    // Shell out to Python.
    let status = std::process::Command::new("python3")
        .arg(&path)
        .status()
        .with_context(|| "failed to spawn python3 — is it on PATH?")?;

    if status.success() {
        Ok(0)
    } else {
        Ok(status.code().unwrap_or(1) as u8)
    }
}

// ------------------------------------------------------------
// corvid doctor
// ------------------------------------------------------------

fn cmd_doctor() -> Result<u8> {
    println!("corvid doctor — checking local environment...\n");
    let mut ok = true;

    // Python 3.11+
    match std::process::Command::new("python3")
        .arg("--version")
        .output()
    {
        Ok(out) if out.status.success() => {
            let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
            println!("  ✓ {v}");
        }
        _ => {
            println!(
                "  ✗ python3 not found. Install Python 3.11 or newer.\n     see: https://www.python.org/downloads/"
            );
            ok = false;
        }
    }

    // corvid-runtime
    let has_runtime = std::process::Command::new("python3")
        .args(["-c", "import corvid_runtime; print(corvid_runtime.__name__)"])
        .output()
        .ok()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if has_runtime {
        println!("  ✓ corvid-runtime installed");
    } else {
        println!(
            "  ✗ corvid-runtime not installed.\n     run: pip install 'corvid-runtime[anthropic]'"
        );
        ok = false;
    }

    // anthropic (optional)
    let has_anthropic = std::process::Command::new("python3")
        .args(["-c", "import anthropic"])
        .output()
        .ok()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if has_anthropic {
        println!("  ✓ anthropic SDK installed (Claude adapter available)");
    } else {
        println!(
            "  · anthropic not installed (optional).\n     run: pip install 'corvid-runtime[anthropic]'"
        );
    }

    // CORVID_MODEL
    match std::env::var("CORVID_MODEL") {
        Ok(v) => println!("  ✓ CORVID_MODEL = {v}"),
        Err(_) => println!(
            "  · CORVID_MODEL not set. Set one (e.g. `export CORVID_MODEL=claude-opus-4-6`) or\n    put `default_model = \"...\"` in corvid.toml under [llm]."
        ),
    }

    println!();
    if ok {
        println!("all required components look good.");
        Ok(0)
    } else {
        println!("issues found — resolve the ✗ items above and run `corvid doctor` again.");
        Ok(1)
    }
}
