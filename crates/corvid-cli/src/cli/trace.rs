use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum TraceCommand {
    /// List every JSONL trace under `--trace-dir` (default:
    /// `target/trace/`). One row per trace with run id, schema
    /// version, event count, and timestamp range.
    List {
        /// Trace directory. Defaults to `target/trace`.
        #[arg(long, value_name = "PATH")]
        trace_dir: Option<PathBuf>,
    },
    /// Print every event in a trace as formatted JSON, one
    /// event per line.
    Show {
        /// Trace identifier: either a direct file path, or a
        /// run id to resolve under `--trace-dir`.
        id_or_path: String,
        /// Trace directory used when `id_or_path` is a bare
        /// run id. Defaults to `target/trace`.
        #[arg(long, value_name = "PATH")]
        trace_dir: Option<PathBuf>,
    },
    /// Render the Grounded<T> provenance DAG of a trace as a
    /// Graphviz DOT graph. Pipe into `dot -Tsvg > prov.svg` to
    /// render. Traces without provenance events produce an empty
    /// digraph plus a warning on stderr.
    Dag {
        /// Trace identifier: either a direct file path, or a
        /// run id to resolve under `--trace-dir`.
        id_or_path: String,
        /// Trace directory used when `id_or_path` is a bare
        /// run id. Defaults to `target/trace`.
        #[arg(long, value_name = "PATH")]
        trace_dir: Option<PathBuf>,
    },
    /// Render a Phase 40 lineage JSONL trace as an indented tree.
    Lineage {
        /// Lineage trace identifier: either a direct file path, or a
        /// run id resolved as `<id>.lineage.jsonl` under `--trace-dir`.
        id_or_path: String,
        /// Trace directory used when `id_or_path` is a bare run id.
        /// Defaults to `target/trace`.
        #[arg(long, value_name = "PATH")]
        trace_dir: Option<PathBuf>,
    },
}
