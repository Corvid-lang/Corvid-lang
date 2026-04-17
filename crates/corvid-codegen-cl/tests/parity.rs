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

#[test]
fn literal_return() {
    assert_parity("agent answer() -> Int:\n    return 42\n", 42);
}

#[test]
fn literal_negative() {
    assert_parity("agent answer() -> Int:\n    return 0 - 7\n", -7);
}

#[test]
fn add_two_literals() {
    assert_parity("agent calc() -> Int:\n    return 2 + 3\n", 5);
}

#[test]
fn subtract_two_literals() {
    assert_parity("agent calc() -> Int:\n    return 10 - 4\n", 6);
}

#[test]
fn multiply_two_literals() {
    assert_parity("agent calc() -> Int:\n    return 6 * 7\n", 42);
}

#[test]
fn divide_two_literals() {
    assert_parity("agent calc() -> Int:\n    return 20 / 4\n", 5);
}

#[test]
fn modulo_two_literals() {
    assert_parity("agent calc() -> Int:\n    return 23 % 5\n", 3);
}

#[test]
fn precedence_add_mul() {
    assert_parity("agent calc() -> Int:\n    return 1 + 2 * 3\n", 7);
}

#[test]
fn precedence_mul_add() {
    assert_parity("agent calc() -> Int:\n    return 2 * 3 + 1\n", 7);
}

#[test]
fn nested_arithmetic_long() {
    assert_parity(
        "agent calc() -> Int:\n    return 100 - 3 * 7 + 2\n",
        100 - 3 * 7 + 2,
    );
}

#[test]
fn recursive_agent_to_agent_call() {
    // `main` calls `helper` which returns 41, adds 1.
    assert_parity(
        "\
agent helper() -> Int:
    return 41

agent main() -> Int:
    return helper() + 1
",
        42,
    );
}

#[test]
fn chained_agent_calls() {
    assert_parity(
        "\
agent a() -> Int:
    return 2

agent b() -> Int:
    return a() * 3

agent main() -> Int:
    return b() + 1
",
        7,
    );
}

#[test]
fn overflow_on_add_is_parity_error() {
    assert_parity_overflow(
        "agent oops() -> Int:\n    return 9223372036854775807 + 1\n",
    );
}

#[test]
fn division_by_zero_is_parity_error() {
    assert_parity_overflow(
        "agent oops() -> Int:\n    return 10 / (3 - 3)\n",
    );
}

#[test]
fn modulo_by_zero_is_parity_error() {
    assert_parity_overflow(
        "agent oops() -> Int:\n    return 10 % (3 - 3)\n",
    );
}

// ============================================================
// Bool, comparisons, if/else, unary ops fixtures,
// short-circuit and/or.
// ============================================================

#[test]
fn bool_literal_true() {
    assert_parity_bool("agent t() -> Bool:\n    return true\n", true);
}

#[test]
fn bool_literal_false() {
    assert_parity_bool("agent f() -> Bool:\n    return false\n", false);
}

#[test]
fn int_equality() {
    assert_parity_bool("agent e() -> Bool:\n    return 3 == 3\n", true);
    assert_parity_bool("agent e() -> Bool:\n    return 3 == 4\n", false);
}

#[test]
fn int_inequality() {
    assert_parity_bool("agent n() -> Bool:\n    return 3 != 4\n", true);
    assert_parity_bool("agent n() -> Bool:\n    return 3 != 3\n", false);
}

#[test]
fn int_ordering() {
    assert_parity_bool("agent lt() -> Bool:\n    return 1 < 2\n", true);
    assert_parity_bool("agent lte() -> Bool:\n    return 2 <= 2\n", true);
    assert_parity_bool("agent gt() -> Bool:\n    return 2 > 1\n", true);
    assert_parity_bool("agent gte() -> Bool:\n    return 2 >= 2\n", true);
}

#[test]
fn not_flips_bool() {
    assert_parity_bool("agent n() -> Bool:\n    return not true\n", false);
    assert_parity_bool("agent n() -> Bool:\n    return not false\n", true);
}

#[test]
fn unary_negation_on_int() {
    // Prefix `-` hits the UnaryOp::Neg path, which goes through the
    // overflow-trapping `ssub_overflow(0, x)` lowering.
    assert_parity("agent n() -> Int:\n    return -5\n", -5);
    assert_parity("agent n() -> Int:\n    return -(2 + 3)\n", -5);
}

#[test]
fn unary_negation_of_int_min_is_parity_error() {
    // `-(i64::MIN)` overflows. Build MIN via `0 - i64::MAX - 1` since
    // the literal `9223372036854775808` overflows the parser.
    assert_parity_overflow(
        "agent oops() -> Int:\n    return -(0 - 9223372036854775807 - 1)\n",
    );
}

#[test]
fn if_then_else_picks_then_branch() {
    assert_parity(
        "\
agent pick() -> Int:
    if 1 < 2:
        return 10
    else:
        return 20
",
        10,
    );
}

#[test]
fn if_then_else_picks_else_branch() {
    assert_parity(
        "\
agent pick() -> Int:
    if 2 < 1:
        return 10
    else:
        return 20
",
        20,
    );
}

#[test]
fn if_without_else_falls_through_on_false() {
    assert_parity(
        "\
agent run() -> Int:
    if 2 < 1:
        return 10
    return 99
",
        99,
    );
}

#[test]
fn if_without_else_takes_then_on_true() {
    assert_parity(
        "\
agent run() -> Int:
    if 1 < 2:
        return 10
    return 99
",
        10,
    );
}

#[test]
fn nested_if_else() {
    assert_parity(
        "\
agent run() -> Int:
    if 1 < 2:
        if 3 < 4:
            return 1
        else:
            return 2
    else:
        return 3
",
        1,
    );
}

#[test]
fn short_circuit_and_evaluates_rhs_only_when_lhs_true() {
    assert_parity_bool(
        "agent sc() -> Bool:\n    return true and (1 == 1)\n",
        true,
    );
    assert_parity_bool(
        "agent sc() -> Bool:\n    return false and (1 == 1)\n",
        false,
    );
}

