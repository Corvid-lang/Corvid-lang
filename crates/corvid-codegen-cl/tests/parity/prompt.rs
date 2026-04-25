use super::{assert_no_leaks, entry_name, ir_of, test_tools_lib_path};
use corvid_codegen_cl::build_native_to_disk;
use corvid_runtime::{ProgrammaticApprover, Runtime};
use corvid_vm::Value;
use std::process::Command;
use std::sync::Arc;

#[track_caller]
fn assert_parity_with_mock_llm(
    src: &str,
    mock_value: serde_json::Value,
    expected: i64,
    model: &str,
    prompt_name: &str,
) {
    let ir = ir_of(src);

    let mock = corvid_runtime::MockAdapter::new(model).reply(prompt_name, mock_value.clone());
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

    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_path = tmp.path().join("prog");
    let produced = build_native_to_disk(
        &ir,
        "corvid_parity_test",
        &bin_path,
        &[test_tools_lib_path().as_path()],
    )
    .expect("compile + link");

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

fn build_prompt_binary(src: &str) -> (tempfile::TempDir, std::path::PathBuf) {
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
    (tmp, produced)
}

#[test]
fn prompt_returns_int() {
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
    assert_parity_with_mock_llm(
        "prompt classify(message: String) -> Int:\n    \"Classify: {message}\"\n\nagent main() -> Int:\n    msg = \"hello\"\n    return classify(msg)\n",
        serde_json::json!(1),
        1,
        "mock-1",
        "classify",
    );
}

#[test]
fn prompt_cites_strictly_native_accepts_grounded_context_phrase() {
    let src = r#"
effect retrieval:
    data: grounded

tool grounded_echo(s: String) -> Grounded<String> uses retrieval

prompt answer(ctx: Grounded<String>) -> Grounded<String>:
    cites ctx strictly
    "Answer from {ctx}"

agent main() -> Grounded<String>:
    ctx = grounded_echo("alpha beta gamma delta epsilon")
    return answer(ctx)
"#;
    let (_tmp, produced) = build_prompt_binary(src);
    let output = Command::new(&produced)
        .env("CORVID_DEBUG_ALLOC", "1")
        .env("CORVID_APPROVE_AUTO", "1")
        .env("CORVID_TEST_MOCK_LLM", "1")
        .env("CORVID_TEST_MOCK_LLM_RESPONSE", "beta gamma delta epsilon")
        .env("CORVID_MODEL", "mock-1")
        .output()
        .expect("run compiled binary");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "compiled binary exited non-zero: status={:?} stdout={stdout} stderr={stderr}",
        output.status.code()
    );
    assert!(
        stdout.contains("beta gamma delta epsilon"),
        "stdout did not contain grounded response: {stdout}"
    );
    assert_no_leaks(&stderr, src);
}

#[test]
fn prompt_cites_strictly_native_rejects_uncited_response() {
    let src = r#"
effect retrieval:
    data: grounded

tool grounded_echo(s: String) -> Grounded<String> uses retrieval

prompt answer(ctx: Grounded<String>) -> Grounded<String>:
    cites ctx strictly
    "Answer from {ctx}"

agent main() -> Grounded<String>:
    ctx = grounded_echo("alpha beta gamma delta epsilon")
    return answer(ctx)
"#;
    let (_tmp, produced) = build_prompt_binary(src);
    let output = Command::new(&produced)
        .env("CORVID_DEBUG_ALLOC", "1")
        .env("CORVID_APPROVE_AUTO", "1")
        .env("CORVID_TEST_MOCK_LLM", "1")
        .env("CORVID_TEST_MOCK_LLM_RESPONSE", "unrelated answer")
        .env("CORVID_MODEL", "mock-1")
        .output()
        .expect("run compiled binary");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        !output.status.success(),
        "compiled binary unexpectedly succeeded; stderr={stderr}"
    );
    assert!(
        stderr.contains("citation verification failed"),
        "stderr did not contain citation failure: {stderr}"
    );
}
