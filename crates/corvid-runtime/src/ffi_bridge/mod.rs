//! C-ABI bridge exposed to compiled Corvid binaries.
//!
//! Compiled binaries emitted by `corvid-codegen-cl` are C-main programs
//! that need to call into Rust for anything involving tokio or the
//! Corvid `Runtime` (tool dispatch, prompt dispatch, approves, tracing).
//! This module is the only bridge between those two worlds.
//!
//! # Safety rationale
//!
//! The rest of the crate lives under `#![deny(unsafe_code)]`. This
//! module opts out because FFI fundamentally requires raw-pointer
//! handling — there is no safe Rust way to expose a `pub extern "C"`
//! surface. Every `unsafe` block in this file is paired with a SAFETY:
//! comment describing the precondition the caller must uphold.
//!
//! # Contract with compiled code
//!
//! The compiled binary's codegen-emitted `main` calls exactly one
//! bridge function at startup:
//!
//!   `corvid_runtime_init()` — constructs the multi-thread tokio
//!   Runtime and the `corvid_runtime::Runtime`, stores both behind
//!   a global `AtomicPtr`. **Called exactly once, eagerly, before any
//!   other bridge call.** No lazy init — if a bridge function runs
//!   before `corvid_runtime_init` has returned, it panics loudly
//!   rather than silently initialising on first access.
//!
//! At program exit the codegen-emitted main calls
//! `corvid_runtime_shutdown()` which drops both runtimes cleanly.
//!
//! # Why multi-thread tokio
//!
//! Single-threaded current-thread runtime starts faster (~0ms vs
//! ~5-10ms) but can't give Corvid a production-grade concurrency story
//! once streaming and multi-agent support land.
//! Design decision from the native runtime bring-up discussion (dev-log Day 30):
//! pay the startup tax now so Corvid ships on a runtime that matches
//! the GP-language positioning from day one, rather than swapping
//! runtimes mid-roadmap.

#![allow(unsafe_code)]

use crate::abi::CorvidString;
use crate::errors::RuntimeError;
use std::path::PathBuf;

mod approval_exports;
mod llm_dispatch;
mod prompt_exports;
mod replay_exports;
mod state;
mod strings;
mod tokio_handle;
mod tool_iter;
pub use approval_exports::{corvid_approve_sync, corvid_citation_verify_or_panic};
pub use prompt_exports::{
    corvid_prompt_call_bool, corvid_prompt_call_float, corvid_prompt_call_int,
    corvid_prompt_call_string,
};
pub use replay_exports::{
    corvid_replay_tool_call_bool, corvid_replay_tool_call_float, corvid_replay_tool_call_int,
    corvid_replay_tool_call_nothing, corvid_replay_tool_call_string,
};
pub(crate) use state::bridge;
pub use state::{
    corvid_bench_approval_wait_ns, corvid_bench_json_bridge_ns, corvid_bench_mock_dispatch_ns,
    corvid_bench_prompt_wait_ns, corvid_bench_tool_wait_ns, corvid_bench_trace_overhead_ns,
    corvid_runtime_embed_init_default, corvid_runtime_init, corvid_runtime_is_replay,
    corvid_runtime_probe, corvid_runtime_shutdown, runtime,
};
use state::{record_json_bridge_ns, BridgeState, BRIDGE};
pub(crate) use strings::{borrow_corvid_string, read_corvid_string};
pub use strings::{
    corvid_free_string, corvid_string_into_cstr, release_string, string_from_rust,
    string_from_static_str,
};
pub use tokio_handle::tokio_handle;
use tokio_handle::{build_corvid_runtime, build_embedded_corvid_runtime, build_tokio_runtime};
pub use tool_iter::iter_registered_tools;
pub(crate) use tool_iter::record_registered_tool_count;

