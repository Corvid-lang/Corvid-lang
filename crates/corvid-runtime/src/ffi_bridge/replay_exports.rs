use super::*;

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
    value.as_i64().unwrap_or_else(|| {
        panic!("corvid native replay tool `{name}` returned non-int JSON: {value}")
    })
}

fn expect_tool_result_bool(name: &str, value: serde_json::Value) -> bool {
    value.as_bool().unwrap_or_else(|| {
        panic!("corvid native replay tool `{name}` returned non-bool JSON: {value}")
    })
}

fn expect_tool_result_float(name: &str, value: serde_json::Value) -> f64 {
    value.as_f64().unwrap_or_else(|| {
        panic!("corvid native replay tool `{name}` returned non-float JSON: {value}")
    })
}

fn expect_tool_result_string(name: &str, value: serde_json::Value) -> String {
    value
        .as_str()
        .unwrap_or_else(|| {
            panic!("corvid native replay tool `{name}` returned non-string JSON: {value}")
        })
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
