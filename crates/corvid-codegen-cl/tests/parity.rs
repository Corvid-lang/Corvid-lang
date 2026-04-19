//! Interpreter-vs-compiled-binary parity tests.
//!
//! Every fixture compiles to a native binary, runs it, and compares the
//! binary's stdout (the printed `i64`) to the interpreter's `Value::Int`.
//! If the two tiers disagree on any fixture, the harness fails — that's
//! the oracle property the early async decision defended.
//!
//! Int-only, pure computation, parameter-less entry fixtures
//! agents (the C shim can't pass argv yet). Arithmetic overflow paths
//! assert on stderr + non-zero exit instead of value equality.

use corvid_codegen_cl::build_native_to_disk;
use corvid_ir::lower;
use corvid_resolve::resolve;
use corvid_runtime::llm::LlmRequestRef;
use corvid_runtime::{
    LlmAdapter, LlmResponse, ProgrammaticApprover, Runtime, RuntimeError, TokenUsage,
};
use corvid_syntax::{lex, parse_file};
use corvid_types::typecheck;
use corvid_vm::Value;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Command;
use std::sync::{Arc, Mutex};

#[path = "parity/int.rs"]
mod int;
#[path = "parity/bool.rs"]
mod bool;
#[path = "parity/float.rs"]
mod float;
#[path = "parity/string.rs"]
mod string;
#[path = "parity/structs.rs"]
mod structs;
#[path = "parity/list.rs"]
mod list;
#[path = "parity/entry.rs"]
mod entry;
#[path = "parity/tool.rs"]
mod tool;
#[path = "parity/prompt.rs"]
mod prompt;
#[path = "parity/method.rs"]
mod method;
#[path = "parity/weak.rs"]
mod weak;

/// Path to the `corvid-test-tools` staticlib. The parity harness links
/// this into every compiled Corvid binary so `#[tool]`-declared mocks
/// are available for fixtures that exercise tool calls (typed tool
/// onwards). Pure-computation fixtures don't call into it — the dead
/// symbols are stripped by the linker.
fn test_tools_lib_path() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is this crate's dir; walk up to workspace
    // root, then into `target/release/` for the staticlib.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf();
    let name = if cfg!(windows) {
        "corvid_test_tools.lib"
    } else {
        "libcorvid_test_tools.a"
    };
    workspace_root.join("target").join("release").join(name)
}

/// Run a compiled binary with the leak detector enabled. Returns
/// (stdout, stderr, exit_status). Caller is responsible for verifying
/// stdout / status; this helper handles the env var.
fn run_with_leak_detector(bin: &std::path::Path) -> (String, String, std::process::ExitStatus) {
    let output = Command::new(bin)
        .env("CORVID_DEBUG_ALLOC", "1")
        .output()
        .expect("run compiled binary");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    (stdout, stderr, output.status)
}

fn compile_without_tools_lib(
    ir: &corvid_ir::IrFile,
    bin_path: &std::path::Path,
) -> std::path::PathBuf {
    build_native_to_disk(ir, "corvid_parity_test", bin_path, &[]).expect("compile + link")
}

/// Run a binary with typed tool-return values set via env vars.
/// Each entry maps a Corvid tool name (e.g. `"answer"`) to the Int
/// value the test wants that tool to return. The helper translates
/// the name to the env-var key the test-tools staticlib reads (e.g.
/// `CORVID_TEST_TOOL_ANSWER`).
///
/// Matches the pattern `crates/corvid-test-tools/src/lib.rs` uses:
/// each `#[tool]` there reads its value from `env_i64(...)` during
/// dispatch, so per-fixture env vars tune behaviour without rebuilding.
fn run_with_leak_detector_and_mocks(
    bin: &std::path::Path,
    mocks: &[(&str, i64)],
) -> (String, String, std::process::ExitStatus) {
    let mut cmd = Command::new(bin);
    cmd.env("CORVID_DEBUG_ALLOC", "1")
        .env("CORVID_APPROVE_AUTO", "1");
    for (name, value) in mocks {
        let key = tool_env_var_name(name);
        cmd.env(&key, value.to_string());
    }
    let output = cmd.output().expect("run compiled binary");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    (stdout, stderr, output.status)
}

/// Map a Corvid tool name to the env-var key `corvid-test-tools`
/// reads. Convention: `CORVID_TEST_TOOL_<UPPER(name)>`.
fn tool_env_var_name(tool_name: &str) -> String {
    format!("CORVID_TEST_TOOL_{}", tool_name.to_ascii_uppercase())
}