#[test]
fn short_circuit_or_evaluates_rhs_only_when_lhs_false() {
    assert_parity_bool(
        "agent sc() -> Bool:\n    return true or (1 == 1)\n",
        true,
    );
    assert_parity_bool(
        "agent sc() -> Bool:\n    return false or (1 == 1)\n",
        true,
    );
}

/// Concrete proof: `true or X` must short-circuit past a division by
/// zero on the right operand. Without short-circuit both tiers would
/// raise/trap; with short-circuit both return `true`.
#[test]
fn short_circuit_or_skips_div_by_zero_on_rhs() {
    assert_parity_bool(
        "agent sc() -> Bool:\n    return true or (1 / (3 - 3) == 0)\n",
        true,
    );
}

/// Same for `and`: `false and X` must skip evaluating X.
#[test]
fn short_circuit_and_skips_div_by_zero_on_rhs() {
    assert_parity_bool(
        "agent sc() -> Bool:\n    return false and (1 / (3 - 3) == 0)\n",
        false,
    );
}

#[test]
fn bool_returning_agent_is_even() {
    assert_parity_bool(
        "agent is_even() -> Bool:\n    return 4 % 2 == 0\n",
        true,
    );
}

// ============================================================
// Bare local bindings (`x = expr`) and `pass` fixtures.
// ============================================================

#[test]
fn local_binding_returns_value() {
    assert_parity(
        "agent run() -> Int:\n    x = 42\n    return x\n",
        42,
    );
}

#[test]
fn local_binding_with_arithmetic() {
    assert_parity(
        "\
agent run() -> Int:
    x = 2
    y = 3
    return x + y * 4
",
        14,
    );
}

#[test]
fn local_binding_used_twice() {
    assert_parity(
        "\
agent run() -> Int:
    x = 7
    return x + x
",
        14,
    );
}

#[test]
fn reassignment_takes_latest_value() {
    assert_parity(
        "\
agent run() -> Int:
    x = 5
    x = x * 2
    x = x + 1
    return x
",
        11,
    );
}

#[test]
fn local_binding_with_bool() {
    assert_parity_bool(
        "\
agent run() -> Bool:
    flag = true
    return flag
",
        true,
    );
}

#[test]
fn reassignment_inside_if_branch() {
    assert_parity(
        "\
agent run() -> Int:
    x = 5
    if x == 5:
        x = 100
    return x
",
        100,
    );
}

#[test]
fn local_binding_used_in_comparison() {
    assert_parity_bool(
        "\
agent run() -> Bool:
    n = 4
    return n % 2 == 0
",
        true,
    );
}

#[test]
fn pass_in_if_is_a_noop() {
    assert_parity(
        "\
agent run() -> Int:
    x = 5
    if x > 0:
        pass
    return x
",
        5,
    );
}

// ============================================================
// Float arithmetic + comparisons + IEEE 754 fixtures.
// ============================================================
//
// Float entry-agent returns are blocked until serialization support (the C shim's
// `printf("%lld")` doesn't print Floats), so every Float fixture
// computes with Float internally but returns `Bool` or `Int`.

#[test]
fn float_addition_eq_check() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return 1.5 + 2.5 == 4.0\n",
        true,
    );
}

#[test]
fn float_subtraction_and_multiplication() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return (5.0 - 1.5) * 2.0 == 7.0\n",
        true,
    );
}

#[test]
fn float_division_exact() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return 9.0 / 3.0 == 3.0\n",
        true,
    );
}

#[test]
fn mixed_int_float_promotes_to_float() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return 3 + 0.5 == 3.5\n",
        true,
    );
    assert_parity_bool(
        "agent f() -> Bool:\n    return 0.5 + 3 == 3.5\n",
        true,
    );
}

#[test]
fn float_ordering() {
    assert_parity_bool("agent f() -> Bool:\n    return 1.5 < 2.0\n", true);
    assert_parity_bool("agent f() -> Bool:\n    return 2.0 <= 2.0\n", true);
    assert_parity_bool("agent f() -> Bool:\n    return 3.14 > 3.0\n", true);
    assert_parity_bool("agent f() -> Bool:\n    return 3.0 >= 3.0\n", true);
}

#[test]
fn float_unary_negation() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return -2.5 == 0.0 - 2.5\n",
        true,
    );
}

/// IEEE 754: `1.0 / 0.0` is `+Inf`, not a trap. `Inf > 1.0` is true.
/// This is the headline divergence from Int's overflow-trap policy.
#[test]
fn float_div_by_zero_is_infinity_not_trap() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return 1.0 / 0.0 > 1.0\n",
        true,
    );
}

/// IEEE 754: `0.0 / 0.0` is `NaN`. `NaN != NaN` is true; `NaN == NaN`
/// is false. Both tiers must agree.
#[test]
fn nan_inequality_is_true() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return (0.0 / 0.0) != (0.0 / 0.0)\n",
        true,
    );
    assert_parity_bool(
        "agent f() -> Bool:\n    return (0.0 / 0.0) == (0.0 / 0.0)\n",
        false,
    );
}

#[test]
fn float_in_local_binding() {
    assert_parity_bool(
        "\
agent f() -> Bool:
    pi = 3.14
    tau = pi * 2.0
    return tau > 6.0
",
        true,
    );
}

/// Float entry-agent returns are not yet supported — driver must
/// surface a clean `NotSupported` pointing at the missing serialization support.
#[test]
fn struct_entry_return_is_blocked_with_clear_error() {
    // The newer entry path lifted the Float-return restriction. Struct/List
    // returns are still blocked — verify the driver guard fires.
    use corvid_codegen_cl::{build_native_to_disk, CodegenErrorKind};
    let ir = ir_of(
        "type Wrap:\n    v: Int\n\nagent f() -> Wrap:\n    return Wrap(42)\n",
    );
    let tmp = tempfile::tempdir().unwrap();
    let bin_path = tmp.path().join("prog");
    let err = build_native_to_disk(&ir, "corvid_parity_test", &bin_path, &[test_tools_lib_path().as_path()]).unwrap_err();
    match err.kind {
        CodegenErrorKind::NotSupported(ref msg) => {
            assert!(
                msg.contains("struct") || msg.contains("Struct") || msg.contains("Wrap"),
                "expected message to mention struct: {msg}"
            );
            assert!(
                msg.contains("serialization"),
                "expected message to point at missing serialization support: {msg}"
            );
        }
        other => panic!("expected NotSupported, got {other:?}"),
    }
}

