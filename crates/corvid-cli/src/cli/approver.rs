use clap::{Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum ApproverCommand {
    Check {
        approver: PathBuf,
        #[arg(long, value_name = "USD")]
        max_budget_usd: Option<f64>,
    },
    Simulate {
        approver: PathBuf,
        site_label: String,
        #[arg(long, value_name = "JSON")]
        args: String,
        #[arg(long, value_name = "USD")]
        max_budget_usd: Option<f64>,
    },
    Card {
        site_label: String,
        #[arg(long, value_name = "JSON")]
        args: String,
        #[arg(long, value_enum, default_value_t = ApproverCardFormat::Text)]
        format: ApproverCardFormat,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ApproverCardFormat {
    Text,
    Json,
    Html,
}