struct QueuedMockAdapter {
    name: String,
    replies: Mutex<HashMap<String, VecDeque<serde_json::Value>>>,
}

impl QueuedMockAdapter {
    fn new(
        model_name: impl Into<String>,
        replies: HashMap<String, VecDeque<serde_json::Value>>,
    ) -> Self {
        Self {
            name: model_name.into(),
            replies: Mutex::new(replies),
        }
    }
}

impl LlmAdapter for QueuedMockAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn handles(&self, model: &str) -> bool {
        model == self.name
    }

    fn call<'a>(
        &'a self,
        req: &'a LlmRequestRef<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<LlmResponse, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let value = {
                let mut replies = self.replies.lock().unwrap();
                replies
                    .get_mut(req.prompt)
                    .and_then(|queue| queue.pop_front())
            }
            .ok_or_else(|| RuntimeError::AdapterFailed {
                adapter: self.name.clone(),
                message: format!("no queued reply registered for prompt `{}`", req.prompt),
            })?;
            Ok(LlmResponse {
                value,
                usage: TokenUsage::default(),
            })
        })
    }
}

/// Parse `ALLOCS=N` and `RELEASES=N` from the stderr output the shim
/// emits when `CORVID_DEBUG_ALLOC=1`. Asserts they are equal — any
/// mismatch means the codegen forgot a `corvid_release` somewhere.
#[track_caller]
fn assert_no_leaks(stderr: &str, src: &str) {
    let mut allocs: Option<i64> = None;
    let mut releases: Option<i64> = None;
    for line in stderr.lines() {
        if let Some(rest) = line.strip_prefix("ALLOCS=") {
            allocs = rest.trim().parse().ok();
        } else if let Some(rest) = line.strip_prefix("RELEASES=") {
            releases = rest.trim().parse().ok();
        }
    }
    let a = allocs.unwrap_or_else(|| {
        panic!("expected `ALLOCS=N` in stderr, got: {stderr}; src=\n{src}")
    });
    let r = releases.unwrap_or_else(|| {
        panic!("expected `RELEASES=N` in stderr, got: {stderr}; src=\n{src}")
    });
    assert_eq!(
        a, r,
        "leak detected: ALLOCS={a} RELEASES={r} (delta={}); src=\n{src}",
        a - r
    );
}

/// End-to-end pipeline: source → IR → both tiers → assertion.
///
/// `expected` is the `Int` value both tiers should produce.
#[track_caller]
fn assert_parity(src: &str, expected: i64) {
    let ir = ir_of(src);

    // --- Interpreter tier ---
    let runtime = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .build();
    let interp_value = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async { corvid_vm::run_agent(&ir, entry_name(&ir), vec![], &runtime).await })
        .expect("interpreter run");

    assert_eq!(
        interp_value,
        Value::Int(expected),
        "interpreter result mismatch for source:\n{src}"
    );

    // --- Compiled binary tier ---
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_path = tmp.path().join("prog");
    let produced = build_native_to_disk(&ir, "corvid_parity_test", &bin_path, &[test_tools_lib_path().as_path()])
        .expect("compile + link");

    let (stdout, stderr, status) = run_with_leak_detector(&produced);
    assert!(
        status.success(),
        "compiled binary exited non-zero: status={:?} stderr={stderr} stdout={stdout} src=\n{src}",
        status.code()
    );
    let compiled_value: i64 = stdout
        .trim()
        .lines()
        .next()
        .unwrap_or("")
        .parse()
        .unwrap_or_else(|e| panic!("parse stdout `{stdout}` as i64: {e}; src=\n{src}"));
    assert_eq!(
        compiled_value, expected,
        "compiled result mismatch for source:\n{src}\nstderr: {stderr}"
    );
    assert_no_leaks(&stderr, src);
}