// ============================================================
// String literals, concat, comparisons fixtures.
// Every fixture is also subject to the leak detector — if any
// allocation outlives release, the test fails with the imbalance.
// ============================================================

#[test]
fn string_literal_equality_is_true() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return \"hello\" == \"hello\"\n",
        true,
    );
}

#[test]
fn string_literal_inequality_is_false() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return \"hello\" == \"world\"\n",
        false,
    );
}

#[test]
fn string_concat_then_compare() {
    // "hi " + "there" should equal "hi there" — exercises concat plus
    // equality, plus the release of the heap-allocated concat result.
    assert_parity_bool(
        "agent f() -> Bool:\n    return \"hi \" + \"there\" == \"hi there\"\n",
        true,
    );
}

#[test]
fn empty_string_concat_is_identity() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return \"\" + \"x\" == \"x\"\n",
        true,
    );
    assert_parity_bool(
        "agent f() -> Bool:\n    return \"x\" + \"\" == \"x\"\n",
        true,
    );
}

#[test]
fn string_not_equal_operator() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return \"hello\" != \"world\"\n",
        true,
    );
    assert_parity_bool(
        "agent f() -> Bool:\n    return \"hello\" != \"hello\"\n",
        false,
    );
}

#[test]
fn string_ordering_lexicographic() {
    assert_parity_bool(
        "agent f() -> Bool:\n    return \"abc\" < \"abd\"\n",
        true,
    );
    assert_parity_bool(
        "agent f() -> Bool:\n    return \"abc\" <= \"abc\"\n",
        true,
    );
    assert_parity_bool(
        "agent f() -> Bool:\n    return \"abd\" > \"abc\"\n",
        true,
    );
    assert_parity_bool(
        "agent f() -> Bool:\n    return \"abc\" >= \"abc\"\n",
        true,
    );
}

#[test]
fn string_in_local_binding_then_concat_then_compare() {
    // Exercises bind-time retain on Let + reassignment-time release-old +
    // scope-exit release. Leak detector verifies count balance.
    assert_parity_bool(
        "\
agent f() -> Bool:
    s = \"foo\"
    s = s + \"bar\"
    return s == \"foobar\"
",
        true,
    );
}

// ============================================================
// Struct construction, field access fixtures,
// destructor-driven release of refcounted fields.
// Leak detector runs on each — zero leaks required.
// ============================================================

#[test]
fn scalar_only_struct_construct_and_access() {
    assert_parity(
        "\
type Point:
    x: Int
    y: Int

agent main() -> Int:
    p = Point(3, 4)
    return p.x + p.y
",
        7,
    );
}

#[test]
fn struct_with_bool_field() {
    // `on` is now a hard keyword (used by
    // `try ... on error retry ...` syntax). The field was renamed
    // `enabled` to unbreak this test. TODO (Dev B or 18-polish
    // this feature): consider making `on` a context-sensitive / soft
    // keyword so user identifiers with that name keep working.
    assert_parity_bool(
        "\
type Flag:
    enabled: Bool
    code: Int

agent main() -> Bool:
    f = Flag(true, 42)
    return f.enabled
",
        true,
    );
}

