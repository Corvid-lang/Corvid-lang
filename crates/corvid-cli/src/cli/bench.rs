use clap::Subcommand;

#[derive(Subcommand)]
pub enum BenchCommand {
    /// Compare Corvid against Python or JS/TypeScript using a published archive.
    Compare {
        /// Comparison target: `python`, `js`, or `typescript`.
        target: String,
        /// Benchmark session id under `benches/results/`.
        #[arg(
            long,
            value_name = "SESSION",
            default_value = "2026-04-17-marketable-session"
        )]
        session: String,
        /// Emit structured JSON.
        #[arg(long)]
        json: bool,
    },
}