/// Like `assert_parity` but for agents that return `Bool`. Interpreter
/// returns `Value::Bool`; the compiled binary's trampoline zero-extends
/// the `I8` to `I64` so stdout prints `0` or `1`.
#[track_caller]
fn assert_parity_bool(src: &str, expected: bool) {
    let ir = ir_of(src);

    let runtime = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .build();
    let interp_value = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async { corvid_vm::run_agent(&ir, entry_name(&ir), vec![], &runtime).await })
        .expect("interpreter run");
    assert_eq!(
        interp_value,
        Value::Bool(expected),
        "interpreter result mismatch for source:\n{src}"
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_path = tmp.path().join("prog");
    let produced = build_native_to_disk(&ir, "corvid_parity_test", &bin_path, &[test_tools_lib_path().as_path()])
        .expect("compile + link");
    let (stdout, stderr, status) = run_with_leak_detector(&produced);
    assert!(
        status.success(),
        "compiled binary exited non-zero: status={:?} stderr={stderr} stdout={stdout} src=\n{src}",
        status.code()
    );
    // Codegen-emitted main prints Bool as "true"/"false"
    // via `corvid_print_bool` (was "1"/"0" via the old shim main).
    // Accept both for resilience against future format tweaks.
    let printed = stdout.trim().lines().next().unwrap_or("");
    let compiled_bool = match printed {
        "true" | "1" => true,
        "false" | "0" => false,
        other => panic!(
            "expected `true` / `false` / `1` / `0` for Bool, got `{other}`; src=\n{src}"
        ),
    };
    assert_eq!(
        compiled_bool, expected,
        "compiled result mismatch for source:\n{src}\nstderr: {stderr}"
    );
    assert_no_leaks(&stderr, src);
}

#[track_caller]
fn assert_parity_bool_without_tools(src: &str, expected: bool) {
    let ir = ir_of(src);

    let runtime = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .build();
    let interp_value = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async { corvid_vm::run_agent(&ir, entry_name(&ir), vec![], &runtime).await })
        .expect("interpreter run");
    assert_eq!(
        interp_value,
        Value::Bool(expected),
        "interpreter result mismatch for source:\n{src}"
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_path = tmp.path().join("prog");
    let produced = compile_without_tools_lib(&ir, &bin_path);
    let (stdout, stderr, status) = run_with_leak_detector(&produced);
    assert!(
        status.success(),
        "compiled binary exited non-zero: status={:?} stderr={stderr} stdout={stdout} src=\n{src}",
        status.code()
    );
    let printed = stdout.trim().lines().next().unwrap_or("");
    let compiled_bool = match printed {
        "true" | "1" => true,
        "false" | "0" => false,
        other => panic!(
            "expected `true` / `false` / `1` / `0` for Bool, got `{other}`; src=\n{src}"
        ),
    };
    assert_eq!(
        compiled_bool, expected,
        "compiled result mismatch for source:\n{src}\nstderr: {stderr}"
    );
    assert_no_leaks(&stderr, src);
}

/// Assert both tiers raise an overflow / divide-by-zero error. Interpreter
/// returns `Err(InterpError { Arithmetic })`; compiled binary exits
/// non-zero with "integer overflow" on stderr.
#[track_caller]
fn assert_parity_overflow(src: &str) {
    let ir = ir_of(src);

    let runtime = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .build();
    let interp_err = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async { corvid_vm::run_agent(&ir, entry_name(&ir), vec![], &runtime).await })
        .expect_err("interpreter should raise on overflow / div-zero");
    let msg = format!("{}", interp_err.kind);
    assert!(
        msg.contains("overflow") || msg.contains("division") || msg.contains("modulo"),
        "interpreter error didn't mention overflow / division: {msg}; src=\n{src}"
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_path = tmp.path().join("prog");
    let produced = build_native_to_disk(&ir, "corvid_parity_test", &bin_path, &[test_tools_lib_path().as_path()])
        .expect("compile + link");
    let output = Command::new(&produced).output().expect("run compiled binary");
    assert!(
        !output.status.success(),
        "compiled binary should have exited non-zero on overflow/div-zero: stdout={}",
        String::from_utf8_lossy(&output.stdout),
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("integer overflow") || stderr.contains("division"),
        "stderr didn't mention overflow: {stderr}; src=\n{src}"
    );
}

fn ir_of(src: &str) -> corvid_ir::IrFile {
    let tokens = lex(src).expect("lex");
    let (file, perr) = parse_file(&tokens);
    assert!(perr.is_empty(), "parse: {perr:?}");
    let resolved = resolve(&file);
    assert!(resolved.errors.is_empty(), "resolve: {:?}", resolved.errors);
    let checked = typecheck(&file, &resolved);
    assert!(checked.errors.is_empty(), "typecheck: {:?}", checked.errors);
    lower(&file, &resolved, &checked)
}