#[test]
fn struct_with_string_field_destructor_releases_field() {
    // The Order's destructor should release the id (String) field.
    // Since "ord_1" is an immortal literal, the destructor call is a
    // no-op at runtime — but the generated destructor MUST be emitted
    // and stored in the header, and `corvid_release` MUST call it.
    // The leak detector verifies ALLOCS == RELEASES: 1 alloc (the
    // Order), 1 release (the Order block when refcount hits 0).
    assert_parity_bool(
        "\
type Order:
    id: String
    amount: Float

agent main() -> Bool:
    o = Order(\"ord_1\", 49.99)
    return o.amount > 10.0
",
        true,
    );
}

#[test]
fn struct_with_string_field_extract_and_compare() {
    // Extract the refcounted String field and compare to a literal.
    // Exercises: struct alloc (ALLOCS=1), destructor emitted, field
    // access + retain on String load, String equality, release of
    // both the extracted String and the struct.
    assert_parity_bool(
        "\
type Named:
    label: String

agent main() -> Bool:
    n = Named(\"hello\")
    return n.label == \"hello\"
",
        true,
    );
}

#[test]
fn struct_passed_to_another_agent() {
    assert_parity(
        "\
type Amount:
    cents: Int

agent total(a: Amount, b: Amount) -> Int:
    return a.cents + b.cents

agent main() -> Int:
    x = Amount(100)
    y = Amount(250)
    return total(x, y)
",
        350,
    );
}

#[test]
fn struct_reassignment_releases_old_instance() {
    // Binding `o` to one Order, then reassigning to another — the
    // release-on-rebind logic releases the first instance before the
    // second takes over. Leak detector: 2 allocs, 2 releases.
    assert_parity(
        "\
type Box:
    v: Int

agent main() -> Int:
    b = Box(1)
    b = Box(100)
    return b.v
",
        100,
    );
}

#[test]
fn nested_struct_field_access() {
    assert_parity(
        "\
type Inner:
    value: Int

type Outer:
    inner: Inner
    tag: Int

agent main() -> Int:
    i = Inner(7)
    o = Outer(i, 10)
    return o.inner.value + o.tag
",
        17,
    );
}

// ============================================================
// List<T>, `for`, `break`, `continue` fixtures.
// Leak detector confirms zero leaks on every fixture — including
// List<String> (shared destructor walks + releases elements).
// ============================================================

#[test]
fn list_literal_sum_via_for() {
    assert_parity(
        "\
agent main() -> Int:
    total = 0
    for x in [1, 2, 3, 4, 5]:
        total = total + x
    return total
",
        15,
    );
}

#[test]
fn for_with_break_exits_early() {
    assert_parity(
        "\
agent main() -> Int:
    total = 0
    for x in [1, 2, 3, 4, 5]:
        if x == 3:
            break
        total = total + x
    return total
",
        3,
    );
}

#[test]
fn for_with_continue_skips_element() {
    assert_parity(
        "\
agent main() -> Int:
    total = 0
    for x in [1, 2, 3, 4, 5]:
        if x == 3:
            continue
        total = total + x
    return total
",
        12,
    );
}

#[test]
fn list_subscript_access() {
    assert_parity(
        "\
agent main() -> Int:
    xs = [10, 20, 30]
    return xs[1]
",
        20,
    );
}

#[test]
fn list_of_strings_destructor_releases_elements() {
    // Exercises the shared list destructor. Each String element is an
    // immortal literal (no-op on release) but the destructor machinery
    // must walk + call release correctly. Leak detector: 1 alloc (the
    // list block), 1 release.
    assert_parity_bool(
        "\
agent main() -> Bool:
    xs = [\"a\", \"b\", \"c\"]
    return xs[1] == \"b\"
",
        true,
    );
}

#[test]
fn list_of_heap_strings_exercises_real_releases() {
    // Heap-allocated String elements (from concat) — their +1
    // refcounts transfer into the list on store. When the list is
    // freed, the destructor releases each element, decrementing real
    // heap refcounts to 0 and freeing those blocks too.
    // Leak detector: 4 allocs (list + 3 concats), 4 releases.
    assert_parity_bool(
        "\
agent main() -> Bool:
    xs = [\"hi \" + \"a\", \"hi \" + \"b\", \"hi \" + \"c\"]
    return xs[2] == \"hi c\"
",
        true,
    );
}

#[test]
fn nested_list_subscript_two_deep() {
    // List<List<Int>> — outer list's destructor releases each inner
    // list, which then frees itself (no inner destructor needed
    // because Int is not refcounted).
    assert_parity(
        "\
agent main() -> Int:
    rows = [[1, 2], [3, 4], [5, 6]]
    return rows[1][0]
",
        3,
    );
}

#[test]
fn empty_list_for_loop_runs_zero_iterations() {
    assert_parity(
        "\
agent main() -> Int:
    total = 0
    for x in [0, 0, 0, 0]:
        total = total + 1
    return total
",
        4,
    );
    // Actual empty list (length 0) — requires a list literal to have
    // at least one item in the current parser? Let me verify via:
    assert_parity(
        "\
agent main() -> Int:
    total = 99
    for x in [1]:
        total = total - 1
    return total
",
        98,
    );
}

// ============================================================
// Parameterised entry agents + non-Int
// returns at the command-line boundary. The compiled main
// decodes argv via parse_i64/_f64/_bool/string_from_cstr and
// prints results via the type-dispatched print helpers.
// ============================================================

/// Run a compiled binary with CLI args + leak detector. Returns
/// (stdout, stderr, status). Entry-agent params are passed via argv.
fn run_compiled_with_args(
    bin: &std::path::Path,
    args: &[&str],
) -> (String, String, std::process::ExitStatus) {
    let output = Command::new(bin)
        .args(args)
        .env("CORVID_DEBUG_ALLOC", "1")
        .output()
        .expect("run compiled binary");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status,
    )
}

/// Build, run with argv, and return (stdout-first-line, stderr). Shared
/// by every entry-fixture helper so the compile + exec plumbing
/// stays in one place.
#[track_caller]
fn compile_and_run(src: &str, argv: &[&str]) -> (String, String) {
    let ir = ir_of(src);
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_path = tmp.path().join("prog");
    let produced =
        build_native_to_disk(&ir, "corvid_parity_test", &bin_path, &[test_tools_lib_path().as_path()]).expect("compile + link");
    let (stdout, stderr, status) = run_compiled_with_args(&produced, argv);
    assert!(
        status.success(),
        "compiled binary exited non-zero: status={:?} stderr={stderr} stdout={stdout} src=\n{src}",
        status.code()
    );
    assert_no_leaks(&stderr, src);
    let printed = stdout.trim().lines().next().unwrap_or("").to_string();
    (printed, stderr)
}

/// Drive the interpreter with typed `Value` args and assert it produced
/// `expected`. Used by entry fixtures whose entry agent takes params.
#[track_caller]
fn run_interp_with_args(src: &str, agent: &str, args: Vec<Value>, expected: Value) {
    let ir = ir_of(src);
    let runtime = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .build();
    let got = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async { corvid_vm::run_agent(&ir, agent, args, &runtime).await })
        .expect("interpreter run");
    assert_eq!(got, expected, "interpreter mismatch for src:\n{src}");
}

#[test]
fn int_param_doubles() {
    let src = "agent calc(n: Int) -> Int:\n    return n * 2\n";
    run_interp_with_args(src, "calc", vec![Value::Int(7)], Value::Int(14));
    let (out, _) = compile_and_run(src, &["7"]);
    assert_eq!(out, "14");
}

#[test]
fn two_int_params_sum() {
    let src = "agent sum(a: Int, b: Int) -> Int:\n    return a + b\n";
    run_interp_with_args(
        src,
        "sum",
        vec![Value::Int(10), Value::Int(32)],
        Value::Int(42),
    );
    let (out, _) = compile_and_run(src, &["10", "32"]);
    assert_eq!(out, "42");
}

#[test]
fn bool_param_inverts() {
    let src = "agent inv(b: Bool) -> Bool:\n    return not b\n";
    run_interp_with_args(src, "inv", vec![Value::Bool(true)], Value::Bool(false));
    run_interp_with_args(src, "inv", vec![Value::Bool(false)], Value::Bool(true));
    let (out, _) = compile_and_run(src, &["true"]);
    assert_eq!(out, "false");
    let (out, _) = compile_and_run(src, &["false"]);
    assert_eq!(out, "true");
}

#[test]
fn float_param_doubled_returns_float() {
    let src = "agent calc(x: Float) -> Float:\n    return x * 2.0\n";
    run_interp_with_args(src, "calc", vec![Value::Float(1.5)], Value::Float(3.0));
    let (out, _) = compile_and_run(src, &["1.5"]);
    let printed: f64 = out
        .parse()
        .unwrap_or_else(|e| panic!("parse `{out}` as f64: {e}"));
    assert_eq!(printed.to_bits(), 3.0f64.to_bits(), "got {printed}");
}

#[test]
fn float_return_nan_round_trips() {
    // 0.0/0.0 is NaN. The compiled binary prints via %.17g (typically
    // "nan"); the interpreter returns Value::Float(NaN). Both tiers
    // must produce a NaN — we compare via is_nan() not bit equality
    // (NaN bit pattern can differ between the FPU and printf round-trip).
    let src = "agent n() -> Float:\n    return 0.0 / 0.0\n";
    let ir = ir_of(src);
    let runtime = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .build();
    let got = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async { corvid_vm::run_agent(&ir, "n", vec![], &runtime).await })
        .expect("interpreter run");
    match got {
        Value::Float(f) => assert!(f.is_nan(), "interpreter should return NaN, got {f}"),
        other => panic!("expected Float, got {other:?}"),
    }
    let (out, _) = compile_and_run(src, &[]);
    assert!(
        out.to_ascii_lowercase().contains("nan"),
        "expected printed NaN, got `{out}`"
    );
}

#[test]
fn string_param_echoes() {
    let src = "agent echo(s: String) -> String:\n    return s\n";
    run_interp_with_args(
        src,
        "echo",
        vec![Value::String(Arc::from("hello"))],
        Value::String(Arc::from("hello")),
    );
    let (out, _) = compile_and_run(src, &["hello"]);
    assert_eq!(out, "hello");
}

#[test]
fn string_return_from_concat_with_param() {
    // Exercises: String param (argv → refcounted descriptor via
    // string_from_cstr), concat (heap alloc), return of a refcounted
    // value across the entry boundary, print_string + release by main.
    // Leak detector: ALLOCS == RELEASES confirms every descriptor is
    // released exactly once.
    let src = "agent greet(name: String) -> String:\n    return \"hi \" + name\n";
    run_interp_with_args(
        src,
        "greet",
        vec![Value::String(Arc::from("world"))],
        Value::String(Arc::from("hi world")),
    );
    let (out, _) = compile_and_run(src, &["world"]);
    assert_eq!(out, "hi world");
}

#[test]
fn float_return_no_params() {
    let src = "agent pi() -> Float:\n    return 3.14\n";
    run_interp_with_args(src, "pi", vec![], Value::Float(3.14));
    let (out, _) = compile_and_run(src, &[]);
    let printed: f64 = out
        .parse()
        .unwrap_or_else(|e| panic!("parse `{out}` as f64: {e}"));
    assert_eq!(printed.to_bits(), 3.14f64.to_bits());
}

#[test]
fn string_return_no_params() {
    let src = "agent hello() -> String:\n    return \"hello\"\n";
    run_interp_with_args(src, "hello", vec![], Value::String(Arc::from("hello")));
    let (out, _) = compile_and_run(src, &[]);
    assert_eq!(out, "hello");
}

#[test]
fn arity_mismatch_exits_nonzero() {
    // Entry agent takes one Int; invoke with zero argv args. Codegen
    // main should detect argc mismatch and call corvid_arity_mismatch,
    // which prints a clear error and exits non-zero.
    let src = "agent calc(n: Int) -> Int:\n    return n + 1\n";
    let ir = ir_of(src);
    let tmp = tempfile::tempdir().unwrap();
    let bin_path = tmp.path().join("prog");
    let produced =
        build_native_to_disk(&ir, "corvid_parity_test", &bin_path, &[test_tools_lib_path().as_path()]).expect("compile + link");
    let output = Command::new(&produced).output().expect("run");
    assert!(
        !output.status.success(),
        "expected non-zero exit on arity mismatch, got success. stdout={}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("argument") || stderr.contains("arity") || stderr.contains("expected"),
        "stderr should mention arity / argument count: {stderr}"
    );
}

#[test]
fn parse_error_on_bad_int_argv_exits_nonzero() {
    // "notanint" isn't a valid Int — parse_i64 should print a
    // specific error (not reuse the overflow path) and exit non-zero.
    let src = "agent calc(n: Int) -> Int:\n    return n\n";
    let ir = ir_of(src);
    let tmp = tempfile::tempdir().unwrap();
    let bin_path = tmp.path().join("prog");
    let produced =
        build_native_to_disk(&ir, "corvid_parity_test", &bin_path, &[test_tools_lib_path().as_path()]).expect("compile + link");
    let output = Command::new(&produced)
        .arg("notanint")
        .output()
        .expect("run");
    assert!(
        !output.status.success(),
        "expected non-zero exit on parse error, got success"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("overflow"),
        "parse error must not reuse the overflow message: {stderr}"
    );
}

#[test]
fn agent_with_param_uses_local_alongside_param() {
    // The `n` parameter is bound at function entry; the local `doubled`
    // demonstrates that parameter Variables and Let Variables coexist.
    // Smoke-test via interpreter only — `corvid build --target=native`
    // currently requires a parameter-less entry agent.
    let ir = ir_of("agent calc(n: Int) -> Int:\n    doubled = n * 2\n    return doubled + 1\n");
    let runtime = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .build();
    let v = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            corvid_vm::run_agent(&ir, "calc", vec![Value::Int(20)], &runtime).await
        })
        .expect("interp");
    assert_eq!(v, Value::Int(41));
}

