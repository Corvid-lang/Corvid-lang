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
use crate::llm::mock::{
    bench_mock_dispatch_ns, bench_prompt_wait_ns, env_mock_string_reply_sync,
};
use crate::errors::RuntimeError;
use corvid_trace_schema::TraceEvent;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::OnceLock;
use std::time::Instant;

mod state;
mod strings;
mod tokio_handle;
pub use state::{
    corvid_bench_approval_wait_ns, corvid_bench_json_bridge_ns, corvid_bench_mock_dispatch_ns,
    corvid_bench_prompt_wait_ns, corvid_bench_tool_wait_ns, corvid_bench_trace_overhead_ns,
    corvid_runtime_embed_init_default, corvid_runtime_init, corvid_runtime_is_replay,
    corvid_runtime_probe, corvid_runtime_shutdown, runtime,
};
pub use strings::{
    corvid_free_string, corvid_string_into_cstr, release_string, string_from_rust,
    string_from_static_str,
};
pub use tokio_handle::tokio_handle;
pub(crate) use state::bridge;
pub(crate) use strings::{borrow_corvid_string, read_corvid_string};
use state::{record_json_bridge_ns, BridgeState, BRIDGE};
use tokio_handle::{build_corvid_runtime, build_embedded_corvid_runtime, build_tokio_runtime};


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

fn replay_tool_value(name: &str, args: Vec<serde_json::Value>) -> serde_json::Value {
    let state = bridge();
    let runtime = state.corvid_runtime();
    let name_owned = name.to_string();
    match state
        .tokio_handle()
        .block_on(async move { runtime.call_tool(&name_owned, args).await })
    {
        Ok(value) => value,
        Err(err) => {
            panic_if_replay_runtime_error(
                &format!("corvid native replay tool `{name}` failed"),
                &err,
            );
            panic!("corvid native replay tool `{name}` failed: {err}");
        }
    }
}

fn expect_tool_result_int(name: &str, value: serde_json::Value) -> i64 {
    value
        .as_i64()
        .unwrap_or_else(|| panic!("corvid native replay tool `{name}` returned non-int JSON: {value}"))
}

fn expect_tool_result_bool(name: &str, value: serde_json::Value) -> bool {
    value
        .as_bool()
        .unwrap_or_else(|| panic!("corvid native replay tool `{name}` returned non-bool JSON: {value}"))
}

fn expect_tool_result_float(name: &str, value: serde_json::Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("corvid native replay tool `{name}` returned non-float JSON: {value}"))
}

fn expect_tool_result_string(name: &str, value: serde_json::Value) -> String {
    value
        .as_str()
        .unwrap_or_else(|| panic!("corvid native replay tool `{name}` returned non-string JSON: {value}"))
        .to_owned()
}

fn expect_tool_result_null(name: &str, value: serde_json::Value) {
    if !value.is_null() {
        panic!("corvid native replay tool `{name}` returned non-null JSON: {value}");
    }
}

#[no_mangle]
pub unsafe extern "C" fn corvid_replay_tool_call_int(
    tool: CorvidString,
    arg_types: CorvidString,
    argc: i64,
    args_ptr: i64,
) -> i64 {
    let tool_name = unsafe { read_corvid_string(tool) };
    let arg_tags = unsafe { borrow_corvid_string(&arg_types) };
    let args = unsafe { crate::native_trace::decode_trace_values(arg_tags, argc, args_ptr) };
    expect_tool_result_int(&tool_name, replay_tool_value(&tool_name, args))
}

#[no_mangle]
pub unsafe extern "C" fn corvid_replay_tool_call_bool(
    tool: CorvidString,
    arg_types: CorvidString,
    argc: i64,
    args_ptr: i64,
) -> bool {
    let tool_name = unsafe { read_corvid_string(tool) };
    let arg_tags = unsafe { borrow_corvid_string(&arg_types) };
    let args = unsafe { crate::native_trace::decode_trace_values(arg_tags, argc, args_ptr) };
    expect_tool_result_bool(&tool_name, replay_tool_value(&tool_name, args))
}

