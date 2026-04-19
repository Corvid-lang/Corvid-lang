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

use crate::abi::{CorvidString, REGISTERED_TOOL_COUNT};
use crate::approvals::{bench_approval_wait_ns, ProgrammaticApprover, StdinApprover};
use crate::llm::anthropic::AnthropicAdapter;
use crate::llm::gemini::GeminiAdapter;
use crate::llm::mock::{
    bench_mock_dispatch_ns, bench_prompt_wait_ns, env_mock_string_reply_sync, EnvVarMockAdapter,
};
use crate::llm::ollama::OllamaAdapter;
use crate::llm::openai::OpenAiAdapter;
use crate::llm::openai_compat::OpenAiCompatibleAdapter;
use crate::redact::RedactionSet;
use crate::runtime::{Runtime, RuntimeBuilder};
use crate::tracing::{bench_trace_overhead_ns, fresh_run_id, Tracer};
use corvid_trace_schema::{TraceEvent, WRITER_NATIVE};
use std::path::PathBuf;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

static BENCH_JSON_BRIDGE_NS: AtomicU64 = AtomicU64::new(0);

fn record_json_bridge_ns(start: Instant, prompt_wait_before: u64, mock_dispatch_before: u64) {
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
    // are visible to us. Box::leak gave the allocation program
    // lifetime so the `&'static` is sound.
    unsafe { &*p }
}

/// Probe function. Returns 42. Used by the early smoke
/// test to verify the staticlib builds and links correctly into a
/// compiled Corvid binary before any of the real bridge surface lands.
/// Kept permanently because smoke-testing the FFI path on any future
/// toolchain change is cheap — just call it.
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

    // Walk every `#[tool]` metadata entry linked into this
    // binary. Today we just record the count for diagnostics; later
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

fn trace_mock_llm_attempt(
    state: &BridgeState,
    prompt_name: &str,
    model: &str,
    rendered: &str,
    args: &[serde_json::Value],
    result: serde_json::Value,
) {
    let runtime = state.corvid_runtime();
    let tracer = runtime.tracer();
    if !tracer.is_enabled() {
        return;
    }
    let effective_model = if model.is_empty() {
        runtime.default_model()
    } else {
        model
    };
    tracer.emit(TraceEvent::LlmCall {
        ts_ms: crate::tracing::now_ms(),
        run_id: tracer.run_id().to_string(),
        prompt: prompt_name.to_string(),
        model: if effective_model.is_empty() {
            None
        } else {
            Some(effective_model.to_string())
        },
        rendered: Some(rendered.to_string()),
        args: args.to_vec(),
    });
    tracer.emit(TraceEvent::LlmResult {
        ts_ms: crate::tracing::now_ms(),
        run_id: tracer.run_id().to_string(),
        prompt: prompt_name.to_string(),
        model: if effective_model.is_empty() {
            None
        } else {
            Some(effective_model.to_string())
        },
        result,
    });
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

    /// Allocate an immortal Corvid String from `bytes` + `length`.
    /// Used for repeated runtime fixture values where per-use release
    /// work is pure overhead.
    fn corvid_string_from_static_bytes(bytes: *const u8, length: i64) -> *const u8;

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

/// Allocate an immortal Corvid String from a borrowed Rust string.
/// The returned value can be copied and released arbitrarily; the
/// immortal refcount sentinel makes release a no-op.
pub fn string_from_static_str(s: &str) -> CorvidString {
    let bytes = s.as_bytes();
    let ptr = bytes.as_ptr();
    let len = bytes.len() as i64;
    let descriptor = unsafe { corvid_string_from_static_bytes(ptr, len) };
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
// Typed prompt-dispatch bridges.
//
// One bridge per return type, mirroring the typed-ABI tool design.
// Each takes 4 CorvidString args (prompt name, signature string,
// rendered prompt body, model name) and returns the typed value.
//
// All four bridges follow the same shape internally:
//   1. Read CorvidString args as Rust Strings (borrow, no refcount poke).
//   2. Build a system prompt: function-signature context +
//      return-type-specific format instruction.
//   3. Loop up to CORVID_PROMPT_MAX_RETRIES (default 3):
//      a. Call the adapter via block_on.
//      b. Parse the response into the typed value.
//      c. On parse success, return.
//      d. On parse failure, capture last response for next retry's
//         stronger system prompt.
//   4. After max retries, panic with a clear message including the
//      last LLM response — compiled binary aborts with stderr trail
//      so the user can see what went wrong.
//
// String returns skip the parse-retry loop entirely (a String response
// is by definition parseable as String). The shape stays uniform so
// codegen has the same call pattern for every return type.
//
// Function-signature context is the inventive piece: the system
// prompt explicitly tells the LLM "you are a function with signature
// X — return the appropriate value." Codegen knows the signature at
// compile time and embeds it as a literal. Same prompt body, much
// better LLM behavior because the model has the type contract.
// ------------------------------------------------------------

use crate::llm::LlmRequestRef;

/// Default retry count when `CORVID_PROMPT_MAX_RETRIES` env is unset.
const DEFAULT_PROMPT_MAX_RETRIES: u32 = 3;

fn prompt_max_retries() -> u32 {
    static VALUE: OnceLock<u32> = OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("CORVID_PROMPT_MAX_RETRIES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_PROMPT_MAX_RETRIES)
    })
}

fn trace_path_from_env() -> Option<PathBuf> {
    std::env::var_os("CORVID_TRACE_PATH").map(PathBuf::from)
}

/// Read a `CorvidString` as an owned Rust `String`.
pub(crate) unsafe fn read_corvid_string(cs: CorvidString) -> String {
    use crate::abi::FromCorvidAbi;
    String::from_corvid_abi(cs)
}

/// Borrow a `CorvidString` as UTF-8 for the duration of the call.
pub(crate) unsafe fn borrow_corvid_string<'a>(cs: &'a CorvidString) -> &'a str {
    unsafe { cs.as_str() }
}

