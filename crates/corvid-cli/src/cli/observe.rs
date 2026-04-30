//! `corvid observe` clap argument tree — slice 40 / observability
//! surface, decomposed in Phase 20j-A1.
//!
//! Owns the [`ObserveCommand`] subcommand enum that the
//! `corvid observe list|show|drift|explain|cost-optimise` dispatch
//! arms consume.

use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum ObserveCommand {
    /// List local lineage runs with costs, failures, approvals,
    /// and the slowest span per run.
    List {
        /// Trace directory. Defaults to `target/trace`.
        #[arg(long, value_name = "PATH")]
        trace_dir: Option<PathBuf>,
    },
    /// Explain one lineage run with contract-aware grouping.
    Show {
        /// Lineage trace identifier: either a direct file path, or a
        /// run id resolved as `<id>.lineage.jsonl` under `--trace-dir`.
        id_or_path: String,
        /// Trace directory used when `id_or_path` is a bare run id.
        /// Defaults to `target/trace`.
        #[arg(long, value_name = "PATH")]
        trace_dir: Option<PathBuf>,
    },
    /// Compare two lineage trace files or directories for production drift.
    Drift {
        /// Baseline lineage file or directory.
        baseline: PathBuf,
        /// Candidate lineage file or directory.
        candidate: PathBuf,
        /// Emit JSON for CI ingestion.
        #[arg(long)]
        json: bool,
    },
    /// AI-assisted root-cause for one trace. Walks the lineage,
    /// classifies the first non-OK event by typed status +
    /// guarantee_id, surfaces affected guarantees, and suggests
    /// next steps. The output's `sources` field carries the
    /// `(trace_id, span_id)` pairs the analysis consulted —
    /// the `Grounded<T>` shape.
    Explain {
        /// Trace identifier to explain.
        trace_id: String,
        /// Trace directory. Defaults to `target/trace`.
        #[arg(long, value_name = "PATH", default_value = "target/trace")]
        trace_dir: PathBuf,
    },
    /// AI-assisted cost optimisation for one agent. Aggregates
    /// cost-by-event-name across `--trace-dir`, identifies the
    /// top-N cost centres, and proposes typed suggestions
    /// (cache, skip-pre-validate, model-swap). Each suggestion
    /// carries `sources` linking back to the supporting events.
    CostOptimise {
        /// Agent name to analyse.
        agent: String,
        /// Trace directory. Defaults to `target/trace`.
        #[arg(long, value_name = "PATH", default_value = "target/trace")]
        trace_dir: PathBuf,
        /// Top-N cost centres to surface.
        #[arg(long, default_value = "5")]
        top_n: usize,
    },
}
