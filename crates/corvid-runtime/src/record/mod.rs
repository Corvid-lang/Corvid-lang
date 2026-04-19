mod interception;
pub(crate) mod writer;

use std::path::Path;

use crate::tracing::Tracer;
use writer::JsonlTraceWriter;

#[derive(Clone)]
pub struct Recorder {
    run_id: String,
    writer: JsonlTraceWriter,
}

impl Recorder {
    pub fn for_tracer(tracer: &Tracer) -> Option<Self> {
        if !tracer.is_enabled() {
            return None;
        }
        Some(Self::from_writer(tracer.writer(), tracer.run_id()))
    }

    pub fn open(path: &Path, run_id: impl Into<String>) -> Self {
        Self::from_writer(writer::JsonlTraceWriter::open(path), run_id)
    }

    fn from_writer(writer: JsonlTraceWriter, run_id: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
            writer,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.writer.is_enabled()
    }

    pub fn path(&self) -> &Path {
        self.writer.path()
    }

    pub fn emit_schema_header(&self) {
        let event = interception::schema_header(&self.run_id, build_commit_sha());
        self.writer.append(&event);
    }

    pub fn emit_seed_read(&self, purpose: &str, value: u64) {
        let event = interception::seed_read(&self.run_id, purpose, value);
        self.writer.append(&event);
    }

    pub fn emit_clock_read(&self, source: &str, value: i64) {
        let event = interception::clock_read(&self.run_id, source, value);
        self.writer.append(&event);
    }
}

fn build_commit_sha() -> Option<String> {
    option_env!("CORVID_GIT_SHA")
        .or(option_env!("GIT_SHA"))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::Recorder;
    use corvid_trace_schema::{read_events_from_path, validate_supported_schema, TraceEvent, SCHEMA_VERSION, WRITER_INTERPRETER};

    #[test]
    fn recorder_writes_header_and_seed_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("record.jsonl");
        let recorder = Recorder::open(&path, "run-record");
        recorder.emit_schema_header();
        recorder.emit_seed_read("rollout_cohort", 42);
        recorder.emit_clock_read("wall", 123);
        drop(recorder);

        let events = read_events_from_path(&path).unwrap();
        validate_supported_schema(&events).unwrap();
        match &events[0] {
            TraceEvent::SchemaHeader {
                version,
                writer,
                run_id,
                ..
            } => {
                assert_eq!(*version, SCHEMA_VERSION);
                assert_eq!(writer, WRITER_INTERPRETER);
                assert_eq!(run_id, "run-record");
            }
            other => panic!("expected schema header, got {other:?}"),
        }
        match &events[1] {
            TraceEvent::SeedRead { purpose, value, .. } => {
                assert_eq!(purpose, "rollout_cohort");
                assert_eq!(*value, 42);
            }
            other => panic!("expected seed read, got {other:?}"),
        }
        match &events[2] {
            TraceEvent::ClockRead { source, value, .. } => {
                assert_eq!(source, "wall");
                assert_eq!(*value, 123);
            }
            other => panic!("expected clock read, got {other:?}"),
        }
    }
}
