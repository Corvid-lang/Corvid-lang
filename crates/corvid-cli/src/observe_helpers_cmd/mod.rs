//! AI-assisted observability + eval helpers — slice 40K,
//! decomposed in Phase 20j-S3.
//!
//! The Phase 40 phase-done checklist names four helper
//! subcommands the developer-flow doc shows operators running:
//!
//!   `corvid observe explain <trace-id>`         — RAG-grounded
//!     incident root cause from a typed lineage trace.
//!   `corvid observe cost-optimise <agent>`       — generative
//!     route/escalate/cache suggestions from cost rollup.
//!   `corvid eval drift --explain`                — decompose
//!     drift between two trace runs into model / input /
//!     prompt / retrieval contributions.
//!   `corvid eval generate-from-feedback <id>`    — eval fixture
//!     synthesised from a "wrong answer" feedback record.
//!
//! Each ships in two layers:
//!
//!   1. A deterministic Rust handler that produces the structured
//!      output (`*Report`/`*Plan`/`*Attribution`/`EvalFixture`)
//!      from the lineage store. This is the always-available path
//!      — no LLM key required.
//!
//!   2. A paired `.cor` source under
//!      `examples/observe_helpers/` documenting the
//!      `Grounded<T>`-shaped LLM-grounded version: typed effects,
//!      `@budget`, the prompt's `cites strictly` clause, and the
//!      `Grounded<…>` return type. Production deployments wire the
//!      `.cor` program through the project's configured LLM
//!      adapter; the heuristic stays as the deterministic
//!      fallback so the helpers remain useful in CI and offline.
//!
//! Each output carries a `sources` array (the `Grounded<T>` shape
//! at the JSON layer) listing the trace_id + span_id of every
//! lineage event the helper consulted. A downstream consumer can
//! `JOIN` against the trace store to reconstruct the evidence
//! the analysis rests on.
//!
//! The module is split per CLI surface (Phase 20j-S3):
//!
//! - [`observe_explain`] — `corvid observe explain <trace-id>`.
//! - [`cost_optimise`] — `corvid observe cost-optimise <agent>`.
//! - [`eval_drift`] — `corvid eval drift --explain`.
//! - [`eval_from_feedback`] —
//!   `corvid eval generate-from-feedback <id>`.
//!
//! Shared lineage I/O (`read_lineage_input`, `source_descriptor`,
//! `select_run`) lives in this file because every leaf consumes
//! it. `test_support` (`#[cfg(test)]` only) holds the
//! `LineageEvent` constructors the per-leaf test modules share.

pub mod cost_optimise;
pub mod eval_drift;
pub mod eval_from_feedback;
pub mod observe_explain;

#[allow(unused_imports)]
pub use cost_optimise::*;
#[allow(unused_imports)]
pub use eval_drift::*;
#[allow(unused_imports)]
pub use eval_from_feedback::*;
#[allow(unused_imports)]
pub use observe_explain::*;

#[cfg(test)]
mod test_support;

use anyhow::{Context, Result};
use corvid_runtime::lineage::LineageEvent;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

/// Read a lineage JSONL file or a directory of them. Mirrors the
/// existing `observe_cmd::read_lineage_input` shape.
pub fn read_lineage_input(path: &Path) -> Result<Vec<LineageEvent>> {
    if path.is_dir() {
        let mut events = Vec::new();
        let mut entries = fs::read_dir(path)
            .with_context(|| format!("read_dir `{}`", path.display()))?
            .map(|e| e.map(|e| e.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        entries.sort();
        for entry in entries {
            if entry.extension().and_then(|s| s.to_str()) == Some("jsonl")
                || entry
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.ends_with(".lineage.jsonl"))
                    .unwrap_or(false)
            {
                events.extend(read_lineage_file(&entry)?);
            }
        }
        return Ok(events);
    }
    read_lineage_file(path)
}

fn read_lineage_file(path: &Path) -> Result<Vec<LineageEvent>> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("reading lineage from `{}`", path.display()))?;
    let mut events = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let event: LineageEvent = serde_json::from_str(trimmed)
            .with_context(|| format!("line {} is not a lineage event", i + 1))?;
        events.push(event);
    }
    Ok(events)
}

pub(crate) fn source_descriptor(event: &LineageEvent) -> Value {
    json!({
        "trace_id": event.trace_id,
        "span_id": event.span_id,
        "kind": event.kind,
        "name": event.name,
    })
}

pub(crate) fn select_run(events: &[LineageEvent], trace_id: &str) -> Vec<LineageEvent> {
    events
        .iter()
        .filter(|e| e.trace_id == trace_id)
        .cloned()
        .collect()
}
