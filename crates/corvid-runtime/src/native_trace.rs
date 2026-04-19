#![allow(unsafe_code)]

use crate::abi::CorvidString;
use crate::ffi_bridge::{borrow_corvid_string, bridge};
use crate::runtime::Runtime;
use crate::tracing::now_ms;
use corvid_trace_schema::TraceEvent;
use serde_json::Value;

fn runtime() -> std::sync::Arc<Runtime> {
    bridge().corvid_runtime()
}

pub(crate) unsafe fn decode_trace_values(
    type_tags: &str,
    value_count: i64,
    values_ptr: i64,
) -> Vec<Value> {
    if value_count <= 0 {
        return Vec::new();
    }
    let count = value_count as usize;
    assert_eq!(
        type_tags.chars().count(),
        count,
        "native trace arg tag/value count mismatch: tags=`{type_tags}` count={count}"
    );
    assert_ne!(
        values_ptr, 0,
        "native trace values pointer was null for non-empty payload"
    );

    let base = values_ptr as usize as *const u8;
    type_tags
        .chars()
        .enumerate()
        .map(|(idx, tag)| unsafe { decode_slot_json(base.add(idx * 8), tag) })
        .collect()
}

unsafe fn decode_slot_json(slot_ptr: *const u8, tag: char) -> Value {
    match tag {
        'i' => Value::from(unsafe { *(slot_ptr as *const i64) }),
        'b' => Value::from(unsafe { *(slot_ptr as *const i64) != 0 }),
        'f' => {
            let value = unsafe { *(slot_ptr as *const f64) };
            serde_json::Number::from_f64(value)
                .map(Value::Number)
                .unwrap_or_else(|| Value::String(value.to_string()))
        }
        's' => {
            let descriptor = unsafe { *(slot_ptr as *const i64) };
            Value::String(unsafe { borrow_descriptor_string(descriptor) })
        }
        other => panic!("unsupported native trace value tag `{other}`"),
    }
}

unsafe fn borrow_descriptor_string(descriptor: i64) -> String {
    let corvid: CorvidString = unsafe { std::mem::transmute((descriptor as usize) as *const u8) };
    unsafe { borrow_corvid_string(&corvid) }.to_owned()
}

fn emit(event: TraceEvent) {
    let runtime = runtime();
    let tracer = runtime.tracer();
    if tracer.is_enabled() {
        tracer.emit(event);
    }
}

fn scalar_result_int(value: i64) -> Value {
    Value::from(value)
}

fn scalar_result_bool(value: i8) -> Value {
    Value::from(value != 0)
}

fn scalar_result_float(value: f64) -> Value {
    serde_json::Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or_else(|| Value::String(value.to_string()))
}

unsafe fn scalar_result_string(value: CorvidString) -> Value {
    Value::String(unsafe { borrow_corvid_string(&value) }.to_owned())
}

#[no_mangle]
pub unsafe extern "C" fn corvid_trace_run_started(
    agent: CorvidString,
    arg_types: CorvidString,
    argc: i64,
    args_ptr: i64,
) {
    let agent_name = unsafe { borrow_corvid_string(&agent) }.to_owned();
    let arg_tags = unsafe { borrow_corvid_string(&arg_types) };
    let args = unsafe { decode_trace_values(arg_tags, argc, args_ptr) };
    let runtime = runtime();
    let _ = runtime.prepare_run(&agent_name, &args);
    emit(TraceEvent::RunStarted {
        ts_ms: now_ms(),
        run_id: runtime.tracer().run_id().to_string(),
        agent: agent_name,
        args,
    });
}

#[no_mangle]
pub extern "C" fn corvid_trace_run_completed_int(value: i64) {
    emit_run_completed(scalar_result_int(value));
}

#[no_mangle]
pub extern "C" fn corvid_trace_run_completed_bool(value: i8) {
    emit_run_completed(scalar_result_bool(value));
}

#[no_mangle]
pub extern "C" fn corvid_trace_run_completed_float(value: f64) {
    emit_run_completed(scalar_result_float(value));
}

#[no_mangle]
pub unsafe extern "C" fn corvid_trace_run_completed_string(value: CorvidString) {
    emit_run_completed(unsafe { scalar_result_string(value) });
}

fn emit_run_completed(result: Value) {
    let runtime = runtime();
    let _ = runtime.complete_run(true, Some(&result), None);
    emit(TraceEvent::RunCompleted {
        ts_ms: now_ms(),
        run_id: runtime.tracer().run_id().to_string(),
        ok: true,
        result: Some(result),
        error: None,
    });
}

#[no_mangle]
pub unsafe extern "C" fn corvid_trace_tool_call(
    tool: CorvidString,
    arg_types: CorvidString,
    argc: i64,
    args_ptr: i64,
) {
    let tool_name = unsafe { borrow_corvid_string(&tool) }.to_owned();
    let arg_tags = unsafe { borrow_corvid_string(&arg_types) };
    let args = unsafe { decode_trace_values(arg_tags, argc, args_ptr) };
    let runtime = runtime();
    emit(TraceEvent::ToolCall {
        ts_ms: now_ms(),
        run_id: runtime.tracer().run_id().to_string(),
        tool: tool_name,
        args,
    });
}

#[no_mangle]
pub unsafe extern "C" fn corvid_trace_tool_result_null(tool: CorvidString) {
    let tool_name = unsafe { borrow_corvid_string(&tool) }.to_owned();
    emit_tool_result(tool_name, Value::Null);
}

#[no_mangle]
pub unsafe extern "C" fn corvid_trace_tool_result_int(tool: CorvidString, value: i64) {
    let tool_name = unsafe { borrow_corvid_string(&tool) }.to_owned();
    emit_tool_result(tool_name, scalar_result_int(value));
}

#[no_mangle]
pub unsafe extern "C" fn corvid_trace_tool_result_bool(tool: CorvidString, value: i8) {
    let tool_name = unsafe { borrow_corvid_string(&tool) }.to_owned();
    emit_tool_result(tool_name, scalar_result_bool(value));
}

#[no_mangle]
pub unsafe extern "C" fn corvid_trace_tool_result_float(tool: CorvidString, value: f64) {
    let tool_name = unsafe { borrow_corvid_string(&tool) }.to_owned();
    emit_tool_result(tool_name, scalar_result_float(value));
}

#[no_mangle]
pub unsafe extern "C" fn corvid_trace_tool_result_string(tool: CorvidString, value: CorvidString) {
    let tool_name = unsafe { borrow_corvid_string(&tool) }.to_owned();
    emit_tool_result(tool_name, unsafe { scalar_result_string(value) });
}

fn emit_tool_result(tool: String, result: Value) {
    let runtime = runtime();
    emit(TraceEvent::ToolResult {
        ts_ms: now_ms(),
        run_id: runtime.tracer().run_id().to_string(),
        tool,
        result,
    });
}

#[cfg(test)]
mod tests {
    use super::decode_trace_values;
    use serde_json::json;

    #[test]
    fn decode_trace_values_handles_scalar_tags() {
        let ints = [7_i64, 1_i64];
        let values = unsafe { decode_trace_values("ib", 2, ints.as_ptr() as usize as i64) };
        assert_eq!(values, vec![json!(7), json!(true)]);
    }

    #[test]
    fn decode_trace_values_handles_float_bits() {
        let floats = [3.5_f64];
        let values =
            unsafe { decode_trace_values("f", 1, floats.as_ptr() as usize as i64) };
        assert_eq!(values, vec![json!(3.5)]);
    }
}
