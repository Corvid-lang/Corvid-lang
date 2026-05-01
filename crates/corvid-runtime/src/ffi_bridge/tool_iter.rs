//! Tool iterator + per-tool descriptor exposure.
//!
//! `iter_registered_tools` walks every `ToolMetadata` registered
//! via the `#[tool]` proc-macro across all linked tool crates
//! (the proc-macro emits an `inventory::submit!` so the entries
//! gather in a process-global static). `corvid_runtime_init`
//! drives the walk once at startup; the resulting count is
//! pinned via `record_registered_tool_count` so the runtime's
//! diagnostic surface can surface it without re-walking.

use std::sync::atomic::Ordering;

use crate::abi::{ToolMetadata, REGISTERED_TOOL_COUNT};

/// Iterate every `ToolMetadata` registered via `#[tool]` across
/// all linked tool crates. Used by `corvid_runtime_init` at
/// startup.
pub fn iter_registered_tools() -> impl Iterator<Item = &'static ToolMetadata> {
    inventory::iter::<ToolMetadata>().into_iter()
}

/// Snapshot the tool-registration count so diagnostics can
/// surface it. Called once during `corvid_runtime_init` after
/// iterating inventory.
pub(crate) fn record_registered_tool_count(n: i64) {
    REGISTERED_TOOL_COUNT.store(n, Ordering::Relaxed);
}
