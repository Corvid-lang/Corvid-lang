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
//! once Phase 20's `Stream<T>` and Phase 25's multi-agent work land.
//! Design decision made at Phase 13 pre-phase chat (dev-log Day 30):
//! pay the startup tax now so Corvid ships on a runtime that matches
//! the GP-language positioning from day one, rather than swapping
//! runtimes mid-roadmap.

#![allow(unsafe_code)]

use crate::abi::{CorvidString, REGISTERED_TOOL_COUNT};
use crate::approvals::{ProgrammaticApprover, StdinApprover};
use crate::llm::anthropic::AnthropicAdapter;
use crate::llm::openai::OpenAiAdapter;
use crate::runtime::{Runtime, RuntimeBuilder};
use crate::tracing::{fresh_run_id, Tracer};
use crate::redact::RedactionSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Arc;

/// The bridge state owned for the lifetime of a compiled Corvid
/// process. Constructed eagerly by `corvid_runtime_init` and stored
/// behind the `BRIDGE` atomic pointer below. Dropped by
/// `corvid_runtime_shutdown`. The layout is private — compiled code
/// never sees the struct, only the bridge function surface.
pub struct BridgeState {
    /// Multi-thread tokio runtime. Owns the worker-thread pool.
    /// `new_multi_thread` default is num_cpus workers. Env override
    /// via `CORVID_TOKIO_WORKERS` is respected if present.
    tokio: tokio::runtime::Runtime,
    /// The Corvid runtime — tool registry, LLM adapters, approver,
    /// tracer. Shared behind `Arc` for downstream futures to clone
    /// freely without contending on the bridge pointer.
    corvid: Arc<Runtime>,
}

impl BridgeState {
    /// A handle into the tokio runtime that async bridge calls use to
    /// `block_on`. Cheap to clone; caller does not need a handle for
    /// every call.
    pub(crate) fn tokio_handle(&self) -> tokio::runtime::Handle {
        self.tokio.handle().clone()
    }

    /// Clone of the `Arc<Runtime>` — tool/prompt bridges move this into
    /// an async block so the future doesn't borrow the bridge.
    pub(crate) fn corvid_runtime(&self) -> Arc<Runtime> {
        Arc::clone(&self.corvid)
    }
}

/// Global pointer to the process-wide bridge state.
///
/// `AtomicPtr<BridgeState>` deliberately NOT `OnceCell` or `Lazy` —
/// this is eager init, not lazy. Before `corvid_runtime_init` returns
/// this is null; after it returns it points at a `Box::leak`'d
/// `BridgeState`. Readers load with `Ordering::Acquire` to pair with
/// the `Ordering::Release` store in init.
static BRIDGE: AtomicPtr<BridgeState> = AtomicPtr::new(std::ptr::null_mut());

/// Read the bridge pointer and panic if init hasn't run. Returns a
/// `&'static BridgeState` because the `Box::leak` that created it gave
/// the allocation program lifetime.
fn bridge() -> &'static BridgeState {
    let p = BRIDGE.load(Ordering::Acquire);
    if p.is_null() {
        panic!(
            "corvid runtime bridge accessed before `corvid_runtime_init()` was called — this is a codegen bug, not a runtime issue"
        );
    }
    // SAFETY: Non-null guaranteed by the check above. Pointer was
    // published via Release store in `corvid_runtime_init`; we observe
    // via Acquire here, so all writes that happened before the store
    // are visible to us. Box::leak gave the allocation program
    // lifetime so the `&'static` is sound.
    unsafe { &*p }
}

/// Phase-13 probe function. Returns 42. Used by the slice-13a smoke
/// test to verify the staticlib builds and links correctly into a
/// compiled Corvid binary before any of the real bridge surface lands.
/// Kept permanently because smoke-testing the FFI path on any future
/// toolchain change is cheap — just call it.
#[no_mangle]
pub extern "C" fn corvid_runtime_probe() -> i64 {
    42
}

