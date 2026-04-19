//! `corvid replay <trace>` — re-execute a Corvid program from a
//! recorded JSONL trace.
//!
//! In Phase 21 v1 the replay runtime is split across two slices:
//!   * `21-C-replay-interp` (Dev B) — swaps live LLM / tool /
//!     approve / seed / clock adapters for trace-substituting
//!     ones in the interpreter tier.
//!   * `21-C-replay-native` (Dev B) — same, in the native tier.
//!
//! Until those land, this command:
//!   1. Loads the trace and validates the schema header.
//!   2. Prints a summary (event count, run id, schema version).
//!   3. Returns exit code 1 with a clean "not yet available"
//!      message pointing at the pending runtime slice.
//!
//! Loading + validating is already useful as a pre-flight check —
//! catches corrupted traces and schema-version mismatches before
//! Dev B's replay engine ever sees the file.

use anyhow::{Context, Result};
use corvid_trace_schema::{
    read_events_from_path, schema_version_of, validate_supported_schema, TraceEvent,
};
use std::path::Path;

/// Exit code returned when the runtime replay path isn't available
/// yet. Callers (CI, tests) can distinguish "tool not implemented"
/// from "trace malformed" by checking this specific code.
pub const EXIT_NOT_IMPLEMENTED: u8 = 1;

/// Entry for `corvid replay <trace>`.
pub fn run_replay(trace: &Path) -> Result<u8> {
    let events = read_events_from_path(trace).with_context(|| {
        format!("failed to load trace at `{}`", trace.display())
    })?;

    if events.is_empty() {
        anyhow::bail!("trace `{}` is empty", trace.display());
    }

    validate_supported_schema(&events).with_context(|| {
        format!("trace `{}` uses an unsupported schema", trace.display())
    })?;

    print_summary(trace, &events);

    eprintln!();
    eprintln!(
        "note: `corvid replay` is not yet available. Replay-runtime support \
         ships in Phase 21 slice 21-C-replay-interp (interpreter tier) and \
         21-C-replay-native (native tier). Trace load + schema validation \
         succeeded above; once the runtime slices land, this command will \
         re-execute the program with recorded responses substituted for live \
         calls."
    );
    Ok(EXIT_NOT_IMPLEMENTED)
}

fn print_summary(trace: &Path, events: &[TraceEvent]) {
    let schema = schema_version_of(events)
        .map(|v| format!("v{v}"))
        .unwrap_or_else(|| "legacy (no header)".into());
    let run_id = run_id_of(events).unwrap_or("<unknown>");
    println!(
        "trace loaded: {} events, run_id={}, schema={}, path={}",
        events.len(),
        run_id,
        schema,
        trace.display()
    );
}

/// Pull the run_id from whichever event carries one first. Works
/// across legacy headerless traces and 21-A-schema-era traces with
/// a `SchemaHeader` up front.
fn run_id_of(events: &[TraceEvent]) -> Option<&str> {
    events.iter().find_map(|event| match event {
        TraceEvent::SchemaHeader { run_id, .. }
        | TraceEvent::RunStarted { run_id, .. }
        | TraceEvent::RunCompleted { run_id, .. }
        | TraceEvent::ToolCall { run_id, .. }
        | TraceEvent::ToolResult { run_id, .. }
        | TraceEvent::LlmCall { run_id, .. }
        | TraceEvent::LlmResult { run_id, .. }
        | TraceEvent::ApprovalRequest { run_id, .. }
        | TraceEvent::ApprovalResponse { run_id, .. }
        | TraceEvent::SeedRead { run_id, .. }
        | TraceEvent::ClockRead { run_id, .. }
        | TraceEvent::ModelSelected { run_id, .. }
        | TraceEvent::ProgressiveEscalation { run_id, .. }
        | TraceEvent::ProgressiveExhausted { run_id, .. }
        | TraceEvent::AbVariantChosen { run_id, .. }
        | TraceEvent::EnsembleVote { run_id, .. }
        | TraceEvent::AdversarialPipelineCompleted { run_id, .. }
        | TraceEvent::AdversarialContradiction { run_id, .. } => Some(run_id.as_str()),
    })
}
