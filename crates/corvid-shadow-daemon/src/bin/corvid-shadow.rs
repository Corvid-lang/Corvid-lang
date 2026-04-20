use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    Start { #[arg(long)] config: PathBuf },
    Ack {
        trace_path: PathBuf,
        #[arg(long)]
        reason: String,
        #[arg(long, default_value = "tests/regression-corpus")]
        target_corpus_dir: PathBuf,
    },
    Status,
    DumpAlerts {
        #[arg(long)]
        alert_log: PathBuf,
        #[arg(long)]
        since: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Command::Start { config } => {
            let handle = corvid_shadow_daemon::start_daemon(&config).await?;
            eprintln!("corvid-shadow: ready");
            tokio::signal::ctrl_c().await?;
            handle.shutdown().await;
            handle.wait().await;
        }
        Command::Ack {
            trace_path,
            reason,
            target_corpus_dir,
        } => {
            let action =
                corvid_shadow_daemon::ack_trace(&trace_path, &reason, &target_corpus_dir).await?;
            println!(
                "enrolled {} -> {}",
                action.trace_path.display(),
                action.enrolled_path.display()
            );
        }
        Command::Status => {
            println!("corvid-shadow status is only available from a running process in v1");
        }
        Command::DumpAlerts { alert_log, since } => {
            let alerts = corvid_shadow_daemon::dump_alerts(&alert_log, since.as_deref())?;
            for alert in alerts {
                println!("{}", serde_json::to_string(&alert)?);
            }
        }
    }
    Ok(())
}