/// Construct the tokio runtime + the Corvid `Runtime` and store them
/// behind the global bridge pointer. MUST be called exactly once,
/// eagerly, before any other bridge function. Second call panics;
/// bridge functions called before it panic.
///
/// Adapter configuration follows the same env-var pattern the driver
/// uses for the interpreter tier:
///
///   `CORVID_MODEL`       — default model name
///   `ANTHROPIC_API_KEY`  — present → AnthropicAdapter registered
///   `OPENAI_API_KEY`     — present → OpenAiAdapter registered
///   `CORVID_APPROVE_AUTO` — `1` selects ProgrammaticApprover::always_yes,
///                          else stdin approver (the interactive default)
///   `CORVID_TOKIO_WORKERS` — integer override for worker thread count
///                            (default: num_cpus)
///
/// Returns 0 on success. Non-zero return values are reserved for
/// future error codes; today the function panics on any failure
/// since init failure means the program cannot continue.
#[no_mangle]
pub extern "C" fn corvid_runtime_init() -> i32 {
    if !BRIDGE.load(Ordering::Acquire).is_null() {
        panic!("corvid_runtime_init called twice");
    }

    let tokio = build_tokio_runtime();
    let corvid = Arc::new(build_corvid_runtime());

    let boxed = Box::new(BridgeState { tokio, corvid });
    let ptr = Box::into_raw(boxed);

    BRIDGE.store(ptr, Ordering::Release);

    // Slice 14c: walk every `#[tool]` metadata entry linked into this
    // binary. Today we just record the count for diagnostics; slice
    // 14e plumbs these entries into the approve-policy table, and a
    // future `corvid check` command can cross-verify the signatures
    // against the `.cor` source.
    //
    // This walk is cold-path — once per program startup, O(n) in the
    // number of tools. For any realistic program, n < 100.
    let mut count: i64 = 0;
    for _meta in iter_registered_tools() {
        count += 1;
    }
    record_registered_tool_count(count);

    0
}

/// Drop the bridge state cleanly. The codegen-emitted main registers
/// this with `atexit`. After it runs, any further bridge call panics
/// (the post-shutdown null state is indistinguishable from pre-init,
/// and either situation means something is wrong in the codegen).
#[no_mangle]
pub extern "C" fn corvid_runtime_shutdown() {
    let ptr = BRIDGE.swap(std::ptr::null_mut(), Ordering::AcqRel);
    if ptr.is_null() {
        return; // already shut down or never initialised — no-op, idempotent
    }
    // SAFETY: Pointer came from `Box::into_raw` in `corvid_runtime_init`
    // and is owned by us (we just atomically swapped it out so no
    // concurrent reader sees it any more). Reconstructing the Box drops
    // the bridge state, which drops the tokio Runtime (joins worker
    // threads) and the Corvid Runtime Arc.
    unsafe {
        drop(Box::from_raw(ptr));
    }
}