fn entry_name(ir: &corvid_ir::IrFile) -> &str {
    if ir.agents.len() == 1 {
        return ir.agents[0].name.as_str();
    }
    ir.agents
        .iter()
        .find(|a| a.name == "main")
        .map(|a| a.name.as_str())
        .expect("multiple agents need a `main`")
}

// ============================================================
// Fixtures
// ============================================================



// ============================================================
// Native-tier prompt dispatch fixtures through the
// LlmRegistry + adapter pipeline.
//
// Every fixture uses the env-var mock LLM (`CORVID_TEST_MOCK_LLM=1`,
// response from `CORVID_TEST_MOCK_LLM_RESPONSE`) so we don't need
// real provider API keys in CI. The compiled binary's
// `corvid_runtime_init` registers the mock as the FIRST adapter,
// so its wildcard `handles()` claims every model spec — real
// providers (Anthropic / OpenAI / Gemini / Ollama / openai-compat)
// remain registered but never get hit.
// ============================================================

#[track_caller]
fn assert_parity_bool_with_mock_llm_queue(
    src: &str,
    prompt_name: &str,
    replies: Vec<serde_json::Value>,
    expected: bool,
    model: &str,
) {
    let ir = ir_of(src);

    let mut queued = HashMap::new();
    queued.insert(prompt_name.to_string(), replies.clone().into());
    let runtime = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(QueuedMockAdapter::new(model, queued)))
        .default_model(model)
        .build();
    let interp_value = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async { corvid_vm::run_agent(&ir, entry_name(&ir), vec![], &runtime).await })
        .expect("interpreter run");
    assert_eq!(
        interp_value,
        Value::Bool(expected),
        "interpreter result mismatch for src:\n{src}"
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_path = tmp.path().join("prog");
    let produced = compile_without_tools_lib(&ir, &bin_path);
    let output = Command::new(&produced)
        .env("CORVID_DEBUG_ALLOC", "1")
        .env("CORVID_APPROVE_AUTO", "1")
        .env("CORVID_TEST_MOCK_LLM", "1")
        .env(
            "CORVID_TEST_MOCK_LLM_REPLIES",
            serde_json::json!({ prompt_name: replies }).to_string(),
        )
        .env("CORVID_MODEL", model)
        .output()
        .expect("run compiled binary");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "compiled binary exited non-zero: status={:?} stdout={stdout} stderr={stderr} src=\n{src}",
        output.status.code()
    );
    let printed = stdout.trim().lines().next().unwrap_or("");
    let compiled_bool = match printed {
        "true" | "1" => true,
        "false" | "0" => false,
        other => panic!(
            "expected `true` / `false` / `1` / `0` for Bool, got `{other}`; src=\n{src}"
        ),
    };
    assert_eq!(
        compiled_bool, expected,
        "compiled result mismatch for src:\n{src}\nstderr: {stderr}"
    );
    assert_no_leaks(&stderr, src);
}



#[test]
fn nullable_option_string_round_trips_through_native_agents() {
    assert_parity_bool_without_tools(
        "agent maybe(flag: Bool) -> Option<String>:\n    if flag:\n        return Some(\"hi\")\n    return None\n\nagent main() -> Bool:\n    value = maybe(true)\n    return value != None\n",
        true,
    );
}

#[test]
fn nullable_option_string_none_compares_equal_to_none() {
    assert_parity_bool_without_tools(
        "agent maybe(flag: Bool) -> Option<String>:\n    if flag:\n        return Some(\"hi\")\n    return None\n\nagent main() -> Bool:\n    value = maybe(false)\n    return value == None\n",
        true,
    );
}

#[test]
fn nullable_option_string_try_propagates_some_and_none() {
    assert_parity_bool_without_tools(
        "agent maybe(flag: Bool) -> Option<String>:\n    if flag:\n        return Some(\"hi\")\n    return None\n\nagent unwrap(flag: Bool) -> Option<String>:\n    value = maybe(flag)?\n    return Some(value)\n\nagent main() -> Bool:\n    return unwrap(false) == None and unwrap(true) != None\n",
        true,
    );
}

#[test]
fn wide_option_int_round_trips_through_native_agents() {
    assert_parity_bool_without_tools(
        "agent maybe(flag: Bool) -> Option<Int>:\n    if flag:\n        return Some(7)\n    return None\n\nagent main() -> Bool:\n    value = maybe(true)\n    return value != None\n",
        true,
    );
}

