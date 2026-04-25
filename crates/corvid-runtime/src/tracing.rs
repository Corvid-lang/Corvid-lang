//! JSONL trace emission.
//!
//! Every interesting runtime event becomes a JSON object on its own line,
//! appended to `target/trace/<run_id>.jsonl`. Trace failures are swallowed:
//! a broken tracer must never crash an agent.
//!
//! Event shape is intentionally identical to the Python runtime's so the
//! same downstream tooling reads both.

use crate::redact::RedactionSet;
use crate::record::writer::JsonlTraceWriter;
use corvid_trace_schema::TraceEvent;
use std::sync::atomic::{AtomicU64, Ordering};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

static BENCH_TRACE_OVERHEAD_NS: AtomicU64 = AtomicU64::new(0);
static DETERMINISTIC_CLOCK_COUNTER: AtomicU64 = AtomicU64::new(0);

fn profile_runtime_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("CORVID_PROFILE_RUNTIME").ok().as_deref() == Some("1"))
}

pub fn bench_trace_overhead_ns() -> u64 {
    BENCH_TRACE_OVERHEAD_NS.load(Ordering::Relaxed)
}

/// JSONL appender. Cheap to clone (shared file handle behind a mutex).
#[derive(Clone)]
pub struct Tracer {
    inner: std::sync::Arc<TracerInner>,
}

struct TracerInner {
    run_id: String,
    writer: JsonlTraceWriter,
    redaction: RedactionSet,
}

impl Tracer {
    /// Open a trace file under `<trace_dir>/<run_id>.jsonl`. Failure to
    /// create the file is logged once and silently demoted — tracing must
    /// never crash an agent.
    pub fn open(trace_dir: &Path, run_id: impl Into<String>) -> Self {
        let run_id = run_id.into();
        let path = trace_dir.join(format!("{run_id}.jsonl"));
        Self::open_path(path, run_id)
    }

    /// Open a trace file at an exact path. Used by the differential
    /// verifier to force native-tier runs to write a known trace file.
    pub fn open_path(path: impl Into<PathBuf>, run_id: impl Into<String>) -> Self {
        let run_id = run_id.into();
        let path = path.into();
        Self {
            inner: std::sync::Arc::new(TracerInner {
                run_id,
                writer: JsonlTraceWriter::open(path),
                redaction: RedactionSet::empty(),
            }),
        }
    }

    /// Tracer that writes nowhere — useful for tests and embedding
    /// scenarios where the host owns observability.
    pub fn null() -> Self {
        Self {
            inner: std::sync::Arc::new(TracerInner {
                run_id: "null".into(),
                writer: JsonlTraceWriter::open(PathBuf::new()),
                redaction: RedactionSet::empty(),
            }),
        }
    }

    /// Attach a `RedactionSet`. Call before any emit. Returns a new
    /// `Tracer` so the redaction set can change without sharing mutable
    /// state across handles.
    pub fn with_redaction(self, redaction: RedactionSet) -> Self {
        let inner = match std::sync::Arc::try_unwrap(self.inner) {
            Ok(inner) => TracerInner {
                run_id: inner.run_id,
                writer: inner.writer,
                redaction,
            },
            Err(arc) => {
                // The Tracer was already cloned; we can't mutate. Create
                // a sibling that shares nothing — caller should call
                // `with_redaction` immediately after `open()` before
                // cloning. (No file handle, so emits become no-ops.)
                TracerInner {
                    run_id: arc.run_id.clone(),
                    writer: JsonlTraceWriter::open(PathBuf::new()),
                    redaction,
                }
            }
        };
        Self {
            inner: std::sync::Arc::new(inner),
        }
    }

    pub fn run_id(&self) -> &str {
        &self.inner.run_id
    }

    pub fn path(&self) -> &Path {
        self.inner.writer.path()
    }

    pub fn is_enabled(&self) -> bool {
        self.inner.writer.is_enabled()
    }

    pub(crate) fn writer(&self) -> JsonlTraceWriter {
        self.inner.writer.clone()
    }

    /// Append an event. IO errors are swallowed. Args inside the event
    /// are passed through the redaction set before serialization.
    pub fn emit(&self, event: TraceEvent) {
        let profile_start = if profile_runtime_enabled() {
            Some(std::time::Instant::now())
        } else {
            None
        };
        if !self.is_enabled() {
            return;
        }
        let event = self.apply_redaction(event);
        self.inner.writer.append(&event);
        if let Some(start) = profile_start {
            BENCH_TRACE_OVERHEAD_NS.fetch_add(
                start.elapsed().as_nanos() as u64,
                Ordering::Relaxed,
            );
        }
    }

