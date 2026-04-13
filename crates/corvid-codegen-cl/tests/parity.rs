//! Interpreter-vs-compiled-binary parity tests.
//!
//! Every fixture compiles to a native binary, runs it, and compares the
//! binary's stdout (the printed `i64`) to the interpreter's `Value::Int`.
//! If the two tiers disagree on any fixture, the harness fails — that's
//! the oracle property slice 2a's async decision defended.
//!
//! Slice 12a fixtures: Int-only, pure computation, parameter-less entry
//! agents (the C shim can't pass argv yet). Arithmetic overflow paths
//! assert on stderr + non-zero exit instead of value equality.

use corvid_codegen_cl::build_native_to_disk;
use corvid_ir::lower;
use corvid_resolve::resolve;
use corvid_runtime::{ProgrammaticApprover, Runtime};
use corvid_syntax::{lex, parse_file};
use corvid_types::typecheck;
use corvid_vm::Value;
use std::process::Command;
use std::sync::Arc;

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
    let produced = build_native_to_disk(&ir, "corvid_parity_test", &bin_path)
        .expect("compile + link");

    let output = Command::new(&produced).output().expect("run compiled binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "compiled binary exited non-zero: status={:?} stderr={stderr} stdout={stdout} src=\n{src}",
        output.status.code()
    );
    let compiled_value: i64 = stdout
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("parse stdout `{stdout}` as i64: {e}; src=\n{src}"));
    assert_eq!(
        compiled_value, expected,
        "compiled result mismatch for source:\n{src}\nstderr: {stderr}"
    );
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
    let produced = build_native_to_disk(&ir, "corvid_parity_test", &bin_path)
        .expect("compile + link");
    let output = Command::new(&produced).output().expect("run compiled binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "compiled binary exited non-zero: status={:?} stderr={stderr} stdout={stdout} src=\n{src}",
        output.status.code()
    );
    let printed: i64 = stdout
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("parse stdout `{stdout}` as i64: {e}; src=\n{src}"));
    let compiled_bool = match printed {
        0 => false,
        1 => true,
        other => panic!("expected 0 or 1 for Bool, got {other}; src=\n{src}"),
    };
    assert_eq!(
        compiled_bool, expected,
        "compiled result mismatch for source:\n{src}\nstderr: {stderr}"
    );
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
    let produced = build_native_to_disk(&ir, "corvid_parity_test", &bin_path)
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
// Slice 12b fixtures: Bool, comparisons, if/else, unary ops,
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
