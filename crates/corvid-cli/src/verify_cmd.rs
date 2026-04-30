//! `corvid verify` CLI dispatch — slice 18 / corpus-verification
//! surface, decomposed in Phase 20j-A1.
//!
//! Two modes the dispatch arm picks between:
//!
//! - `--corpus <dir>` runs every program under the directory
//!   through the differential verifier, prints a grid summary,
//!   and emits per-program detail for any divergent run.
//! - `--shrink <file>` minimises a failing reproducer.
//!
//! All actual work happens in `corvid-differential-verify`; this
//! module only owns the CLI-shape dispatch + JSON rendering.

use anyhow::Result;
use corvid_differential_verify::{
    render_corpus_grid, render_report, shrink_program, verify_corpus,
};
use std::path::Path;

pub(crate) fn cmd_verify(corpus: Option<&Path>, shrink: Option<&Path>, json: bool) -> Result<u8> {
    match (corpus, shrink) {
        (Some(dir), None) => {
            let reports = verify_corpus(dir)?;
            let divergent: Vec<_> = reports
                .iter()
                .filter(|report| !report.divergences.is_empty())
                .collect();
            println!("{}", render_corpus_grid(&reports));
            if !divergent.is_empty() {
                println!();
                for (index, report) in divergent.iter().enumerate() {
                    if index > 0 {
                        println!();
                    }
                    println!("{}", render_report(report));
                }
            }
            if json {
                eprintln!("{}", serde_json::to_string_pretty(&reports)?);
            }
            Ok(if divergent.is_empty() { 0 } else { 1 })
        }
        (None, Some(file)) => {
            let result = shrink_program(file)?;
            println!(
                "shrunk reproducer: {} -> {} (removed {} line(s))",
                result.original.display(),
                result.output.display(),
                result.removed_lines
            );
            if json {
                eprintln!("{}", serde_json::to_string_pretty(&result)?);
            }
            Ok(0)
        }
        (None, None) => {
            anyhow::bail!("use `corvid verify --corpus <dir>` or `corvid verify --shrink <file>`")
        }
        (Some(_), Some(_)) => unreachable!("clap enforces conflicts"),
    }
}