    fn apply_redaction(&self, event: TraceEvent) -> TraceEvent {
        if self.inner.redaction.is_empty() {
            return event;
        }
        let r = &self.inner.redaction;
        match event {
            TraceEvent::ToolCall {
                ts_ms,
                run_id,
                tool,
                args,
            } => TraceEvent::ToolCall {
                ts_ms,
                run_id,
                tool,
                args: r.redact_args(args),
            },
            TraceEvent::RunStarted {
                ts_ms,
                run_id,
                agent,
                args,
            } => TraceEvent::RunStarted {
                ts_ms,
                run_id,
                agent,
                args: r.redact_args(args),
            },
            TraceEvent::RunCompleted {
                ts_ms,
                run_id,
                ok,
                result,
                error,
            } => TraceEvent::RunCompleted {
                ts_ms,
                run_id,
                ok,
                result: result.map(|value| r.redact(value)),
                error,
            },
            TraceEvent::ToolResult {
                ts_ms,
                run_id,
                tool,
                result,
            } => TraceEvent::ToolResult {
                ts_ms,
                run_id,
                tool,
                result: r.redact(result),
            },
            TraceEvent::LlmResult {
                ts_ms,
                run_id,
                prompt,
                model,
                model_version,
                result,
            } => TraceEvent::LlmResult {
                ts_ms,
                run_id,
                prompt,
                model,
                model_version,
                result: r.redact(result),
            },
            TraceEvent::LlmCall {
                ts_ms,
                run_id,
                prompt,
                model,
                model_version,
                rendered,
                args,
            } => TraceEvent::LlmCall {
                ts_ms,
                run_id,
                prompt,
                model,
                model_version,
                rendered: rendered.map(|s| redact_string(&r.redact(serde_json::Value::String(s)))),
                args: r.redact_args(args),
            },
            TraceEvent::ApprovalRequest {
                ts_ms,
                run_id,
                label,
                args,
            } => TraceEvent::ApprovalRequest {
                ts_ms,
                run_id,
                label,
                args: r.redact_args(args),
            },
            TraceEvent::HostEvent {
                ts_ms,
                run_id,
                name,
                payload,
            } => TraceEvent::HostEvent {
                ts_ms,
                run_id,
                name,
                payload: r.redact(payload),
            },
            other => other,
        }
    }
}

fn redact_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Wall-clock millisecond timestamp. Used by event constructors.
pub fn now_ms() -> u64 {
    if let Some(seed) = deterministic_seed() {
        return seed + DETERMINISTIC_CLOCK_COUNTER.fetch_add(1, Ordering::Relaxed);
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Generate a run id from the current wall clock. Good enough for v0.5 —
/// uniqueness inside a single process. UUIDs arrive when we need them.
pub fn fresh_run_id() -> String {
    format!("run-{}", now_ms())
}

pub fn deterministic_seed() -> Option<u64> {
    static SEED: OnceLock<Option<u64>> = OnceLock::new();
    *SEED.get_or_init(|| {
        std::env::var("CORVID_DETERMINISTIC_SEED")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn writes_events_to_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let tracer = Tracer::open(dir.path(), "run-test");
        tracer.emit(TraceEvent::RunStarted {
            ts_ms: 1,
            run_id: "run-test".into(),
            agent: "demo".into(),
            args: vec![json!("arg")],
        });
        tracer.emit(TraceEvent::ToolCall {
            ts_ms: 2,
            run_id: "run-test".into(),
            tool: "double".into(),
            args: vec![json!(21)],
        });
        drop(tracer);

        let path = dir.path().join("run-test.jsonl");
        let body = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"kind\":\"run_started\""));
        assert!(lines[1].contains("\"tool\":\"double\""));
    }

    #[test]
    fn writes_events_to_explicit_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("trace.jsonl");
        let tracer = Tracer::open_path(&path, "run-explicit");
        tracer.emit(TraceEvent::RunCompleted {
            ts_ms: 1,
            run_id: "run-explicit".into(),
            ok: true,
            result: None,
            error: None,
        });
        drop(tracer);

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("\"kind\":\"run_completed\""));
    }

    #[test]
    fn null_tracer_is_a_noop() {
        let tracer = Tracer::null();
        tracer.emit(TraceEvent::RunCompleted {
            ts_ms: 1,
            run_id: "x".into(),
            ok: true,
            result: None,
            error: None,
        });
        // No panic, no file. Success.
    }

    #[test]
    fn missing_dir_is_swallowed() {
        // Open under a deeply nested path that does exist (we create it),
        // then prove emit doesn't panic if the file mutex is empty after a
        // hypothetical failure. We simulate by using `Tracer::null`.
        let tracer = Tracer::null();
        tracer.emit(TraceEvent::RunStarted {
            ts_ms: 0,
            run_id: "z".into(),
            agent: "a".into(),
            args: vec![],
        });
    }
}