// ============================================================
// Native-tier `tool` call fixtures dispatched through
// the async runtime bridge.
//
// Each fixture registers its zero-arg Int-returning mocks in both
// tiers:
//   - Interpreter: via `Runtime::builder().tool(...)` before run.
//   - Native: via the CORVID_TEST_MOCK_INT_TOOLS env var which the
//     codegen-emitted main's `corvid_runtime_init` call reads during
//     runtime construction (see corvid-runtime's ffi_bridge.rs).
//
// The typed bridge ships the user-facing proc-macro registry and generalises
// the bridge to arbitrary arg + return types; these fixtures exercise
// only the narrow `() -> Int` shape the early bridge supports.
// ============================================================

/// Parity harness variant that pre-registers mock zero-arg Int tools in
/// both tiers before running. `expected` is the entry agent's return
/// value (always Int in these fixtures). Leak-detector still runs.
#[track_caller]
fn assert_parity_with_mock_tools(src: &str, mocks: &[(&str, i64)], expected: i64) {
    let ir = ir_of(src);

    // --- Interpreter tier with mocks registered ---
    let mut builder = Runtime::builder().approver(Arc::new(ProgrammaticApprover::always_yes()));
    for (name, value) in mocks {
        let v = *value;
        let name_owned = name.to_string();
        builder = builder.tool(name_owned, move |_args| async move {
            Ok(serde_json::json!(v))
        });
    }
    let runtime = builder.build();
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

    // --- Compiled binary tier with mocks passed via env var ---
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_path = tmp.path().join("prog");
    let produced = build_native_to_disk(&ir, "corvid_parity_test", &bin_path, &[test_tools_lib_path().as_path()])
        .expect("compile + link");

    let (stdout, stderr, status) = run_with_leak_detector_and_mocks(&produced, mocks);
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

#[test]
fn tool_returns_int_directly() {
    // The simplest tool-dispatch shape: entry agent calls one tool,
    // returns its result. Exercises: Cranelift lowering emits the
    // call to corvid_tool_call_sync_int, runtime init+shutdown wired
    // into main, env-var mock registration round-trip.
    assert_parity_with_mock_tools(
        "tool answer() -> Int\n\nagent main() -> Int:\n    return answer()\n",
        &[("answer", 42)],
        42,
    );
}

#[test]
fn tool_result_in_arithmetic() {
    // Tool result is used in an arithmetic expression — verifies
    // the bridge result is a plain Int ClValue usable in downstream
    // codegen (not a special wrapper type).
    assert_parity_with_mock_tools(
        "tool base() -> Int\n\nagent main() -> Int:\n    return base() * 2 + 5\n",
        &[("base", 10)],
        25,
    );
}

#[test]
fn tool_result_in_conditional() {
    // Tool result drives an if branch — verifies the bridge plays
    // nicely with the existing control-flow codegen.
    assert_parity_with_mock_tools(
        "tool flag() -> Int\n\nagent main() -> Int:\n    f = flag()\n    if f > 0:\n        return 100\n    return 200\n",
        &[("flag", 7)],
        100,
    );
}

#[test]
fn tool_result_in_conditional_false_branch() {
    // Same structure as above but the tool returns a value that
    // hits the other branch. Confirms we didn't accidentally hardcode
    // a branch direction.
    assert_parity_with_mock_tools(
        "tool flag() -> Int\n\nagent main() -> Int:\n    f = flag()\n    if f > 0:\n        return 100\n    return 200\n",
        &[("flag", -1)],
        200,
    );
}

#[test]
fn two_tools_added() {
    // Two distinct tools; the env-var parser correctly registers both.
    assert_parity_with_mock_tools(
        "tool a() -> Int\ntool b() -> Int\n\nagent main() -> Int:\n    return a() + b()\n",
        &[("a", 30), ("b", 12)],
        42,
    );
}

#[test]
fn tool_called_from_helper_agent() {
    // Agent -> helper agent -> tool. Exercises that the runtime bridge
    // works through the agent-to-agent call path, not just from the
    // entry agent directly.
    assert_parity_with_mock_tools(
        "tool leaf() -> Int\n\nagent helper() -> Int:\n    return leaf() + 1\n\nagent main() -> Int:\n    return helper() * 10\n",
        &[("leaf", 4)],
        50,
    );
}

// ============================================================
// Typed-ABI dispatch fixtures across scalar arg types.
// Each fixture uses a fixed-behaviour tool from `corvid-test-tools`
// (no env var required — the tool's behaviour is baked in). The
// interpreter tier registers a matching handler via
// `RuntimeBuilder::tool`; the compiled tier picks up the typed
// extern wrapper from the linked staticlib. Both tiers must agree.
// ============================================================

/// Helper for typed-ABI fixtures whose tools have fixed (non-env)
/// behaviour. Caller supplies a closure that adds the tool handlers
/// to the interpreter Runtime; the native binary uses `corvid-test-tools`'s
/// baked-in implementations.
#[track_caller]
fn assert_parity_prebuilt_tools<F>(src: &str, expected: i64, register_handlers: F)
where
    F: FnOnce(corvid_runtime::RuntimeBuilder) -> corvid_runtime::RuntimeBuilder,
{
    let ir = ir_of(src);

    let builder =
        Runtime::builder().approver(Arc::new(ProgrammaticApprover::always_yes()));
    let runtime = register_handlers(builder).build();
    let interp_value = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async { corvid_vm::run_agent(&ir, entry_name(&ir), vec![], &runtime).await })
        .expect("interpreter run");
    assert_eq!(
        interp_value,
        Value::Int(expected),
        "interpreter mismatch for src:\n{src}"
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_path = tmp.path().join("prog");
    let produced =
        build_native_to_disk(&ir, "corvid_parity_test", &bin_path, &[test_tools_lib_path().as_path()])
            .expect("compile + link");
    // No env vars needed — the test-tools `#[tool]` impls here have
    // fixed behaviour. APPROVE_AUTO still on for safety.
    let (stdout, stderr, status) = run_with_leak_detector_and_mocks(&produced, &[]);
    assert!(
        status.success(),
        "compiled binary exited non-zero: status={:?} stdout={stdout} stderr={stderr} src=\n{src}"
    , status.code());
    let compiled: i64 = stdout
        .trim()
        .lines()
        .next()
        .unwrap_or("")
        .parse()
        .unwrap_or_else(|e| panic!("parse stdout `{stdout}` as i64: {e}"));
    assert_eq!(
        compiled, expected,
        "compiled result mismatch for src:\n{src}\nstderr: {stderr}"
    );
    assert_no_leaks(&stderr, src);
}

#[test]
fn tool_takes_int_arg() {
    // Typed-ABI call: Int argument passed directly (no JSON marshalling).
    assert_parity_prebuilt_tools(
        "tool double_int(n: Int) -> Int\n\nagent main() -> Int:\n    return double_int(21)\n",
        42,
        |b| {
            b.tool("double_int", |args| async move {
                let n = args[0].as_i64().unwrap();
                Ok(serde_json::json!(n * 2))
            })
        },
    );
}

#[test]
fn tool_takes_two_int_args() {
    // Multi-arg typed-ABI call. Verifies argument ordering + ABI
    // alignment at the boundary.
    assert_parity_prebuilt_tools(
        "tool add_two(a: Int, b: Int) -> Int\n\nagent main() -> Int:\n    return add_two(17, 25)\n",
        42,
        |b| {
            b.tool("add_two", |args| async move {
                let a = args[0].as_i64().unwrap();
                let b = args[1].as_i64().unwrap();
                Ok(serde_json::json!(a + b))
            })
        },
    );
}

#[test]
fn tool_takes_string_arg_returns_int() {
    // CorvidString -> Rust `String` conversion on the arg; i64
    // return. Exercises the refcount-aware String ABI wrapper.
    assert_parity_prebuilt_tools(
        "tool string_len(s: String) -> Int\n\nagent main() -> Int:\n    return string_len(\"hello world\")\n",
        11,
        |b| {
            b.tool("string_len", |args| async move {
                let s = args[0].as_str().unwrap();
                Ok(serde_json::json!(s.chars().count() as i64))
            })
        },
    );
}

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

/// Run a prompt-using parity fixture with a fixed mock LLM response.
/// Verifies both tiers produce the same result given the same mock.
/// `mock_value` is the JSON value the interpreter mock returns
/// (typically `json!(42)`, `json!(true)`, etc.); the native tier
/// receives the same value as TEXT (stringified per `Value::to_string`)
/// because the env-var mock channel is text-only and the bridge's
/// retry-with-validation parses it back. Both tiers must produce
/// `expected`.
#[track_caller]
fn assert_parity_with_mock_llm(
    src: &str,
    mock_value: serde_json::Value,
    expected: i64,
    model: &str,
    prompt_name: &str,
) {
    let ir = ir_of(src);

    // --- Interpreter tier ---
    let mock = corvid_runtime::MockAdapter::new(model)
        .reply(prompt_name, mock_value.clone());
    let runtime = Runtime::builder()
        .approver(Arc::new(ProgrammaticApprover::always_yes()))
        .llm(Arc::new(mock))
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
        Value::Int(expected),
        "interpreter result mismatch for src:\n{src}"
    );

    // --- Native tier with env-var mock ---
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_path = tmp.path().join("prog");
    let produced = build_native_to_disk(
        &ir,
        "corvid_parity_test",
        &bin_path,
        &[test_tools_lib_path().as_path()],
    )
    .expect("compile + link");

    // Stringify the mock JSON value for the env-var channel. For
    // `json!(42)` this is `"42"`; for `json!("hi")` it's `"hi"` (we
    // strip surrounding JSON-string quotes since the bridge will
    // strip them back during parse anyway). Using as_str when
    // available avoids the JSON-quoted form for String returns.
    let mock_text = match &mock_value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let output = Command::new(&produced)
        .env("CORVID_DEBUG_ALLOC", "1")
        .env("CORVID_APPROVE_AUTO", "1")
        .env("CORVID_TEST_MOCK_LLM", "1")
        .env("CORVID_TEST_MOCK_LLM_RESPONSE", mock_text)
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
    let compiled: i64 = stdout
        .trim()
        .lines()
        .next()
        .unwrap_or("")
        .parse()
        .unwrap_or_else(|e| panic!("parse stdout `{stdout}` as i64: {e}"));
    assert_eq!(
        compiled, expected,
        "compiled result mismatch for src:\n{src}\nstderr: {stderr}"
    );
    assert_no_leaks(&stderr, src);
}

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
fn prompt_returns_int() {
    // Simplest prompt path: zero-arg, Int return. Mock LLM returns
    // 42 (JSON Int for interpreter; "42" text for compiled bridge).
    assert_parity_with_mock_llm(
        "prompt answer() -> Int:\n    \"What is the answer\"\n\nagent main() -> Int:\n    return answer()\n",
        serde_json::json!(42),
        42,
        "mock-1",
        "answer",
    );
}