/// Tool-call bridge for the narrow case `fn(no args) -> Int`.
///
/// Compiled Corvid code that resolved a tool call to an agent-scope
/// `IrCallKind::Tool` emits a call to this function with the tool name
/// as a pointer + length. We call into the runtime's `call_tool` via
/// `block_on` and return the resulting Int.
///
/// Arguments JSON is hardcoded to `[]` here — the typed bridge ships
/// the generalised bridge with full argument + return-type marshalling
/// via `serde_json`. This narrow version exists so the parity harness
/// can exercise the async path end-to-end before the typed bridge lands.
///
/// Error conventions:
///
/// - Tool not found → returns `i64::MIN` and prints a diagnostic on stderr.
///   `i64::MIN` is a sentinel because it's the one `i64` value that
///   overflows Corvid's Int (negating it traps), so no valid Corvid
///   tool result can collide with it.
/// - Tool returns non-integer JSON → returns `i64::MIN` with stderr.
/// - Tool returns error → returns `i64::MIN` with stderr.
///
/// # Safety
///
/// `name_ptr` must point to `name_len` valid UTF-8 bytes the caller
/// keeps alive for the duration of the call. The runtime bridge must
/// have been initialised via `corvid_runtime_init` first.
#[no_mangle]
pub unsafe extern "C" fn corvid_tool_call_sync_int(name_ptr: *const u8, name_len: usize) -> i64 {
    // SAFETY: Caller contract — pointer + length describe valid UTF-8
    // bytes alive for the call. Empty name is handled by the `is_null`
    // check rather than slicing into a null pointer.
    let name: &str = unsafe {
        if name_ptr.is_null() || name_len == 0 {
            eprintln!("corvid_tool_call_sync_int: null/empty tool name");
            return i64::MIN;
        }
        let name_bytes = std::slice::from_raw_parts(name_ptr, name_len);
        match std::str::from_utf8(name_bytes) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("corvid_tool_call_sync_int: invalid UTF-8 in tool name: {e}");
                return i64::MIN;
            }
        }
    };

    let state = bridge();
    let runtime = state.corvid_runtime();
    let name_owned = name.to_string();

    // block_on dispatches the async call on the multi-thread tokio
    // runtime. The worker pool handles the future; the caller thread
    // (this one) blocks until it resolves.
    let result = state
        .tokio_handle()
        .block_on(async move { runtime.call_tool(&name_owned, Vec::new()).await });

    match result {
        Ok(value) => match value.as_i64() {
            Some(n) => {
                if n == i64::MIN {
                    eprintln!(
                        "corvid_tool_call_sync_int: tool `{name}` returned i64::MIN which collides with the error sentinel"
                    );
                    i64::MIN
                } else {
                    n
                }
            }
            None => {
                eprintln!(
                    "corvid_tool_call_sync_int: tool `{name}` returned non-integer JSON: {value}"
                );
                i64::MIN
            }
        },
        Err(e) => {
            panic_if_replay_runtime_error(
                &format!("corvid_tool_call_sync_int: tool `{name}` failed"),
                &e,
            );
            eprintln!("corvid_tool_call_sync_int: tool `{name}` error: {e}");
            i64::MIN
        }
    }
}

// ------------------------------------------------------------
// Public helpers used by `#[tool]`-generated wrappers and the
// Cranelift codegen. Not part of the C ABI — ordinary Rust surface
// consumed by compile-time-generated code.
// ------------------------------------------------------------

fn panic_if_replay_runtime_error(context: &str, err: &RuntimeError) {
    if matches!(
        err,
        RuntimeError::ReplayTraceLoad { .. }
            | RuntimeError::ReplayDivergence(_)
            | RuntimeError::InvalidReplayMutation { .. }
            | RuntimeError::CrossTierReplayUnsupported { .. }
    ) {
        panic!("{context}: {err}");
    }
}

// ------------------------------------------------------------

pub(super) fn trace_path_from_env() -> Option<PathBuf> {
    std::env::var_os("CORVID_TRACE_PATH").map(PathBuf::from)
}

// ------------------------------------------------------------
// Internal helpers (safe Rust, no FFI).
// ------------------------------------------------------------
