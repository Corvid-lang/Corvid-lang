use super::{
    assert_no_leaks, entry_name, ir_of, run_with_leak_detector_and_mocks, test_tools_lib_path,
};
use corvid_codegen_cl::build_native_to_disk;
use corvid_runtime::{ProgrammaticApprover, Runtime};
use corvid_vm::Value;
use std::sync::Arc;

#[track_caller]
fn assert_parity_with_mock_tools(src: &str, mocks: &[(&str, i64)], expected: i64) {
    let ir = ir_of(src);

    let mut builder = Runtime::builder().approver(Arc::new(ProgrammaticApprover::always_yes()));
    for (name, value) in mocks {
        let v = *value;
        let name_owned = name.to_string();
        builder = builder.tool(name_owned, move |_args| async move { Ok(serde_json::json!(v)) });
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

    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_path = tmp.path().join("prog");
    let produced = build_native_to_disk(
        &ir,
        "corvid_parity_test",
        &bin_path,
        &[test_tools_lib_path().as_path()],
    )
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

#[track_caller]
fn assert_parity_prebuilt_tools<F>(src: &str, expected: i64, register_handlers: F)
where
    F: FnOnce(corvid_runtime::RuntimeBuilder) -> corvid_runtime::RuntimeBuilder,
{
    let ir = ir_of(src);

    let builder = Runtime::builder().approver(Arc::new(ProgrammaticApprover::always_yes()));
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
    let produced = build_native_to_disk(
        &ir,
        "corvid_parity_test",
        &bin_path,
        &[test_tools_lib_path().as_path()],
    )
    .expect("compile + link");
    let (stdout, stderr, status) = run_with_leak_detector_and_mocks(&produced, &[]);
    assert!(
        status.success(),
        "compiled binary exited non-zero: status={:?} stdout={stdout} stderr={stderr} src=\n{src}",
        status.code()
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

#[test]
fn tool_returns_int_directly() {
    assert_parity_with_mock_tools(
        "tool answer() -> Int\n\nagent main() -> Int:\n    return answer()\n",
        &[("answer", 42)],
        42,
    );
}

#[test]
fn tool_result_in_arithmetic() {
    assert_parity_with_mock_tools(
        "tool base() -> Int\n\nagent main() -> Int:\n    return base() * 2 + 5\n",
        &[("base", 10)],
        25,
    );
}

#[test]
fn tool_result_in_conditional() {
    assert_parity_with_mock_tools(
        "tool flag() -> Int\n\nagent main() -> Int:\n    f = flag()\n    if f > 0:\n        return 100\n    return 200\n",
        &[("flag", 7)],
        100,
    );
}

#[test]
fn tool_result_in_conditional_false_branch() {
    assert_parity_with_mock_tools(
        "tool flag() -> Int\n\nagent main() -> Int:\n    f = flag()\n    if f > 0:\n        return 100\n    return 200\n",
        &[("flag", -1)],
        200,
    );
}

#[test]
fn two_tools_added() {
    assert_parity_with_mock_tools(
        "tool a() -> Int\ntool b() -> Int\n\nagent main() -> Int:\n    return a() + b()\n",
        &[("a", 30), ("b", 12)],
        42,
    );
}

#[test]
fn tool_called_from_helper_agent() {
    assert_parity_with_mock_tools(
        "tool leaf() -> Int\n\nagent helper() -> Int:\n    return leaf() + 1\n\nagent main() -> Int:\n    return helper() * 10\n",
        &[("leaf", 4)],
        50,
    );
}

#[test]
fn tool_takes_int_arg() {
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

#[test]
fn approve_before_dangerous_tool_compiles_and_runs() {
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
    assert_parity_prebuilt_tools(
        "tool greet_string(name: String) -> String\ntool string_len(s: String) -> Int\n\nagent main() -> Int:\n    g = greet_string(\"world\")\n    return string_len(g)\n",
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