#[test]
fn wide_option_int_try_propagates_some_and_none() {
    assert_parity_bool_without_tools(
        "agent maybe(flag: Bool) -> Option<Int>:\n    if flag:\n        return Some(7)\n    return None\n\nagent unwrap(flag: Bool) -> Option<Int>:\n    value = maybe(flag)?\n    return Some(value + 1)\n\nagent main() -> Bool:\n    return unwrap(false) == None and unwrap(true) != None\n",
        true,
    );
}

#[test]
fn wide_option_int_try_propagates_into_different_outer_option_type() {
    assert_parity_bool_without_tools(
        "agent maybe(flag: Bool) -> Option<Int>:\n    if flag:\n        return Some(7)\n    return None\n\nagent widen(flag: Bool) -> Option<Bool>:\n    value = maybe(flag)?\n    return Some(value > 0)\n\nagent main() -> Bool:\n    return widen(false) == None and widen(true) != None\n",
        true,
    );
}

#[test]
fn nullable_option_string_try_propagates_into_wide_outer_option_type() {
    assert_parity_bool_without_tools(
        "agent maybe(flag: Bool) -> Option<String>:\n    if flag:\n        return Some(\"hi\")\n    return None\n\nagent widen(flag: Bool) -> Option<Bool>:\n    value = maybe(flag)?\n    return Some(value == \"hi\")\n\nagent main() -> Bool:\n    return widen(false) == None and widen(true) != None\n",
        true,
    );
}

#[test]
fn native_option_retry_retries_until_some() {
    assert_parity_bool_with_mock_llm_queue(
        "prompt probe() -> String:\n    \"Probe\"\n\nagent fetch() -> Option<Int>:\n    value = probe()\n    if value == \"ok\":\n        return Some(7)\n    return None\n\nagent main() -> Bool:\n    outcome = try fetch() on error retry 3 times backoff linear 0\n    return probe() == \"marker\"\n",
        "probe",
        vec![
            serde_json::json!("bad"),
            serde_json::json!("bad"),
            serde_json::json!("ok"),
            serde_json::json!("marker"),
        ],
        true,
        "mock-1",
    );
}

#[test]
fn native_option_retry_returns_none_after_exhausting_attempts() {
    assert_parity_bool_with_mock_llm_queue(
        "prompt probe() -> String:\n    \"Probe\"\n\nagent fetch() -> Option<Int>:\n    value = probe()\n    if value == \"ok\":\n        return Some(7)\n    return None\n\nagent main() -> Bool:\n    outcome = try fetch() on error retry 3 times backoff exponential 0\n    return outcome == None and probe() == \"marker\"\n",
        "probe",
        vec![
            serde_json::json!("bad"),
            serde_json::json!("bad"),
            serde_json::json!("bad"),
            serde_json::json!("marker"),
        ],
        true,
        "mock-1",
    );
}

#[test]
fn native_nested_option_int_distinguishes_none_from_some_none() {
    assert_parity_bool_without_tools(
        "agent fetch(mode: Int) -> Option<Option<Int>>:\n    if mode == 0:\n        return None\n    if mode == 1:\n        return Some(None)\n    return Some(Some(7))\n\nagent main() -> Bool:\n    first = fetch(0)\n    second = fetch(1)\n    third = fetch(2)\n    return first == None and second != None and third != None\n",
        true,
    );
}

#[test]
fn native_nested_option_try_propagates_outer_none_and_preserves_inner_option() {
    assert_parity_bool_without_tools(
        "agent fetch(mode: Int) -> Option<Option<Int>>:\n    if mode == 0:\n        return None\n    if mode == 1:\n        return Some(None)\n    return Some(Some(7))\n\nagent inspect(mode: Int) -> Option<Bool>:\n    value = fetch(mode)?\n    return Some(value == None or value != None)\n\nagent main() -> Bool:\n    return inspect(0) == None and inspect(1) != None and inspect(2) != None\n",
        true,
    );
}

#[test]
fn wide_option_bool_none_compares_equal_to_none() {
    assert_parity_bool_without_tools(
        "agent maybe(flag: Bool) -> Option<Bool>:\n    if flag:\n        return Some(true)\n    return None\n\nagent main() -> Bool:\n    return maybe(false) == None and maybe(true) != None\n",
        true,
    );
}

