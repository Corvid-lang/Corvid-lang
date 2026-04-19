//! Determinism catalog — the source-level nondeterministic
//! builtins a Corvid program can call, and which trace-event
//! variant must capture each one for replayability.
//!
//! Consumed by the `@replayable` checker (Phase 21 slice A) to
//! decide whether a function call in an agent body introduces
//! an unrecoverable source of nondeterminism. When a user
//! declares `@replayable agent foo(...)`, every call in the
//! body that resolves to a name in this catalog must have a
//! matching recording hook in the runtime.
//!
//! As of v1 of the Phase 21 schema, Corvid source does not
//! expose any source-level clock or PRNG builtins. This
//! catalog is therefore empty but structurally complete — new
//! builtins register here and the `@replayable` checker picks
//! them up with no further wiring.

use std::borrow::Cow;

/// Which kind of nondeterministic source a call introduces.
/// Each variant names the trace-event variant that must capture
/// the result for replay to reproduce it.
///
/// See `corvid_trace_schema::TraceEvent` for the corresponding
/// event shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NondeterminismSource {
    /// Wall-clock read. Records as `TraceEvent::ClockRead` with
    /// `source = "wall"`.
    WallClock,
    /// Monotonic-clock read. Records as `TraceEvent::ClockRead`
    /// with `source = "monotonic"`.
    MonotonicClock,
    /// Process-start-time read. Records as
    /// `TraceEvent::ClockRead` with `source = "system_start"`.
    SystemStartClock,
    /// Pseudo-random number. Records as `TraceEvent::SeedRead`.
    Prng,
    /// Environment-variable read. Records as `TraceEvent::ClockRead`
    /// is the wrong fit; a future schema version will add
    /// `TraceEvent::EnvironmentRead`. Reserved.
    EnvironmentVar,
    /// Random UUID generation. A future schema version will add
    /// `TraceEvent::UuidGenerated`. Reserved.
    RandomUuid,
}

impl NondeterminismSource {
    /// Human-readable label used in diagnostics ("wall-clock read",
    /// etc.). Stable wording — used in spec tests that match on
    /// the exact text.
    pub fn label(&self) -> &'static str {
        match self {
            Self::WallClock => "wall-clock read",
            Self::MonotonicClock => "monotonic-clock read",
            Self::SystemStartClock => "system-start-clock read",
            Self::Prng => "pseudo-random draw",
            Self::EnvironmentVar => "environment-variable read",
            Self::RandomUuid => "random UUID",
        }
    }
}

/// One entry in the determinism catalog. `name` is the fully-
/// qualified source-level identifier the resolver binds the call
/// target to. `source` names the kind of nondeterminism the call
/// introduces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NondeterministicBuiltin {
    pub name: Cow<'static, str>,
    pub source: NondeterminismSource,
}

/// Every nondeterministic source-level builtin Corvid knows about.
/// Empty as of Phase 21 v1 — source-level clock / PRNG APIs are
/// not yet exposed.
///
/// Populate this when new builtins land; the `@replayable` checker
/// picks them up automatically.
pub const KNOWN_NONDETERMINISTIC_BUILTINS: &[NondeterministicBuiltin] = &[];

/// Lookup helper — returns `Some(source)` if the fully-qualified
/// name matches a registered nondeterministic builtin, `None`
/// otherwise. The checker calls this once per resolved function
/// reference inside a `@replayable` body.
pub fn classify_call_target(name: &str) -> Option<NondeterminismSource> {
    KNOWN_NONDETERMINISTIC_BUILTINS
        .iter()
        .find(|entry| entry.name == name)
        .map(|entry| entry.source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_empty_until_source_level_builtins_land() {
        assert!(KNOWN_NONDETERMINISTIC_BUILTINS.is_empty());
    }

    #[test]
    fn classify_rejects_unknown_names() {
        assert!(classify_call_target("some_tool").is_none());
        assert!(classify_call_target("std::time::now").is_none());
    }

    #[test]
    fn labels_are_stable() {
        // These labels appear in diagnostics and spec tests;
        // changing them is a user-visible change and needs a
        // dev-log entry.
        assert_eq!(NondeterminismSource::WallClock.label(), "wall-clock read");
        assert_eq!(NondeterminismSource::Prng.label(), "pseudo-random draw");
        assert_eq!(
            NondeterminismSource::MonotonicClock.label(),
            "monotonic-clock read"
        );
        assert_eq!(
            NondeterminismSource::SystemStartClock.label(),
            "system-start-clock read"
        );
        assert_eq!(
            NondeterminismSource::EnvironmentVar.label(),
            "environment-variable read"
        );
        assert_eq!(NondeterminismSource::RandomUuid.label(), "random UUID");
    }
}