/// Format-instruction text per return type. Sent in the system prompt.
fn format_instruction_int() -> &'static str {
    "Output only a single integer literal — no quotes, no explanation, no formatting, no thousands separators. Examples: 42, -7, 0."
}

fn format_instruction_bool() -> &'static str {
    "Output only the word `true` or `false` — lowercase, no quotes, no explanation, no surrounding text."
}

fn format_instruction_float() -> &'static str {
    "Output only a single decimal number — no quotes, no explanation, no scientific notation prefix beyond what `f64::parse` accepts. Examples: 3.14, -0.5, 42.0."
}

fn using_env_mock_llm() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("CORVID_TEST_MOCK_LLM").ok().as_deref() == Some("1"))
}

/// Build the system prompt sent to the LLM. Encodes the function
/// signature + return-type instruction + (after retries) escalating
/// reminders. `attempt` is 0-indexed; `last_failure` is `Some(text)`
/// on retry attempts and contains the LLM's previous (unparseable)
/// response.
fn build_system_prompt(
    signature: &str,
    format_instruction: &str,
    attempt: u32,
    last_failure: Option<&str>,
) -> String {
    let mut sys = format!(
        "You are a function with signature `{signature}`. The user message contains the rendered prompt body. Compute and return the appropriate value, formatted as follows.\n\nFormat: {format_instruction}"
    );
    if attempt > 0 {
        if let Some(prev) = last_failure {
            sys.push_str(&format!(
                "\n\nIMPORTANT: Your previous response `{prev}` could not be parsed. Respond with ONLY the value in the exact format described above — nothing else, no surrounding text, no explanation."
            ));
        }
        if attempt >= 2 {
            sys.push_str("\n\nThis is your last attempt. The format requirements are absolute.");
        }
    }
    sys
}

/// Single LLM call within the retry loop. Returns the response text
/// (not the parsed value — parsing happens per-return-type in each
/// bridge).
fn call_llm_once(
    state: &BridgeState,
    prompt_name: &str,
    model: &str,
    rendered: &str,
    args: &[serde_json::Value],
    system_prompt: &str,
) -> Result<String, String> {
    let runtime = state.corvid_runtime();
    let combined = if using_env_mock_llm() {
        rendered.to_owned()
    } else {
        // Combine system prompt + user-side rendered prompt with two
        // newlines. Adapters that have native system-prompt support could
        // separate these later; for now the concat is universal.
        let mut combined = String::with_capacity(system_prompt.len() + 2 + rendered.len());
        combined.push_str(system_prompt);
        combined.push_str("\n\n");
        combined.push_str(rendered);
        combined
    };
    let req = LlmRequestRef {
        prompt: prompt_name,
        model,
        rendered: &combined,
        args,
        output_schema: None,
    };
    let resp = state.tokio_handle().block_on(async move {
        runtime.call_llm_ref(req).await
    });
    match resp {
        Ok(r) => match r.value {
            serde_json::Value::String(s) => Ok(s),
            other => Ok(other.to_string()),
        },
        Err(e) => Err(format!("{e}")),
    }
}

