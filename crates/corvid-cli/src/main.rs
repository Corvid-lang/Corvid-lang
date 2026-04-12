//! The `corvid` CLI.
//!
//! Subcommands (v0.1 target):
//!   corvid new <name>        scaffold a new project
//!   corvid check             type-check only
//!   corvid build             compile to target/py/
//!   corvid run <file>        build + run
//!   corvid test              run tests

use anyhow::Result;
use clap::{Parser, Subcommand};

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
    /// Type-check the current project.
    Check,
    /// Compile to target/.
    Build,
    /// Build and run a Corvid file.
    Run { file: String },
    /// Run tests.
    Test,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::New { name }) => {
            println!("TODO: scaffold project {name}");
        }
        Some(Command::Check) => {
            println!("TODO: check");
        }
        Some(Command::Build) => {
            println!("TODO: build");
        }
        Some(Command::Run { file }) => {
            println!("TODO: run {file}");
        }
        Some(Command::Test) => {
            println!("TODO: test");
        }
        None => {
            println!("corvid — the AI-native language compiler");
            println!("Run `corvid --help` for usage.");
        }
    }

    Ok(())
}
