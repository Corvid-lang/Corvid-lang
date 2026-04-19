use super::{
    assert_no_leaks, assert_parity_bool_without_tools, compile_without_tools_lib, entry_name,
    ir_of, QueuedMockAdapter,
};
use corvid_runtime::{ProgrammaticApprover, Runtime};
use corvid_vm::Value;
use std::collections::HashMap;
use std::process::Command;
use std::sync::Arc;
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





