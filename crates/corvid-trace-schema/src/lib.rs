//! Shared trace schema for Corvid interpreter + native tiers.
//!
//! Every tier writes the same JSONL format; every reader
//! (`corvid replay`, `corvid routing-report`, REPL replay, the
//! differential-verify harness) consumes the same `TraceEvent`
//! enum. The schema lives in its own crate so no implementation
//! tier can drift from it.
//!
//! # Shape
//!
//! A trace is a sequence of line-delimited JSON objects. The
//! first object is a [`TraceEvent::SchemaHeader`] carrying the
//! schema version; the rest are the events emitted during the
//! program's run, ending with a [`TraceEvent::RunCompleted`].
//!
//! See [`SCHEMA_VERSION`] for the current version of the wire
//! format.

pub mod event;
pub mod io;

pub use event::TraceEvent;
pub use io::{
    append_event, parse_event_line, read_events, read_events_from_path, schema_version_of,
    serialize_event_line, source_path_of, validate_supported_schema, ReadError,
    UnsupportedSchema, write_events_to_path,
};

/// Current trace schema version. Bump whenever an existing
/// variant's shape changes. Adding a new variant is additive —
/// readers skip unknown kinds — and does not require a bump.
///
/// # Version history
///
/// - `1`: Initial schema (21-A-schema). `SchemaHeader` +
///   run/tool/llm/approval/seed/clock/dispatch events.
/// - `2`: `SchemaHeader.source_path` (21-A-schema-ext-source).
///   Optional, `#[serde(default)]` — v1 traces remain readable.
pub const SCHEMA_VERSION: u32 = 2;

/// Oldest schema version this binary can still read. Traces at
/// any version in `MIN_SUPPORTED_SCHEMA..=SCHEMA_VERSION` are
/// accepted; fields added in later versions fall back to their
/// `#[serde(default)]` when deserializing older traces.
pub const MIN_SUPPORTED_SCHEMA: u32 = 1;

/// Writer identifier the interpreter tier emits in its
/// `SchemaHeader`. Kept here so every tier references the same
/// constant.
pub const WRITER_INTERPRETER: &str = "corvid-vm";

/// Writer identifier the native tier emits in its `SchemaHeader`.
pub const WRITER_NATIVE: &str = "corvid-codegen-cl";
