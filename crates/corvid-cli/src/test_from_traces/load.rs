use anyhow::{Context, Result};
use corvid_trace_schema::{read_events_from_path, validate_supported_schema, TraceEvent};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// One trace file's summary after load + validation.
pub(super) struct LoadedTrace {
    pub(super) path: PathBuf,
    /// Unique prompt names seen in `LlmCall` events.
    pub(super) prompts: BTreeSet<String>,
    /// Unique tool names seen in `ToolCall` events.
    pub(super) tools: BTreeSet<String>,
    /// Unique approval labels seen in `ApprovalRequest` events.
    pub(super) approvals: BTreeSet<String>,
    /// True iff any `ApprovalRequest` event is present. Equivalent
    /// to "exercises a `@dangerous` tool" by the compiler's
    /// approve-before-dangerous guarantee.
    pub(super) has_approval_event: bool,
    /// Count of substitutable events (ToolCall + LlmCall +
    /// ApprovalRequest). Used for the execution plan preview.
    pub(super) llm_calls: usize,
    pub(super) tool_calls: usize,
    pub(super) approval_requests: usize,
    /// Maximum ts_ms across events in the trace. Used by `--since`.
    pub(super) max_ts_ms: u64,
}

pub(super) fn load_all_traces(dir: &Path) -> Result<Vec<LoadedTrace>> {
    let mut jsonl_files = Vec::new();
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("failed to read directory `{}`", dir.display()))?
    {
        let entry = entry.with_context(|| {
            format!("failed to read a directory entry under `{}`", dir.display())
        })?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            jsonl_files.push(path);
        }
    }
    jsonl_files.sort();

    let mut out = Vec::with_capacity(jsonl_files.len());
    for path in jsonl_files {
        let events = read_events_from_path(&path)
            .with_context(|| format!("failed to load trace `{}`", path.display()))?;
        if events.is_empty() {
            anyhow::bail!("trace `{}` is empty", path.display());
        }
        validate_supported_schema(&events)
            .with_context(|| format!("trace `{}` uses an unsupported schema", path.display()))?;
        out.push(summarize(&path, &events));
    }
    Ok(out)
}

fn summarize(path: &Path, events: &[TraceEvent]) -> LoadedTrace {
    let mut prompts = BTreeSet::new();
    let mut tools = BTreeSet::new();
    let mut approvals = BTreeSet::new();
    let mut llm_calls = 0usize;
    let mut tool_calls = 0usize;
    let mut approval_requests = 0usize;
    let mut max_ts_ms = 0u64;
    for event in events {
        let ts = event_ts_ms(event);
        if ts > max_ts_ms {
            max_ts_ms = ts;
        }
        match event {
            TraceEvent::LlmCall { prompt, .. } => {
                prompts.insert(prompt.clone());
                llm_calls += 1;
            }
            TraceEvent::ToolCall { tool, .. } => {
                tools.insert(tool.clone());
                tool_calls += 1;
            }
            TraceEvent::ApprovalRequest { label, .. } => {
                approvals.insert(label.clone());
                approval_requests += 1;
            }
            _ => {}
        }
    }
    LoadedTrace {
        path: path.to_path_buf(),
        has_approval_event: !approvals.is_empty(),
        prompts,
        tools,
        approvals,
        llm_calls,
        tool_calls,
        approval_requests,
        max_ts_ms,
    }
}

pub(super) fn parse_since(since: Option<&str>) -> Result<Option<u64>> {
    let Some(s) = since else {
        return Ok(None);
    };
    let ts = OffsetDateTime::parse(s, &Rfc3339)
        .with_context(|| format!("invalid --since timestamp `{s}`; expected RFC3339"))?;
    Ok(Some((ts.unix_timestamp_nanos() / 1_000_000) as u64))
}

/// Event timestamp extractor. Mirrors the same helper in
/// `trace_cmd.rs` / `routing_report.rs`; refactoring to a shared
/// crate-level module is out of scope for this slice.
fn event_ts_ms(event: &TraceEvent) -> u64 {
    match event {
        TraceEvent::SchemaHeader { ts_ms, .. }
        | TraceEvent::RunStarted { ts_ms, .. }
        | TraceEvent::RunCompleted { ts_ms, .. }
        | TraceEvent::ToolCall { ts_ms, .. }
        | TraceEvent::ToolResult { ts_ms, .. }
        | TraceEvent::LlmCall { ts_ms, .. }
        | TraceEvent::LlmResult { ts_ms, .. }
        | TraceEvent::PromptCache { ts_ms, .. }
        | TraceEvent::ApprovalRequest { ts_ms, .. }
        | TraceEvent::ApprovalDecision { ts_ms, .. }
        | TraceEvent::ApprovalResponse { ts_ms, .. }
        | TraceEvent::ApprovalTokenIssued { ts_ms, .. }
        | TraceEvent::ApprovalScopeViolation { ts_ms, .. }
        | TraceEvent::HumanInputRequest { ts_ms, .. }
        | TraceEvent::HumanInputResponse { ts_ms, .. }
        | TraceEvent::HumanChoiceRequest { ts_ms, .. }
        | TraceEvent::HumanChoiceResponse { ts_ms, .. }
        | TraceEvent::HostEvent { ts_ms, .. }
        | TraceEvent::SeedRead { ts_ms, .. }
        | TraceEvent::ClockRead { ts_ms, .. }
        | TraceEvent::ModelSelected { ts_ms, .. }
        | TraceEvent::ProgressiveEscalation { ts_ms, .. }
        | TraceEvent::ProgressiveExhausted { ts_ms, .. }
        | TraceEvent::StreamUpgrade { ts_ms, .. }
        | TraceEvent::AbVariantChosen { ts_ms, .. }
        | TraceEvent::EnsembleVote { ts_ms, .. }
        | TraceEvent::AdversarialPipelineCompleted { ts_ms, .. }
        | TraceEvent::AdversarialContradiction { ts_ms, .. }
        | TraceEvent::ProvenanceEdge { ts_ms, .. } => *ts_ms,
    }
}