#[test]
fn native_result_string_round_trips_through_native_agents() {
    assert_parity_bool_without_tools(
        "agent fetch(flag: Bool) -> Result<String, String>:\n    if flag:\n        return Ok(\"hi\")\n    return Err(\"no\")\n\nagent main() -> Bool:\n    first = fetch(true)\n    second = fetch(false)\n    return true\n",
        true,
    );
}

#[test]
fn native_result_string_try_propagates_ok_and_err() {
    assert_parity_bool_without_tools(
        "agent fetch(flag: Bool) -> Result<String, String>:\n    if flag:\n        return Ok(\"hi\")\n    return Err(\"no\")\n\nagent forward(flag: Bool) -> Result<String, String>:\n    value = fetch(flag)?\n    return Ok(value)\n\nagent main() -> Bool:\n    first = forward(true)\n    second = forward(false)\n    return true\n",
        true,
    );
}

#[test]
fn native_result_string_try_propagates_into_different_ok_type() {
    assert_parity_bool_without_tools(
        "agent fetch(flag: Bool) -> Result<String, String>:\n    if flag:\n        return Ok(\"hi\")\n    return Err(\"no\")\n\nagent widen(flag: Bool) -> Result<Bool, String>:\n    value = fetch(flag)?\n    return Ok(true)\n\nagent main() -> Bool:\n    first = widen(true)\n    second = widen(false)\n    return true\n",
        true,
    );
}

#[test]
fn native_result_retry_retries_until_success() {
    assert_parity_bool_with_mock_llm_queue(
        "prompt probe() -> String:\n    \"Probe\"\n\nagent fetch() -> Result<String, String>:\n    value = probe()\n    if value == \"ok\":\n        return Ok(value)\n    return Err(value)\n\nagent main() -> Bool:\n    outcome = try fetch() on error retry 3 times backoff linear 0\n    return probe() == \"marker\"\n",
        "probe",
        vec![
            serde_json::json!("bad"),
            serde_json::json!("bad"),
            serde_json::json!("ok"),
            serde_json::json!("marker"),
        ],
        true,
        "mock-1",
    );
}

#[test]
fn native_result_retry_returns_last_error_value_without_propagating() {
    assert_parity_bool_with_mock_llm_queue(
        "prompt probe() -> String:\n    \"Probe\"\n\nagent fetch() -> Result<String, String>:\n    value = probe()\n    if value == \"ok\":\n        return Ok(value)\n    return Err(value)\n\nagent main() -> Bool:\n    outcome = try fetch() on error retry 3 times backoff exponential 0\n    return probe() == \"marker\"\n",
        "probe",
        vec![
            serde_json::json!("bad"),
            serde_json::json!("bad"),
            serde_json::json!("bad"),
            serde_json::json!("marker"),
        ],
        true,
        "mock-1",
    );
}

#[test]
fn native_result_option_int_round_trips_through_native_agents() {
    assert_parity_bool_without_tools(
        "agent fetch(flag: Bool) -> Result<Option<Int>, String>:\n    if flag:\n        return Ok(Some(7))\n    return Err(\"no\")\n\nagent main() -> Bool:\n    first = fetch(true)\n    second = fetch(false)\n    return true\n",
        true,
    );
}

#[test]
fn native_result_option_int_try_propagates_ok_and_err() {
    assert_parity_bool_without_tools(
        "agent fetch(flag: Bool) -> Result<Option<Int>, String>:\n    if flag:\n        return Ok(Some(7))\n    return Err(\"no\")\n\nagent forward(flag: Bool) -> Result<Option<Int>, String>:\n    value = fetch(flag)?\n    return Ok(value)\n\nagent main() -> Bool:\n    first = forward(true)\n    second = forward(false)\n    return true\n",
        true,
    );
}

#[test]
fn native_result_option_int_retry_retries_until_success() {
    assert_parity_bool_with_mock_llm_queue(
        "prompt probe() -> String:\n    \"Probe\"\n\nagent fetch() -> Result<Option<Int>, String>:\n    value = probe()\n    if value == \"ok\":\n        return Ok(Some(7))\n    return Err(value)\n\nagent main() -> Bool:\n    outcome = try fetch() on error retry 3 times backoff linear 0\n    return probe() == \"marker\"\n",
        "probe",
        vec![
            serde_json::json!("bad"),
            serde_json::json!("bad"),
            serde_json::json!("ok"),
            serde_json::json!("marker"),
        ],
        true,
        "mock-1",
    );
}

