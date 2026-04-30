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
//! - The cost-optimise / drift / from-feedback surfaces stay in
//!   this file mid-refactor; commits 20j-S3 #2/#3/#4 relocate
//!   them.

pub mod cost_optimise;
pub mod eval_drift;
pub mod observe_explain;
#[allow(unused_imports)]
pub use cost_optimise::*;
#[allow(unused_imports)]
pub use eval_drift::*;
#[allow(unused_imports)]
pub use observe_explain::*;

#[cfg(test)]
mod test_support;

use anyhow::{anyhow, Context, Result};
use corvid_runtime::lineage::LineageEvent;
use corvid_runtime::lineage_redact::{redact_lineage_events, LineageRedactionPolicy};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------

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

// ---------------------------------------------------------------
// 4. eval generate-from-feedback — synthesised eval fixture
// ---------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct EvalFromFeedbackArgs {
    pub trace_dir: PathBuf,
    pub feedback_file: PathBuf,
    pub out: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvalFixture {
    pub fixture_id: String,
    pub trace_id: String,
    pub feedback_kind: String,
    pub user_correction: String,
    pub redacted_lineage_count: usize,
    pub sources: Vec<Value>,
    pub redaction_policy: String,
    pub fixture_path: Option<PathBuf>,
}

/// Read a feedback record (a JSON file with `trace_id`,
/// `feedback_kind` ∈ {`wrong_answer`, `unsafe_action`,
/// `low_confidence`, …}, `user_correction`), look up the named
/// trace, redact PII via the production redaction policy, write a
/// `corvid eval promote`-shaped fixture (`.eval.json`).
pub fn run_eval_generate_from_feedback(
    args: EvalFromFeedbackArgs,
) -> Result<EvalFixture> {
    let feedback_text = fs::read_to_string(&args.feedback_file).with_context(|| {
        format!(
            "reading feedback record from `{}`",
            args.feedback_file.display()
        )
    })?;
    let feedback: Value = serde_json::from_str(&feedback_text)
        .with_context(|| "feedback file is not JSON")?;
    let trace_id = feedback
        .get("trace_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("feedback record must include `trace_id`"))?
        .to_string();
    let feedback_kind = feedback
        .get("feedback_kind")
        .and_then(|v| v.as_str())
        .unwrap_or("wrong_answer")
        .to_string();
    let user_correction = feedback
        .get("user_correction")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let events = read_lineage_input(&args.trace_dir)?;
    let run = select_run(&events, &trace_id);
    if run.is_empty() {
        return Err(anyhow!(
            "no lineage events found for trace `{}` referenced by feedback",
            trace_id
        ));
    }
    let policy = LineageRedactionPolicy::production_default();
    let redacted = redact_lineage_events(&run, &policy);

    let fixture_id = format!(
        "eval-from-feedback-{}-{}",
        feedback_kind,
        &trace_id.chars().take(12).collect::<String>()
    );
    let sources: Vec<Value> = redacted.iter().map(source_descriptor).collect();

    let fixture_path = if let Some(path) = &args.out {
        let body = json!({
            "fixture_id": fixture_id,
            "kind": "eval_from_feedback",
            "trace_id": trace_id,
            "feedback_kind": feedback_kind,
            "user_correction": user_correction,
            "redaction_policy": policy.name,
            "lineage_events": redacted,
            "sources": sources,
        });
        fs::write(path, serde_json::to_string_pretty(&body)?)
            .with_context(|| format!("writing eval fixture to `{}`", path.display()))?;
        Some(path.clone())
    } else {
        None
    };

    Ok(EvalFixture {
        fixture_id,
        trace_id,
        feedback_kind,
        user_correction,
        redacted_lineage_count: redacted.len(),
        sources,
        redaction_policy: policy.name,
        fixture_path,
    })
}

// ---------------------------------------------------------------
// Tests
// ---------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe_helpers_cmd::test_support::{ev, write_lineage};
    use corvid_runtime::lineage::{LineageKind, LineageStatus};

    /// Slice 40K: eval generate-from-feedback reads a feedback
    /// record, redacts the matching trace, writes a typed fixture
    /// to disk, and the fixture's `sources` array carries the
    /// `(trace_id, span_id)` pairs of every redacted event.
    #[test]
    fn eval_generate_from_feedback_writes_redacted_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let trace_path = dir.path().join("trace.lineage.jsonl");
        write_lineage(
            &trace_path,
            &[ev(
                LineageKind::Prompt,
                "decide",
                "t1",
                "s1",
                LineageStatus::Ok,
                "",
                0.01,
            )],
        );
        let feedback_path = dir.path().join("feedback.json");
        fs::write(
            &feedback_path,
            r#"{"trace_id":"t1","feedback_kind":"wrong_answer","user_correction":"refund the order"}"#,
        )
        .unwrap();
        let out_path = dir.path().join("fixture.eval.json");
        let fixture = run_eval_generate_from_feedback(EvalFromFeedbackArgs {
            trace_dir: dir.path().to_path_buf(),
            feedback_file: feedback_path,
            out: Some(out_path.clone()),
        })
        .unwrap();
        assert_eq!(fixture.feedback_kind, "wrong_answer");
        assert_eq!(fixture.redacted_lineage_count, 1);
        assert!(out_path.exists());
        let written = fs::read_to_string(&out_path).unwrap();
        let parsed: Value = serde_json::from_str(&written).unwrap();
        assert_eq!(parsed["fixture_id"], fixture.fixture_id);
        assert_eq!(parsed["trace_id"], "t1");
        assert!(parsed["sources"].as_array().unwrap().len() == 1);
        // The redacted lineage must NOT contain the raw tenant id —
        // the production redaction policy hashes it.
        assert!(!written.contains("\"tenant_id\":\"t1\""));
    }

    /// Slice 40K adversarial: missing `trace_id` in the feedback
    /// record is refused with a clear diagnostic.
    #[test]
    fn eval_generate_from_feedback_missing_trace_id_refused() {
        let dir = tempfile::tempdir().unwrap();
        let feedback_path = dir.path().join("feedback.json");
        fs::write(&feedback_path, r#"{"feedback_kind":"wrong_answer"}"#).unwrap();
        let err = run_eval_generate_from_feedback(EvalFromFeedbackArgs {
            trace_dir: dir.path().to_path_buf(),
            feedback_file: feedback_path,
            out: None,
        })
        .unwrap_err();
        assert!(err.to_string().contains("trace_id"));
    }
}