#[test]
fn prompt_with_int_arg_interpolation() {
    // Template interpolation of a non-String arg. Codegen calls
    // `corvid_string_from_int` to stringify the Int before
    // concatenating into the rendered prompt. Mock returns a known
    // value regardless — the test verifies the dispatch path works.
    assert_parity_with_mock_llm(
        "prompt double(n: Int) -> Int:\n    \"Double {n}\"\n\nagent main() -> Int:\n    return double(7)\n",
        serde_json::json!(14),
        14,
        "mock-1",
        "double",
    );
}

#[test]
fn prompt_with_string_arg_interpolation() {
    // String arg passes through stringify-as-identity in codegen.
    // Tests the String concat path through the prompt bridge.
    assert_parity_with_mock_llm(
        "prompt classify(message: String) -> Int:\n    \"Classify: {message}\"\n\nagent main() -> Int:\n    return classify(\"hello\")\n",
        serde_json::json!(1),
        1,
        "mock-1",
        "classify",
    );
}

#[test]
fn prompt_with_local_string_arg_interpolation() {
    // Bare-Local String args at prompt boundaries are
    // pinned as borrowed rather than released like owned temps. This
    // fixture exercises the local path directly and proves the prompt
    // boundary doesn't retire the binding's structural +1.
    assert_parity_with_mock_llm(
        "prompt classify(message: String) -> Int:\n    \"Classify: {message}\"\n\nagent main() -> Int:\n    msg = \"hello\"\n    return classify(msg)\n",
        serde_json::json!(1),
        1,
        "mock-1",
        "classify",
    );
}