#[test]
fn native_result_struct_round_trips_through_native_agents() {
    assert_parity_bool_without_tools(
        "type Boxed:\n    value: Int\n\nagent fetch(flag: Bool) -> Result<Boxed, String>:\n    if flag:\n        return Ok(Boxed(7))\n    return Err(\"no\")\n\nagent main() -> Bool:\n    first = fetch(true)\n    second = fetch(false)\n    return true\n",
        true,
    );
}

#[test]
fn native_result_struct_try_propagates_ok_and_err() {
    assert_parity_bool_without_tools(
        "type Boxed:\n    value: Int\n\nagent fetch(flag: Bool) -> Result<Boxed, String>:\n    if flag:\n        return Ok(Boxed(7))\n    return Err(\"no\")\n\nagent forward(flag: Bool) -> Result<Boxed, String>:\n    value = fetch(flag)?\n    return Ok(value)\n\nagent main() -> Bool:\n    first = forward(true)\n    second = forward(false)\n    return true\n",
        true,
    );
}

#[test]
fn native_result_list_int_round_trips_through_native_agents() {
    assert_parity_bool_without_tools(
        "agent fetch(flag: Bool) -> Result<List<Int>, String>:\n    if flag:\n        return Ok([1, 2, 3])\n    return Err(\"no\")\n\nagent main() -> Bool:\n    first = fetch(true)\n    second = fetch(false)\n    return true\n",
        true,
    );
}

#[test]
fn native_result_list_int_try_propagates_ok_and_err() {
    assert_parity_bool_without_tools(
        "agent fetch(flag: Bool) -> Result<List<Int>, String>:\n    if flag:\n        return Ok([1, 2, 3])\n    return Err(\"no\")\n\nagent forward(flag: Bool) -> Result<List<Int>, String>:\n    value = fetch(flag)?\n    return Ok(value)\n\nagent main() -> Bool:\n    first = forward(true)\n    second = forward(false)\n    return true\n",
        true,
    );
}

#[test]
fn native_result_nested_ok_round_trips_through_native_agents() {
    assert_parity_bool_without_tools(
        "agent fetch(flag: Bool) -> Result<Result<Int, String>, String>:\n    if flag:\n        return Ok(Ok(7))\n    return Err(\"no\")\n\nagent main() -> Bool:\n    first = fetch(true)\n    second = fetch(false)\n    return true\n",
        true,
    );
}

#[test]
fn native_result_nested_ok_try_propagates_ok_and_err() {
    assert_parity_bool_without_tools(
        "agent fetch(flag: Bool) -> Result<Result<Int, String>, String>:\n    if flag:\n        return Ok(Ok(7))\n    return Err(\"no\")\n\nagent forward(flag: Bool) -> Result<Result<Int, String>, String>:\n    value = fetch(flag)?\n    return Ok(value)\n\nagent main() -> Bool:\n    first = forward(true)\n    second = forward(false)\n    return true\n",
        true,
    );
}

#[test]
fn native_result_nested_error_try_propagates_into_different_ok_type() {
    assert_parity_bool_without_tools(
        "agent inner_error() -> Result<String, Bool>:\n    return Err(false)\n\nagent fetch(flag: Bool) -> Result<Int, Result<String, Bool>>:\n    if flag:\n        return Ok(7)\n    return Err(inner_error())\n\nagent widen(flag: Bool) -> Result<Bool, Result<String, Bool>>:\n    value = fetch(flag)?\n    return Ok(value == 7)\n\nagent main() -> Bool:\n    first = widen(true)\n    second = widen(false)\n    return true\n",
        true,
    );
}

#[test]
fn native_result_retry_then_try_propagates_into_different_ok_type() {
    assert_parity_bool_with_mock_llm_queue(
        "prompt probe() -> String:\n    \"Probe\"\n\nagent fetch() -> Result<String, String>:\n    value = probe()\n    if value == \"ok\":\n        return Ok(value)\n    return Err(value)\n\nagent widen() -> Result<Bool, String>:\n    attempt = try fetch() on error retry 3 times backoff linear 0\n    value = attempt?\n    return Ok(value == \"ok\")\n\nagent main() -> Bool:\n    first = widen()\n    return true\n",
        "probe",
        vec![
            serde_json::json!("bad"),
            serde_json::json!("bad"),
            serde_json::json!("ok"),
        ],
        true,
        "mock-1",
    );
}




