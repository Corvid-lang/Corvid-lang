use super::{assert_no_leaks, ir_of, test_tools_lib_path};
use corvid_codegen_cl::build_native_to_disk;
use corvid_runtime::{ProgrammaticApprover, Runtime};
use corvid_vm::Value;
use std::process::Command;
use std::sync::Arc;

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
    let produced = build_native_to_disk(
        &ir,
        "corvid_parity_test",
        &bin_path,
        &[test_tools_lib_path().as_path()],
    )
    .expect("compile + link");
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
    let src = "agent calc(n: Int) -> Int:\n    return n + 1\n";
    let ir = ir_of(src);
    let tmp = tempfile::tempdir().unwrap();
    let bin_path = tmp.path().join("prog");
    let produced = build_native_to_disk(
        &ir,
        "corvid_parity_test",
        &bin_path,
        &[test_tools_lib_path().as_path()],
    )
    .expect("compile + link");
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
    let src = "agent calc(n: Int) -> Int:\n    return n\n";
    let ir = ir_of(src);
    let tmp = tempfile::tempdir().unwrap();
    let bin_path = tmp.path().join("prog");
    let produced = build_native_to_disk(
        &ir,
        "corvid_parity_test",
        &bin_path,
        &[test_tools_lib_path().as_path()],
    )
    .expect("compile + link");
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
