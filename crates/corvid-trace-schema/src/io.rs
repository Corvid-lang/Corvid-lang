//! JSONL read/write helpers for `TraceEvent` streams.
//!
//! The recording tier streams events to a file one-per-line; the
//! replay tier reads them back. Both sides of the pipeline agree
//! on: UTF-8, `\n` line separator, one JSON object per line, no
//! trailing comma. No framing, no compression, no schema
//! negotiation beyond the `SchemaHeader` event at the top of
//! every trace.

use crate::event::TraceEvent;
use serde::de::Error as _;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;

/// Result of reading a single line. Separates structural failures
/// (IO / invalid JSON / unknown kind) from the normal successful
/// read, so callers can surface partial traces rather than aborting
/// on the first corrupted line.
#[derive(Debug)]
pub enum ReadError {
    /// The underlying reader failed.
    Io(std::io::Error),
    /// A line didn't parse as JSON or didn't match any known
    /// `TraceEvent` variant. Includes the line number (1-based)
    /// and the raw text for diagnostics.
    Parse {
        line_number: usize,
        line: String,
        source: serde_json::Error,
    },
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io error reading trace: {e}"),
            Self::Parse {
                line_number,
                source,
                ..
            } => write!(
                f,
                "trace line {line_number} failed to parse as TraceEvent: {source}"
            ),
        }
    }
}

impl std::error::Error for ReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Parse { source, .. } => Some(source),
        }
    }
}

impl From<std::io::Error> for ReadError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Serialize a single event as one JSONL line (without the
/// trailing `\n`). The writer side is responsible for appending
/// the newline — that lets callers batch their own flushing.
pub fn serialize_event_line(event: &TraceEvent) -> Result<String, serde_json::Error> {
    serde_json::to_string(event)
}

/// Parse a single JSONL line into a `TraceEvent`. Blank lines
/// return `None` so callers can treat them as "no event".
pub fn parse_event_line(line: &str) -> Result<Option<TraceEvent>, serde_json::Error> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    serde_json::from_str::<TraceEvent>(trimmed).map(Some)
}

/// Append a single event as one line (with `\n`) to `writer`.
pub fn append_event<W: Write>(writer: &mut W, event: &TraceEvent) -> Result<(), std::io::Error> {
    let line = serialize_event_line(event)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    writer.write_all(line.as_bytes())?;
    writer.write_all(b"\n")?;
    Ok(())
}

/// Write every event in `events` to `path`, one per line.
/// Overwrites the file if it already exists.
pub fn write_events_to_path(
    path: &Path,
    events: &[TraceEvent],
) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let file = std::fs::File::create(path)?;
    let mut writer = BufWriter::new(file);
    for event in events {
        append_event(&mut writer, event)?;
    }
    writer.flush()?;
    Ok(())
}

/// Read every event from a JSONL `reader`. Blank lines are skipped.
/// A structural failure on any line returns immediately with
/// `ReadError::Parse` carrying the line number and raw text.
pub fn read_events<R: Read>(reader: R) -> Result<Vec<TraceEvent>, ReadError> {
    let mut out = Vec::new();
    for (line_idx, line) in BufReader::new(reader).lines().enumerate() {
        let line = line?;
        let line_number = line_idx + 1;
        match parse_event_line(&line) {
            Ok(Some(event)) => out.push(event),
            Ok(None) => continue,
            Err(source) => {
                return Err(ReadError::Parse {
                    line_number,
                    line,
                    source,
                });
            }
        }
    }
    Ok(out)
}

/// Convenience: read every event from a trace file at `path`.
pub fn read_events_from_path(path: &Path) -> Result<Vec<TraceEvent>, ReadError> {
    let file = std::fs::File::open(path)?;
    read_events(file)
}

/// Pull the `SchemaHeader` version out of a trace, if present.
/// Returns `None` when the trace is empty or doesn't start with a
/// `SchemaHeader` (which is the case for pre-21-A-schema traces
/// that started with `RunStarted` directly).
pub fn schema_version_of(events: &[TraceEvent]) -> Option<u32> {
    match events.first() {
        Some(TraceEvent::SchemaHeader { version, .. }) => Some(*version),
        _ => None,
    }
}

/// Check whether a trace carries a `SchemaHeader` whose version
/// the current binary understands. Returns the first unsupported
/// version encountered, or `Ok(())` if every header is compatible.
/// Traces with no header pass silently — they're legacy but not
/// structurally malformed.
pub fn validate_supported_schema(events: &[TraceEvent]) -> Result<(), UnsupportedSchema> {
    for event in events {
        if let TraceEvent::SchemaHeader { version, .. } = event {
            if *version != crate::SCHEMA_VERSION {
                return Err(UnsupportedSchema {
                    found: *version,
                    supported: crate::SCHEMA_VERSION,
                });
            }
        }
    }
    Ok(())
}

/// Error returned by [`validate_supported_schema`] when a trace's
/// header version is newer or older than the current binary knows.
#[derive(Debug, Clone)]
pub struct UnsupportedSchema {
    pub found: u32,
    pub supported: u32,
}

impl std::fmt::Display for UnsupportedSchema {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "trace schema version {} is not supported by this binary (supported: {})",
            self.found, self.supported
        )
    }
}

impl std::error::Error for UnsupportedSchema {}

// Silence an unused-warning on the `serde::de::Error` bring-in when
// nothing in the module references it (the bring-in is documentation
// that parse_event_line returns serde errors unwrapped).
#[allow(dead_code)]
fn _unused_serde_de_error_bring_in() -> serde_json::Error {
    serde_json::Error::custom("")
}