#[no_mangle]
pub unsafe extern "C" fn corvid_replay_tool_call_float(
    tool: CorvidString,
    arg_types: CorvidString,
    argc: i64,
    args_ptr: i64,
) -> f64 {
    let tool_name = unsafe { read_corvid_string(tool) };
    let arg_tags = unsafe { borrow_corvid_string(&arg_types) };
    let args = unsafe { crate::native_trace::decode_trace_values(arg_tags, argc, args_ptr) };
    expect_tool_result_float(&tool_name, replay_tool_value(&tool_name, args))
}

#[no_mangle]
pub unsafe extern "C" fn corvid_replay_tool_call_string(
    tool: CorvidString,
    arg_types: CorvidString,
    argc: i64,
    args_ptr: i64,
) -> CorvidString {
    use crate::abi::IntoCorvidAbi;

    let tool_name = unsafe { read_corvid_string(tool) };
    let arg_tags = unsafe { borrow_corvid_string(&arg_types) };
    let args = unsafe { crate::native_trace::decode_trace_values(arg_tags, argc, args_ptr) };
    expect_tool_result_string(&tool_name, replay_tool_value(&tool_name, args)).into_corvid_abi()
}

#[no_mangle]
pub unsafe extern "C" fn corvid_replay_tool_call_nothing(
    tool: CorvidString,
    arg_types: CorvidString,
    argc: i64,
    args_ptr: i64,
) {
    let tool_name = unsafe { read_corvid_string(tool) };
    let arg_tags = unsafe { borrow_corvid_string(&arg_types) };
    let args = unsafe { crate::native_trace::decode_trace_values(arg_tags, argc, args_ptr) };
    expect_tool_result_null(&tool_name, replay_tool_value(&tool_name, args));
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
        model_version: runtime.model_version(effective_model),
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
        model_version: runtime.model_version(effective_model),
        result,
    });
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

pub(super) fn trace_path_from_env() -> Option<PathBuf> {
    std::env::var_os("CORVID_TRACE_PATH").map(PathBuf::from)
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
    let enabled =
        *ENABLED.get_or_init(|| std::env::var("CORVID_TEST_MOCK_LLM").ok().as_deref() == Some("1"));
    enabled && !BRIDGE.load(Ordering::Acquire).is_null() && !bridge().corvid_runtime().is_replay_mode()
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
    let combined = if using_env_mock_llm() || (runtime.is_replay_mode() && !runtime.replay_uses_live_llm()) {
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
        runtime
            .call_llm_ref_with_trace_rendered(req, Some(rendered))
            .await
    });
    match resp {
        Ok(r) => match r.value {
            serde_json::Value::String(s) => Ok(s),
            other => Ok(other.to_string()),
        },
        Err(e) => {
            panic_if_replay_runtime_error(
                &format!("corvid prompt `{prompt_name}` (model `{model}`) replay failed"),
                &e,
            );
            Err(format!("{e}"))
        }
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
pub unsafe extern "C" fn corvid_citation_verify_or_panic(
    prompt_name: CorvidString,
    context: CorvidString,
    response: CorvidString,
) -> bool {
    let prompt_name_ref = unsafe { borrow_corvid_string(&prompt_name) };
    let context_ref = unsafe { borrow_corvid_string(&context) };
    let response_ref = unsafe { borrow_corvid_string(&response) };
    if crate::citation::citation_verified(context_ref, response_ref) {
        return true;
    }

    panic!(
        "citation verification failed for prompt `{prompt_name_ref}`: response does not reference content from the cited context parameter"
    );
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
            panic_if_replay_runtime_error(
                &format!("corvid_approve_sync: approval `{label}` failed"),
                &e,
            );
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

