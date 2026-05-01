//! Process-global bridge state: the typed `BridgeState` handle, the
//! `BRIDGE` atomic pointer it lives behind, the eager-init /
//! shutdown C-ABI entry points the codegen-emitted main bookends a
//! Corvid program with, and the `bench_*` counters that feed the
//! native runtime overhead benchmark.
//!
//! The bridge is stored as `AtomicPtr<BridgeState>` deliberately —
//! NOT a `OnceCell` or `Lazy` — because the contract with compiled
//! code is eager init: `corvid_runtime_init()` is called exactly
//! once at program startup before any other bridge function runs.
//! Bridge functions that observe a null pointer panic loudly rather
//! than silently initialising on first access.

use std::path::PathBuf;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::approvals::bench_approval_wait_ns;
use crate::llm::mock::{bench_mock_dispatch_ns, bench_prompt_wait_ns};
use crate::runtime::Runtime;
use crate::tracing::bench_trace_overhead_ns;

use super::{
    build_corvid_runtime, build_embedded_corvid_runtime, build_tokio_runtime,
    iter_registered_tools, record_registered_tool_count,
};

pub(super) static BENCH_JSON_BRIDGE_NS: AtomicU64 = AtomicU64::new(0);

pub(super) fn record_json_bridge_ns(
    start: Instant,
    prompt_wait_before: u64,
    mock_dispatch_before: u64,
) {
    let elapsed_ns = start.elapsed().as_nanos() as u64;
    let prompt_wait_ns = bench_prompt_wait_ns().saturating_sub(prompt_wait_before);
    let mock_dispatch_ns = bench_mock_dispatch_ns().saturating_sub(mock_dispatch_before);
    let residual = elapsed_ns
        .saturating_sub(prompt_wait_ns)
        .saturating_sub(mock_dispatch_ns);
    BENCH_JSON_BRIDGE_NS.fetch_add(residual, Ordering::Relaxed);
}

/// The bridge state owned for the lifetime of a compiled Corvid
/// process. Constructed eagerly by `corvid_runtime_init` and stored
/// behind the `BRIDGE` atomic pointer below. Dropped by
/// `corvid_runtime_shutdown`. The layout is private — compiled code
/// never sees the struct, only the bridge function surface.
pub struct BridgeState {
    /// Multi-thread tokio runtime. Owns the worker-thread pool.
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
pub(super) static BRIDGE: AtomicPtr<BridgeState> = AtomicPtr::new(std::ptr::null_mut());

/// Read the bridge pointer and panic if init hasn't run.
pub(crate) fn bridge() -> &'static BridgeState {
    let p = BRIDGE.load(Ordering::Acquire);
    if p.is_null() {
        panic!(
            "corvid runtime bridge accessed before `corvid_runtime_init()` was called — this is a codegen bug, not a runtime issue"
        );
    }
    // SAFETY: Non-null guaranteed by the check above. Pointer was
    // published via Release store in `corvid_runtime_init`; we observe
    // via Acquire here, so all writes that happened before the store
    // are visible to us.
    unsafe { &*p }
}

/// Probe function. Returns 42. Smoke-test the FFI path.
#[no_mangle]
pub extern "C" fn corvid_runtime_probe() -> i64 {
    42
}

#[no_mangle]
pub extern "C" fn corvid_bench_prompt_wait_ns() -> u64 {
    bench_prompt_wait_ns()
}

#[no_mangle]
pub extern "C" fn corvid_bench_tool_wait_ns() -> u64 {
    0
}

#[no_mangle]
pub extern "C" fn corvid_bench_approval_wait_ns() -> u64 {
    bench_approval_wait_ns()
}

#[no_mangle]
pub extern "C" fn corvid_bench_mock_dispatch_ns() -> u64 {
    bench_mock_dispatch_ns()
}

#[no_mangle]
pub extern "C" fn corvid_bench_trace_overhead_ns() -> u64 {
    bench_trace_overhead_ns()
}

#[no_mangle]
pub extern "C" fn corvid_bench_json_bridge_ns() -> u64 {
    BENCH_JSON_BRIDGE_NS.load(Ordering::Relaxed)
}

/// Construct the tokio runtime + the Corvid `Runtime` and store them
/// behind the global bridge pointer. MUST be called exactly once.
#[no_mangle]
pub extern "C" fn corvid_runtime_init() -> i32 {
    if !BRIDGE.load(Ordering::Acquire).is_null() {
        panic!("corvid_runtime_init called twice");
    }

    let ptr = Box::into_raw(Box::new(BridgeState {
        tokio: build_tokio_runtime(),
        corvid: Arc::new(build_corvid_runtime()),
    }));

    BRIDGE.store(ptr, Ordering::Release);

    let mut count: i64 = 0;
    for _meta in iter_registered_tools() {
        count += 1;
    }
    record_registered_tool_count(count);

    0
}

/// Idempotent runtime init for embedded cdylib/staticlib calls.
#[no_mangle]
pub extern "C" fn corvid_runtime_embed_init_default() -> i32 {
    if !BRIDGE.load(Ordering::Acquire).is_null() {
        return 0;
    }
    let ptr = Box::into_raw(Box::new(BridgeState {
        tokio: build_tokio_runtime(),
        corvid: Arc::new(build_embedded_corvid_runtime()),
    }));
    match BRIDGE.compare_exchange(
        std::ptr::null_mut(),
        ptr,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => 0,
        Err(_) => {
            // SAFETY: compare_exchange failure means we still own `ptr`.
            unsafe {
                drop(Box::from_raw(ptr));
            }
            0
        }
    }
}

/// Drop the bridge state cleanly.
#[no_mangle]
pub extern "C" fn corvid_runtime_shutdown() {
    let ptr = BRIDGE.swap(std::ptr::null_mut(), Ordering::AcqRel);
    if ptr.is_null() {
        return;
    }
    // SAFETY: Pointer came from `Box::into_raw` in `corvid_runtime_init`
    // and is owned by us (we just atomically swapped it out).
    let bridge = unsafe { Box::from_raw(ptr) };
    if let Some(path) = std::env::var_os("CORVID_REPLAY_DIFFERENTIAL_REPORT_PATH") {
        if let Err(err) = bridge
            .corvid
            .write_replay_differential_report(PathBuf::from(path))
        {
            eprintln!("corvid replay differential report write failed: {err}");
        }
    }
    if let Some(path) = std::env::var_os("CORVID_REPLAY_MUTATION_REPORT_PATH") {
        if let Err(err) = bridge
            .corvid
            .write_replay_mutation_report(PathBuf::from(path))
        {
            eprintln!("corvid replay mutation report write failed: {err}");
        }
    }
    drop(bridge);
    crate::grounded_handles::emit_debug_leak_warning();
    crate::observation_handles::emit_debug_leak_warning();
}

/// Clone of the process-global `Arc<Runtime>`.
pub fn runtime() -> Arc<Runtime> {
    bridge().corvid_runtime()
}

#[no_mangle]
pub extern "C" fn corvid_runtime_is_replay() -> bool {
    bridge().corvid_runtime().is_replay_mode()
}
