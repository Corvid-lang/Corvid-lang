use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum ClaimCommand {
    /// Audit launch-facing claims for runnable evidence or explicit non-scope status.
    Audit {
        /// Claim inventory markdown table.
        #[arg(
            long,
            value_name = "PATH",
            default_value = "docs/launch-claim-audit.md"
        )]
        inventory: PathBuf,
        /// Emit JSON report.
        #[arg(long)]
        json: bool,
    },
}