#[no_mangle]
pub unsafe extern "C" fn corvid_prompt_call_int(
    prompt_name: CorvidString,
    signature: CorvidString,
    rendered: CorvidString,
    model: CorvidString,
    arg_types: CorvidString,
    argc: i64,
    args_ptr: i64,
) -> i64 {
    let bridge_start = Instant::now();
    let prompt_wait_before = bench_prompt_wait_ns();
    let mock_dispatch_before = bench_mock_dispatch_ns();
    let prompt_name_ref = unsafe { borrow_corvid_string(&prompt_name) };
    let rendered_ref = unsafe { borrow_corvid_string(&rendered) };
    let model_ref = unsafe { borrow_corvid_string(&model) };
    let arg_tags = unsafe { borrow_corvid_string(&arg_types) };
    let llm_args = unsafe { crate::native_trace::decode_trace_values(arg_tags, argc, args_ptr) };

    if using_env_mock_llm() {
        let state = bridge();
        let max_retries = prompt_max_retries();
        let mut last_response: Option<String> = None;
        for _attempt in 0..=max_retries {
            match env_mock_string_reply_sync(prompt_name_ref) {
                Some(reply) => {
                    let reply_text = unsafe { borrow_corvid_string(&reply) }.to_owned();
                    let parsed = parse_int(&reply_text);
                    if let Some(value) = parsed {
                        trace_mock_llm_attempt(
                            state,
                            prompt_name_ref,
                            model_ref,
                            rendered_ref,
                            &llm_args,
                            serde_json::Value::from(value),
                        );
                        unsafe { release_string(reply) };
                        record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
                        return value;
                    }
                    trace_mock_llm_attempt(
                        state,
                        prompt_name_ref,
                        model_ref,
                        rendered_ref,
                        &llm_args,
                        serde_json::Value::String(reply_text.clone()),
                    );
                    unsafe { release_string(reply) };
                    last_response = Some(reply_text);
                }
                None => break,
            }
        }
        if last_response.is_some() {
            record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
            panic!(
                "corvid prompt `{prompt_name_ref}`: could not parse Int from env-mock response after {} attempts. Last response: {:?}",
                max_retries + 1,
                last_response.as_deref().unwrap_or("(none)")
            );
        }
    }

    let state = bridge();
    let prompt_name = prompt_name_ref.to_owned();
    let signature = unsafe { read_corvid_string(signature) };
    let rendered = unsafe { read_corvid_string(rendered) };
    let model = unsafe { read_corvid_string(model) };

    let max_retries = prompt_max_retries();
    let mut last_response: Option<String> = None;
    for attempt in 0..=max_retries {
        let sys = build_system_prompt(
            &signature,
            format_instruction_int(),
            attempt,
            last_response.as_deref(),
        );
        match call_llm_once(state, &prompt_name, &model, &rendered, &llm_args, &sys) {
            Err(e) => {
                if attempt == max_retries {
                    record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
                    panic!(
                        "corvid prompt `{prompt_name}` (model `{model}`): adapter failed after {} attempts: {e}",
                        attempt + 1
                    );
                }
                continue;
            }
            Ok(text) => match parse_int(&text) {
                Some(v) => {
                    record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
                    return v;
                }
                None => last_response = Some(text),
            },
        }
    }
    record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
    panic!(
        "corvid prompt `{prompt_name}` (model `{model}`): could not parse Int from LLM response after {} attempts. Last response: {:?}",
        max_retries + 1,
        last_response.as_deref().unwrap_or("(none)")
    );
}

