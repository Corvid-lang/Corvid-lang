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
    let produced = build_native_to_disk(&ir, "corvid_parity_test", &bin_path)
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
    let produced = build_native_to_disk(&ir, "corvid_parity_test", &bin_path)
        .expect("compile + link");
    let (stdout, stderr, status) = run_with_leak_detector(&produced);
    assert!(
        status.success(),
        "compiled binary exited non-zero: status={:?} stderr={stderr} stdout={stdout} src=\n{src}",
        status.code()
    );
    let printed: i64 = stdout
        .trim()
        .lines()
        .next()
        .unwrap_or("")
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

// ============================================================
// Slice 12c fixtures: bare local bindings (`x = expr`) and `pass`.
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
// Slice 12d fixtures: Float arithmetic + comparisons + IEEE 754.
// ============================================================
//
// Float entry-agent returns are blocked until slice 12h (the C shim's
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
/// surface a clean `NotSupported` pointing at slice 12h.
#[test]
fn float_entry_return_is_blocked_with_clear_error() {
    use corvid_codegen_cl::{build_native_to_disk, CodegenErrorKind};
    let ir = ir_of("agent f() -> Float:\n    return 3.14\n");
    let tmp = tempfile::tempdir().unwrap();
    let bin_path = tmp.path().join("prog");
    let err = build_native_to_disk(&ir, "corvid_parity_test", &bin_path).unwrap_err();
    match err.kind {
        CodegenErrorKind::NotSupported(ref msg) => {
            assert!(msg.contains("Float"), "expected message to mention Float: {msg}");
            assert!(
                msg.contains("12i"),
                "expected message to point at slice 12i (renumbered from 12h after slice split): {msg}"
            );
        }
        other => panic!("expected NotSupported, got {other:?}"),
    }
}

// ============================================================
// Slice 12f fixtures: String literals, concat, comparisons.
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
// Slice 12g fixtures: Struct construction, field access,
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
    assert_parity_bool(
        "\
type Flag:
    on: Bool
    code: Int

agent main() -> Bool:
    f = Flag(true, 42)
    return f.on
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