/// Phase-13 tool-call bridge for the narrow case `fn(no args) -> Int`.
///
/// Compiled Corvid code that resolved a tool call to an agent-scope
/// `IrCallKind::Tool` emits a call to this function with the tool name
/// as a pointer + length. We call into the runtime's `call_tool` via
/// `block_on` and return the resulting Int.
///
/// Arguments JSON is hardcoded to `[]` in this slice — Phase 14 ships
/// the generalised bridge with full argument + return-type marshalling
/// via `serde_json`. This narrow version exists so the parity harness
/// can exercise the async path end-to-end before Phase 14 lands.
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
pub unsafe extern "C" fn corvid_tool_call_sync_int(
    name_ptr: *const u8,
    name_len: usize,
) -> i64 {
    // SAFETY: Caller contract — pointer + length describe valid UTF-8
    // bytes alive for the call. Empty name is handled by the `is_null`
    // check rather than slicing into a null pointer.
    let name: &str = unsafe {
        if name_ptr.is_null() || name_len == 0 {
            eprintln!("corvid_tool_call_sync_int: null/empty tool name");
            return i64::MIN;
        }
        let slice = std::slice::from_raw_parts(name_ptr, name_len);
        match std::str::from_utf8(slice) {
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

/// Tokio handle the `#[tool]` wrappers block_on. Panics if
/// `corvid_runtime_init` hasn't run — matches the eager-init contract.
pub fn tokio_handle() -> tokio::runtime::Handle {
    bridge().tokio_handle()
}

/// Clone of the process-global `Arc<Runtime>`. Available to anything
/// that needs to dispatch through the runtime from a non-C-ABI
/// context (e.g. in-process tests).
pub fn runtime() -> std::sync::Arc<crate::runtime::Runtime> {
    bridge().corvid_runtime()
}

// ------------------------------------------------------------
// String helpers consumed by `corvid_runtime::abi`'s conversion
// traits. Declared here because they cross the FFI boundary — the
// bytes they allocate are visible to compiled Corvid code as
// refcounted Corvid Strings.
// ------------------------------------------------------------

extern "C" {
    /// Allocate a heap Corvid String from `bytes` + `length`.
    /// Implemented in C (`runtime/strings.c`). Returns a descriptor
    /// pointer with refcount 1.
    fn corvid_string_from_bytes(bytes: *const u8, length: i64) -> *const u8;

    /// Decrement a Corvid String's refcount, freeing when it hits 0.
    /// The refcount sentinel `i64::MIN` short-circuits for immortal
    /// `.rodata` literals.
    fn corvid_release(descriptor: *const u8);
}

/// Allocate a Corvid String from a Rust `String`. The returned
/// `CorvidString` has refcount 1 — caller takes ownership.
pub fn string_from_rust(s: String) -> CorvidString {
    let bytes = s.as_bytes();
    let ptr = bytes.as_ptr();
    let len = bytes.len() as i64;
    // SAFETY: `corvid_string_from_bytes` reads `len` bytes starting
    // at `ptr`. We hold `s` alive for the duration of the call, so
    // the bytes are valid. The returned descriptor owns its own
    // allocation — caller is free to drop `s` after this returns.
    let descriptor = unsafe { corvid_string_from_bytes(ptr, len) };
    // SAFETY: `CorvidString` is `#[repr(transparent)]` over a
    // descriptor pointer; transmuting from a raw pointer of the same
    // layout is sound. Using `transmute_copy` is overkill — a plain
    // pointer cast is enough, but we use an unsafe block to make the
    // layout assumption auditable.
    unsafe { std::mem::transmute(descriptor) }
}

/// Release a `CorvidString`'s refcount. Used by the `FromCorvidAbi`
/// impl on `String` after copying bytes out — paired with the implicit
/// retain the caller's `+0 ABI` contract performed on entry.
///
/// # Safety
///
/// `cs` must come from valid codegen- or runtime-emitted source
/// (i.e. the caller followed the Corvid ABI when passing the value).
pub unsafe fn release_string(cs: CorvidString) {
    // SAFETY: Transmuting a `#[repr(transparent)]` wrapper back to its
    // single field is sound. `corvid_release` expects a descriptor
    // pointer (the type alias for "CorvidString at the ABI") and
    // tolerates null by short-circuiting.
    unsafe {
        let descriptor: *const u8 = std::mem::transmute(cs);
        corvid_release(descriptor);
    }
}

/// Iterate every `ToolMetadata` registered via `#[tool]` across all
/// linked tool crates. Used by `corvid_runtime_init` at startup.
pub fn iter_registered_tools() -> impl Iterator<Item = &'static crate::abi::ToolMetadata> {
    inventory::iter::<crate::abi::ToolMetadata>().into_iter()
}

/// Snapshot the tool-registration count so diagnostics can surface it.
/// Called once during `corvid_runtime_init` after iterating inventory.
pub(crate) fn record_registered_tool_count(n: i64) {
    REGISTERED_TOOL_COUNT.store(n, std::sync::atomic::Ordering::Relaxed);
}

// ------------------------------------------------------------
// Internal helpers (safe Rust, no FFI).
// ------------------------------------------------------------

fn build_tokio_runtime() -> tokio::runtime::Runtime {
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    if let Ok(n) = std::env::var("CORVID_TOKIO_WORKERS") {
        if let Ok(parsed) = n.parse::<usize>() {
            if parsed > 0 {
                builder.worker_threads(parsed);
            }
        }
    }
    builder
        .build()
        .expect("construct multi-thread tokio runtime")
}

fn build_corvid_runtime() -> Runtime {
    let trace_dir = trace_dir_for_current_process();
    let tracer = Tracer::open(&trace_dir, fresh_run_id())
        .with_redaction(RedactionSet::from_env());

    let mut b: RuntimeBuilder = Runtime::builder().tracer(tracer);

    // Approver: interactive stdin by default; programmatic-yes if the
    // user has opted into auto-approve (useful for batch / CI runs).
    if std::env::var("CORVID_APPROVE_AUTO").ok().as_deref() == Some("1") {
        b = b.approver(Arc::new(ProgrammaticApprover::always_yes()));
    } else {
        b = b.approver(Arc::new(StdinApprover::new()));
    }

    if let Ok(model) = std::env::var("CORVID_MODEL") {
        b = b.default_model(&model);
    }
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        b = b.llm(Arc::new(AnthropicAdapter::new(key)));
    }
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        b = b.llm(Arc::new(OpenAiAdapter::new(key)));
    }

    // Phase 13 test-only mock-tool registration. Format:
    //   CORVID_TEST_MOCK_INT_TOOLS="name1:value1;name2:value2"
    //
    // Each name becomes a tool that ignores its args and returns the
    // given Int. Used by the parity harness to exercise the compiled
    // tool-call path before Phase 14 ships the user-facing proc-macro
    // registry. Not a production feature — users never set this env
    // var, and nothing in the driver surfaces it.
    if let Ok(spec) = std::env::var("CORVID_TEST_MOCK_INT_TOOLS") {
        for pair in spec.split(';') {
            let pair = pair.trim();
            if pair.is_empty() {
                continue;
            }
            let Some((name, value_str)) = pair.split_once(':') else {
                eprintln!(
                    "corvid: malformed CORVID_TEST_MOCK_INT_TOOLS entry `{pair}` (expected `name:value`); skipping"
                );
                continue;
            };
            let Ok(value) = value_str.trim().parse::<i64>() else {
                eprintln!(
                    "corvid: CORVID_TEST_MOCK_INT_TOOLS value `{value_str}` for `{name}` isn't a valid i64; skipping"
                );
                continue;
            };
            let name_owned = name.trim().to_string();
            b = b.tool(name_owned, move |_args| async move {
                Ok(serde_json::json!(value))
            });
        }
    }

    b.build()
}