#[no_mangle]
pub unsafe extern "C" fn corvid_prompt_call_bool(
    prompt_name: CorvidString,
    signature: CorvidString,
    rendered: CorvidString,
    model: CorvidString,
    arg_types: CorvidString,
    argc: i64,
    args_ptr: i64,
) -> bool {
    let bridge_start = Instant::now();
    let prompt_wait_before = bench_prompt_wait_ns();
    let mock_dispatch_before = bench_mock_dispatch_ns();
    let prompt_name_ref = unsafe { borrow_corvid_string(&prompt_name) };
    let rendered_ref = unsafe { borrow_corvid_string(&rendered) };
    let model_ref = unsafe { borrow_corvid_string(&model) };
    let arg_tags = unsafe { borrow_corvid_string(&arg_types) };
    let llm_args = unsafe { crate::native_trace::decode_trace_values(arg_tags, argc, args_ptr) };

    if using_env_mock_llm() {
        let state = bridge();
        let max_retries = prompt_max_retries();
        let mut last_response: Option<String> = None;
        for _attempt in 0..=max_retries {
            match env_mock_string_reply_sync(prompt_name_ref) {
                Some(reply) => {
                    let reply_text = unsafe { borrow_corvid_string(&reply) }.to_owned();
                    let parsed = parse_bool(&reply_text);
                    if let Some(value) = parsed {
                        trace_mock_llm_attempt(
                            state,
                            prompt_name_ref,
                            model_ref,
                            rendered_ref,
                            &llm_args,
                            serde_json::Value::from(value),
                        );
                        unsafe { release_string(reply) };
                        record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
                        return value;
                    }
                    trace_mock_llm_attempt(
                        state,
                        prompt_name_ref,
                        model_ref,
                        rendered_ref,
                        &llm_args,
                        serde_json::Value::String(reply_text.clone()),
                    );
                    unsafe { release_string(reply) };
                    last_response = Some(reply_text);
                }
                None => break,
            }
        }
        if last_response.is_some() {
            record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
            panic!(
                "corvid prompt `{prompt_name_ref}`: could not parse Bool from env-mock response after {} attempts. Last: {:?}",
                max_retries + 1,
                last_response.as_deref().unwrap_or("(none)")
            );
        }
    }

    let state = bridge();
    let prompt_name = prompt_name_ref.to_owned();
    let signature = unsafe { read_corvid_string(signature) };
    let rendered = unsafe { read_corvid_string(rendered) };
    let model = unsafe { read_corvid_string(model) };

    let max_retries = prompt_max_retries();
    let mut last_response: Option<String> = None;
    for attempt in 0..=max_retries {
        let sys = build_system_prompt(
            &signature,
            format_instruction_bool(),
            attempt,
            last_response.as_deref(),
        );
        match call_llm_once(state, &prompt_name, &model, &rendered, &llm_args, &sys) {
            Err(e) => {
                if attempt == max_retries {
                    record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
                    panic!(
                        "corvid prompt `{prompt_name}`: adapter failed after {} attempts: {e}",
                        attempt + 1
                    );
                }
                continue;
            }
            Ok(text) => match parse_bool(&text) {
                Some(v) => {
                    record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
                    return v;
                }
                None => last_response = Some(text),
            },
        }
    }
    record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
    panic!(
        "corvid prompt `{prompt_name}`: could not parse Bool from LLM response after {} attempts. Last: {:?}",
        max_retries + 1,
        last_response.as_deref().unwrap_or("(none)")
    );
}

#[no_mangle]
pub unsafe extern "C" fn corvid_prompt_call_float(
    prompt_name: CorvidString,
    signature: CorvidString,
    rendered: CorvidString,
    model: CorvidString,
    arg_types: CorvidString,
    argc: i64,
    args_ptr: i64,
) -> f64 {
    let bridge_start = Instant::now();
    let prompt_wait_before = bench_prompt_wait_ns();
    let mock_dispatch_before = bench_mock_dispatch_ns();
    let prompt_name_ref = unsafe { borrow_corvid_string(&prompt_name) };
    let rendered_ref = unsafe { borrow_corvid_string(&rendered) };
    let model_ref = unsafe { borrow_corvid_string(&model) };
    let arg_tags = unsafe { borrow_corvid_string(&arg_types) };
    let llm_args = unsafe { crate::native_trace::decode_trace_values(arg_tags, argc, args_ptr) };

    if using_env_mock_llm() {
        let state = bridge();
        let max_retries = prompt_max_retries();
        let mut last_response: Option<String> = None;
        for _attempt in 0..=max_retries {
            match env_mock_string_reply_sync(prompt_name_ref) {
                Some(reply) => {
                    let reply_text = unsafe { borrow_corvid_string(&reply) }.to_owned();
                    let parsed = parse_float(&reply_text);
                    if let Some(value) = parsed {
                        trace_mock_llm_attempt(
                            state,
                            prompt_name_ref,
                            model_ref,
                            rendered_ref,
                            &llm_args,
                            serde_json::Number::from_f64(value)
                                .map(serde_json::Value::Number)
                                .unwrap_or_else(|| serde_json::Value::String(value.to_string())),
                        );
                        unsafe { release_string(reply) };
                        record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
                        return value;
                    }
                    trace_mock_llm_attempt(
                        state,
                        prompt_name_ref,
                        model_ref,
                        rendered_ref,
                        &llm_args,
                        serde_json::Value::String(reply_text.clone()),
                    );
                    unsafe { release_string(reply) };
                    last_response = Some(reply_text);
                }
                None => break,
            }
        }
        if last_response.is_some() {
            record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
            panic!(
                "corvid prompt `{prompt_name_ref}`: could not parse Float from env-mock response after {} attempts. Last: {:?}",
                max_retries + 1,
                last_response.as_deref().unwrap_or("(none)")
            );
        }
    }

    let state = bridge();
    let prompt_name = prompt_name_ref.to_owned();
    let signature = unsafe { read_corvid_string(signature) };
    let rendered = unsafe { read_corvid_string(rendered) };
    let model = unsafe { read_corvid_string(model) };

    let max_retries = prompt_max_retries();
    let mut last_response: Option<String> = None;
    for attempt in 0..=max_retries {
        let sys = build_system_prompt(
            &signature,
            format_instruction_float(),
            attempt,
            last_response.as_deref(),
        );
        match call_llm_once(state, &prompt_name, &model, &rendered, &llm_args, &sys) {
            Err(e) => {
                if attempt == max_retries {
                    record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
                    panic!(
                        "corvid prompt `{prompt_name}`: adapter failed after {} attempts: {e}",
                        attempt + 1
                    );
                }
                continue;
            }
            Ok(text) => match parse_float(&text) {
                Some(v) => {
                    record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
                    return v;
                }
                None => last_response = Some(text),
            },
        }
    }
    record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
    panic!(
        "corvid prompt `{prompt_name}`: could not parse Float from LLM response after {} attempts. Last: {:?}",
        max_retries + 1,
        last_response.as_deref().unwrap_or("(none)")
    );
}