#[test]
fn approve_before_dangerous_tool_compiles_and_runs() {
    // `approve` is a compile-time-checked no-op in
    // generated code. The effect checker verifies that
    // every dangerous-tool call is preceded by a matching approve
    // at compile time; later safety work adds runtime verification
    // as additional defense-in-depth. Here we exercise the codegen
    // path end-to-end: the approve statement lowers, the dangerous
    // tool call dispatches through the typed ABI, and the result
    // flows back.
    assert_parity_prebuilt_tools(
        "tool double_int(n: Int) -> Int dangerous\n\nagent main() -> Int:\n    approve DoubleInt(5)\n    return double_int(5)\n",
        10,
        |b| {
            b.tool("double_int", |args| async move {
                let n = args[0].as_i64().unwrap();
                Ok(serde_json::json!(n * 2))
            })
        },
    );
}

#[test]
fn tool_roundtrips_string() {
    // String in, String out — exercises both conversion directions
    // through the typed ABI. Compares via `string_len` on the
    // result to fit the Int-return parity helper contract.
    assert_parity_prebuilt_tools(
        "tool greet_string(name: String) -> String\ntool string_len(s: String) -> Int\n\nagent main() -> Int:\n    g = greet_string(\"world\")\n    return string_len(g)\n",
        // "hi world" = 8 chars
        8,
        |b| {
            b.tool("greet_string", |args| async move {
                let name = args[0].as_str().unwrap();
                Ok(serde_json::json!(format!("hi {name}")))
            })
            .tool("string_len", |args| async move {
                let s = args[0].as_str().unwrap();
                Ok(serde_json::json!(s.chars().count() as i64))
            })
        },
    );
}