/// `target/trace/` under the current process's working directory. Same
/// convention as the interpreter tier uses — a compiled binary run
/// from `<project>/` writes traces next to the project's other
/// build artifacts.
fn trace_dir_for_current_process() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("target")
        .join("trace")
}

// ------------------------------------------------------------
// Tests — internal only, exercise the safe Rust surface. The C-ABI
// path is covered by the corvid-codegen-cl parity harness once
// slice 13a's link flow lands.
// ------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_returns_42() {
        assert_eq!(corvid_runtime_probe(), 42);
    }

    // Note: init/shutdown tests can't run in parallel because the
    // bridge is a process-global. Kept to a single sequenced test
    // that covers both paths. Ignored by default to keep the standard
    // `cargo test` path clean of global-state mutation; run explicitly
    // with `cargo test -- --ignored init_and_shutdown_cycle`.
    #[test]
    #[ignore = "mutates the process-global BRIDGE; run with --ignored"]
    fn init_and_shutdown_cycle() {
        assert_eq!(corvid_runtime_init(), 0);
        assert!(!BRIDGE.load(Ordering::Acquire).is_null());
        // Second init must panic — verified via a separate test run;
        // can't assert here without crashing the test process. Just
        // confirm shutdown works.
        corvid_runtime_shutdown();
        assert!(BRIDGE.load(Ordering::Acquire).is_null());
        // Shutdown is idempotent — second call is a no-op, not a panic.
        corvid_runtime_shutdown();
        assert!(BRIDGE.load(Ordering::Acquire).is_null());
    }
}
