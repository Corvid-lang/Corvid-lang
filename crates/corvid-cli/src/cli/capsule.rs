use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum CapsuleCommand {
    Create {
        trace: PathBuf,
        cdylib: PathBuf,
        #[arg(long, value_name = "PATH")]
        out: Option<PathBuf>,
    },
    Replay {
        capsule: PathBuf,
    },
}
