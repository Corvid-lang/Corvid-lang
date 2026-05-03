use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum AbiCommand {
    Dump {
        library: PathBuf,
    },
    Hash {
        source: PathBuf,
    },
    Verify {
        library: PathBuf,
        #[arg(long, value_name = "HEX")]
        expected_hash: String,
    },
}
