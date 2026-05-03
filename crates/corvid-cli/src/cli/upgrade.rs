use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum UpgradeCommand {
    /// Report syntax and stdlib migrations without modifying files.
    Check {
        /// Source file or project directory to scan.
        path: PathBuf,
        /// Emit JSON findings.
        #[arg(long)]
        json: bool,
    },
    /// Apply safe syntax and stdlib migrations.
    Apply {
        /// Source file or project directory to rewrite.
        path: PathBuf,
        /// Emit JSON findings.
        #[arg(long)]
        json: bool,
    },
}
