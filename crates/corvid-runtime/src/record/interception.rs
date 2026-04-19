use crate::tracing::now_ms;
use corvid_trace_schema::{TraceEvent, SCHEMA_VERSION, WRITER_INTERPRETER};

pub(crate) fn schema_header(run_id: &str, commit_sha: Option<String>) -> TraceEvent {
    TraceEvent::SchemaHeader {
        version: SCHEMA_VERSION,
        writer: WRITER_INTERPRETER.to_string(),
        commit_sha,
        ts_ms: now_ms(),
        run_id: run_id.to_string(),
    }
}

pub(crate) fn seed_read(run_id: &str, purpose: &str, value: u64) -> TraceEvent {
    TraceEvent::SeedRead {
        ts_ms: now_ms(),
        run_id: run_id.to_string(),
        purpose: purpose.to_string(),
        value,
    }
}

pub(crate) fn clock_read(run_id: &str, source: &str, value: i64) -> TraceEvent {
    TraceEvent::ClockRead {
        ts_ms: now_ms(),
        run_id: run_id.to_string(),
        source: source.to_string(),
        value,
    }
}