// ============================================================
// Methods on user types via `extend T:` block fixtures.
//
// Methods compile to ordinary agent calls with the receiver
// prepended as the first argument (typechecker + IR rewrites
// `x.foo(args)` to `foo(x, args...)`). No codegen changes were
// needed for methods — these fixtures verify the rewrite is
// transparent to the existing native-tier pipeline.
// ============================================================

#[test]
fn method_returns_field() {
    // Simplest method: receiver-only, returns a field. Verifies the
    // dot-syntax → typed-call rewrite + struct-field access chain.
    assert_parity(
        "type Amount:\n    cents: Int\n\nextend Amount:\n    public agent value(a: Amount) -> Int:\n        return a.cents\n\nagent main() -> Int:\n    a = Amount(42)\n    return a.value()\n",
        42,
    );
}

#[test]
fn method_with_arithmetic_on_field() {
    // Method body does arithmetic on the receiver's field.
    assert_parity(
        "type Order:\n    amount: Int\n    tax: Int\n\nextend Order:\n    public agent total(o: Order) -> Int:\n        return o.amount + o.tax\n\nagent main() -> Int:\n    o = Order(100, 7)\n    return o.total()\n",
        107,
    );
}

#[test]
fn method_with_extra_arg_after_receiver() {
    // Method takes the receiver + an explicit second argument.
    // Call site: `o.scale(3)` lowers to `scale(o, 3)`.
    assert_parity(
        "type Amount:\n    cents: Int\n\nextend Amount:\n    public agent scale(a: Amount, factor: Int) -> Int:\n        return a.cents * factor\n\nagent main() -> Int:\n    a = Amount(7)\n    return a.scale(6)\n",
        42,
    );
}

#[test]
fn method_calls_another_method() {
    // One method (`total`) calls another method (`with_tip`) on the
    // same receiver. Verifies the rewrite works inside method bodies,
    // not just at top-level call sites.
    assert_parity(
        "type Bill:\n    base: Int\n\nextend Bill:\n    public agent with_tip(b: Bill, pct: Int) -> Int:\n        return b.base + (b.base * pct) / 100\n    public agent total(b: Bill) -> Int:\n        return b.with_tip(20)\n\nagent main() -> Int:\n    b = Bill(100)\n    return b.total()\n",
        120,
    );
}

#[test]
fn methods_with_same_name_on_different_types() {
    // Two different types each have a `total` method. Verifies the
    // receiver-type-keyed dispatch picks the right one for each
    // call. Also exercises the resolver's per-type method namespace.
    assert_parity(
        "type Order:\n    amount: Int\n\ntype Line:\n    units: Int\n\nextend Order:\n    public agent total(o: Order) -> Int:\n        return o.amount\n\nextend Line:\n    public agent total(l: Line) -> Int:\n        return l.units * 10\n\nagent main() -> Int:\n    o = Order(5)\n    l = Line(3)\n    return o.total() + l.total()\n",
        35,
    );
}

#[test]
fn method_with_string_field_releases_correctly() {
    // Method on a struct with a refcounted (String) field. Leak
    // detector verifies the receiver's refcount lifecycle stays
    // balanced through the method-call rewrite.
    assert_parity_bool(
        "type Named:\n    label: String\n\nextend Named:\n    public agent matches(n: Named, query: String) -> Bool:\n        return n.label == query\n\nagent main() -> Bool:\n    n = Named(\"hello\")\n    return n.matches(\"hello\")\n",
        true,
    );
}

#[test]
fn weak_upgrade_is_live_while_strong_value_is_still_in_scope() {
    assert_parity_bool(
        "agent main() -> Bool:\n    s = \"hello\"\n    w = Weak::new(s)\n    return Weak::upgrade(w) != None\n",
        true,
    );
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
