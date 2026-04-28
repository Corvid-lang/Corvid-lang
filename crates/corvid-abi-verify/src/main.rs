use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "corvid-abi-verify",
    about = "Independently rebuild and compare a Corvid cdylib ABI descriptor"
)]
struct Args {
    /// Corvid source file used to rebuild the descriptor.
    #[arg(long)]
    source: PathBuf,

    /// Built cdylib containing CORVID_ABI_DESCRIPTOR.
    cdylib: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();
    match corvid_abi_verify::verify_source_matches_cdylib(&args.source, &args.cdylib) {
        Ok(report) => {
            eprintln!(
                "ABI descriptor OK ({} bytes, sha256={})",
                report.embedded_json_len,
                corvid_abi_verify::hex_hash(&report.embedded_json_hash)
            );
            Ok(())
        }
        Err(err) => {
            eprintln!("ABI descriptor verification failed: {err:#}");
            std::process::exit(1);
        }
    }
}