#[no_mangle]
pub unsafe extern "C" fn corvid_prompt_call_string(
    prompt_name: CorvidString,
    signature: CorvidString,
    rendered: CorvidString,
    model: CorvidString,
    arg_types: CorvidString,
    argc: i64,
    args_ptr: i64,
) -> CorvidString {
    use crate::abi::IntoCorvidAbi;
    let bridge_start = Instant::now();
    let prompt_wait_before = bench_prompt_wait_ns();
    let mock_dispatch_before = bench_mock_dispatch_ns();
    let prompt_name_ref = unsafe { borrow_corvid_string(&prompt_name) };
    let rendered_ref = unsafe { borrow_corvid_string(&rendered) };
    let model_ref = unsafe { borrow_corvid_string(&model) };
    let arg_tags = unsafe { borrow_corvid_string(&arg_types) };
    let llm_args = unsafe { crate::native_trace::decode_trace_values(arg_tags, argc, args_ptr) };

    if using_env_mock_llm() {
        let state = bridge();
        if let Some(text) = env_mock_string_reply_sync(prompt_name_ref) {
            trace_mock_llm_attempt(
                state,
                prompt_name_ref,
                model_ref,
                rendered_ref,
                &llm_args,
                serde_json::Value::String(unsafe { borrow_corvid_string(&text) }.to_owned()),
            );
            record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
            return text;
        }
    }

    let state = bridge();
    let prompt_name = prompt_name_ref.to_owned();
    let signature = unsafe { read_corvid_string(signature) };
    let rendered = unsafe { read_corvid_string(rendered) };
    let model = unsafe { read_corvid_string(model) };

    // String return: no parse-retry loop. Whatever the LLM returns
    // IS the String. We still call once, and on adapter failure we
    // panic with a clear message — adapter errors are infrastructure
    // problems, not response-format problems.
    let sys = if using_env_mock_llm() {
        String::new()
    } else {
        format!(
            "You are a function with signature `{signature}`. Return the appropriate string value as your full response — no quotes around the value, no explanation, no formatting markers."
        )
    };
    match call_llm_once(state, &prompt_name, &model, &rendered, &llm_args, &sys) {
        Ok(text) => {
            record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
            text.into_corvid_abi()
        }
        Err(e) => panic!(
            "corvid prompt `{prompt_name}` (model `{model}`): adapter failed: {e}"
        ),
    }
}

