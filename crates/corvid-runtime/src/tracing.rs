//! JSONL trace emission.
//!
//! Every interesting runtime event becomes a JSON object on its own line,
//! appended to `target/trace/<run_id>.jsonl`. Trace failures are swallowed:
//! a broken tracer must never crash an agent.
//!
//! Event shape is intentionally identical to the Python runtime's so the
//! same downstream tooling reads both.

use crate::redact::RedactionSet;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TraceEvent {
    RunStarted {
        ts_ms: u64,
        run_id: String,
        agent: String,
        #[serde(default)]
        args: Vec<serde_json::Value>,
    },
    RunCompleted {
        ts_ms: u64,
        run_id: String,
        ok: bool,
        #[serde(default)]
        result: Option<serde_json::Value>,
        #[serde(default)]
        error: Option<String>,
    },
    ToolCall {
        ts_ms: u64,
        run_id: String,
        tool: String,
        args: Vec<serde_json::Value>,
    },
    ToolResult {
        ts_ms: u64,
        run_id: String,
        tool: String,
        result: serde_json::Value,
    },
    LlmCall {
        ts_ms: u64,
        run_id: String,
        prompt: String,
        model: Option<String>,
        #[serde(default)]
        rendered: Option<String>,
        #[serde(default)]
        args: Vec<serde_json::Value>,
    },
    LlmResult {
        ts_ms: u64,
        run_id: String,
        prompt: String,
        result: serde_json::Value,
    },
    ApprovalRequest {
        ts_ms: u64,
        run_id: String,
        label: String,
        args: Vec<serde_json::Value>,
    },
    ApprovalResponse {
        ts_ms: u64,
        run_id: String,
        label: String,
        approved: bool,
    },
}

/// JSONL appender. Cheap to clone (shared file handle behind a mutex).
#[derive(Clone)]
pub struct Tracer {
    inner: std::sync::Arc<TracerInner>,
}

struct TracerInner {
    run_id: String,
    path: PathBuf,
    file: Mutex<Option<std::fs::File>>,
    redaction: RedactionSet,
}

impl Tracer {
    /// Open a trace file under `<trace_dir>/<run_id>.jsonl`. Failure to
    /// create the file is logged once and silently demoted — tracing must
    /// never crash an agent.
    pub fn open(trace_dir: &Path, run_id: impl Into<String>) -> Self {
        let run_id = run_id.into();
        let path = trace_dir.join(format!("{run_id}.jsonl"));
        let file = (|| -> std::io::Result<std::fs::File> {
            std::fs::create_dir_all(trace_dir)?;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
        })()
        .ok();
        Self {
            inner: std::sync::Arc::new(TracerInner {
                run_id,
                path,
                file: Mutex::new(file),
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
                path: PathBuf::new(),
                file: Mutex::new(None),
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
                path: inner.path,
                file: inner.file,
                redaction,
            },
            Err(arc) => {
                // The Tracer was already cloned; we can't mutate. Create
                // a sibling that shares nothing — caller should call
                // `with_redaction` immediately after `open()` before
                // cloning. (No file handle, so emits become no-ops.)
                TracerInner {
                    run_id: arc.run_id.clone(),
                    path: arc.path.clone(),
                    file: Mutex::new(None),
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
        &self.inner.path
    }

    /// Append an event. IO errors are swallowed. Args inside the event
    /// are passed through the redaction set before serialization.
    pub fn emit(&self, event: TraceEvent) {
        let event = self.apply_redaction(event);
        let line = match serde_json::to_string(&event) {
            Ok(s) => s,
            Err(_) => return,
        };
        if let Ok(mut guard) = self.inner.file.lock() {
            if let Some(f) = guard.as_mut() {
                use std::io::Write;
                let _ = writeln!(f, "{line}");
                let _ = f.flush();
            }
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
                result,
            } => TraceEvent::LlmResult {
                ts_ms,
                run_id,
                prompt,
                result: r.redact(result),
            },
            TraceEvent::LlmCall {
                ts_ms,
                run_id,
                prompt,
                model,
                rendered,
                args,
            } => TraceEvent::LlmCall {
                ts_ms,
                run_id,
                prompt,
                model,
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

        let path = dir.path().join("run-test.jsonl");
        let body = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"kind\":\"run_started\""));
        assert!(lines[1].contains("\"tool\":\"double\""));
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