#[no_mangle]
pub unsafe extern "C" fn corvid_approve_sync(
    label: CorvidString,
    arg_types: CorvidString,
    argc: i64,
    args_ptr: i64,
) -> bool {
    let label = unsafe { read_corvid_string(label) };
    let arg_tags = unsafe { borrow_corvid_string(&arg_types) };
    let args = unsafe { crate::native_trace::decode_trace_values(arg_tags, argc, args_ptr) };
    let state = bridge();
    let runtime = state.corvid_runtime();
    let label_for_call = label.clone();
    let result = state
        .tokio_handle()
        .block_on(async move { runtime.approval_gate(&label_for_call, args).await });
    match result {
        Ok(()) => true,
        Err(e) => {
            eprintln!("corvid_approve_sync: approval `{label}` failed: {e}");
            false
        }
    }
}

/// Parse helpers — tolerant of common LLM quirks (surrounding quotes,
/// whitespace, code-fence wrappers).
fn strip_response(s: &str) -> &str {
    let t = s.trim();
    // Strip a single layer of code-fence: ```...```, ```rust...```, ```\n...```
    if t.starts_with("```") && t.ends_with("```") && t.len() >= 6 {
        let inner = &t[3..t.len() - 3];
        // Trim a leading language tag like ```rust\n...
        let after_lang = inner
            .find('\n')
            .map(|nl| &inner[nl + 1..])
            .unwrap_or(inner);
        return after_lang.trim();
    }
    t
}

fn parse_int(s: &str) -> Option<i64> {
    let t = strip_response(s);
    let t = t.trim_matches(|c: char| c == '"' || c == '\'').trim();
    t.parse::<i64>().ok()
}

fn parse_bool(s: &str) -> Option<bool> {
    let t = strip_response(s).trim().trim_matches(|c: char| c == '"' || c == '\'');
    match t.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

fn parse_float(s: &str) -> Option<f64> {
    let t = strip_response(s).trim().trim_matches(|c: char| c == '"' || c == '\'');
    t.parse::<f64>().ok()
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
    let tracer = if std::env::var("CORVID_TRACE_DISABLE").ok().as_deref() == Some("1") {
        Tracer::null()
    } else if let Some(trace_path) = trace_path_from_env() {
        Tracer::open_path(trace_path, fresh_run_id())
            .with_redaction(RedactionSet::from_env())
    } else {
        let trace_dir = trace_dir_for_current_process();
        Tracer::open(&trace_dir, fresh_run_id())
            .with_redaction(RedactionSet::from_env())
    };

    let mut b: RuntimeBuilder = Runtime::builder()
        .tracer(tracer)
        .trace_schema_writer(WRITER_NATIVE);

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
    if let Ok(seed) = std::env::var("CORVID_ROLLOUT_SEED") {
        if let Ok(parsed) = seed.parse::<u64>() {
            b = b.rollout_seed(parsed);
        }
    }

    // Register every supported LLM adapter unconditionally
    // so the model-prefix dispatch in `LlmRegistry::call` can route
    // any `CORVID_MODEL` to its provider. Adapters that need an API
    // key fall back to an empty string when the env var is missing —
    // calls then surface as `HTTP 401` from the provider, which is
    // a clearer failure than silently routing nowhere.
    //
    // Test-mode env-var mock takes PRECEDENCE: when
    // `CORVID_TEST_MOCK_LLM=1`, the mock handles every model spec
    // (its `handles` returns true unconditionally), avoiding real
    // API calls in CI even when keys leak into the env.
    if std::env::var("CORVID_TEST_MOCK_LLM").ok().as_deref() == Some("1") {
        b = b.llm(Arc::new(EnvVarMockAdapter::from_env()));
    }
    let anthropic_key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();
    b = b.llm(Arc::new(AnthropicAdapter::new(anthropic_key)));
    let openai_key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
    b = b.llm(Arc::new(OpenAiAdapter::new(openai_key)));
    let gemini_key = std::env::var("GOOGLE_API_KEY")
        .or_else(|_| std::env::var("GEMINI_API_KEY"))
        .unwrap_or_default();
    b = b.llm(Arc::new(GeminiAdapter::new(gemini_key)));
    // Ollama is local, no key. OpenAI-compat key is optional.
    b = b.llm(Arc::new(OllamaAdapter::new()));
    let compat_key = std::env::var("OPENAI_COMPAT_API_KEY").unwrap_or_default();
    b = b.llm(Arc::new(OpenAiCompatibleAdapter::new(compat_key)));

    // Test-only mock-tool registration. Format:
    //   CORVID_TEST_MOCK_INT_TOOLS="name1:value1;name2:value2"
    //
    // Each name becomes a tool that ignores its args and returns the
    // given Int. Used by the parity harness to exercise the compiled
    // tool-call path before the user-facing proc-macro
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
// the early link flow lands.
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
