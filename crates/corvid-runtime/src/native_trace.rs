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
        'j' => {
            // Non-scalar args land here: codegen-cl built a JSON
            // string for the value via the corvid_json_buffer_*
            // helpers and stored a Corvid String descriptor
            // pointer in the slot. Parse it back into a structured
            // Value so the trace event preserves the same shape the
            // program saw.
            let descriptor = unsafe { *(slot_ptr as *const i64) };
            let json_text = unsafe { borrow_descriptor_string(descriptor) };
            serde_json::from_str::<Value>(&json_text).unwrap_or_else(|err| {
                panic!(
                    "native trace 'j'-tagged slot held malformed JSON `{json_text}`: {err}"
                )
            })
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
    runtime
        .prepare_run(&agent_name, &args)
        .unwrap_or_else(|err| panic!("corvid_trace_run_started failed: {err}"));
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
    runtime
        .complete_run(true, Some(&result), None)
        .unwrap_or_else(|err| panic!("corvid_trace_run_completed failed: {err}"));
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

    extern "C" {
        fn corvid_json_buffer_new() -> *mut std::ffi::c_void;
        fn corvid_json_buffer_finish(buf: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
        fn corvid_json_buffer_append_raw(
            buf: *mut std::ffi::c_void,
            desc: *mut std::ffi::c_void,
        );
        fn corvid_json_buffer_append_int(buf: *mut std::ffi::c_void, value: i64);
        fn corvid_json_buffer_append_float(buf: *mut std::ffi::c_void, value: f64);
        fn corvid_json_buffer_append_bool(buf: *mut std::ffi::c_void, value: i8);
        fn corvid_json_buffer_append_null(buf: *mut std::ffi::c_void);
        fn corvid_json_buffer_append_string(
            buf: *mut std::ffi::c_void,
            desc: *mut std::ffi::c_void,
        );
    }

    /// Drop a Corvid String descriptor obtained through these tests.
    /// Matches `ffi_bridge.rs`'s extern signature byte-for-byte (the
    /// pointer width of `*const u8` and `*mut c_void` is identical on
    /// every supported target) so rustc does not warn about a
    /// duplicate `corvid_release` extern with a different signature.
    unsafe fn corvid_release(payload: *mut std::ffi::c_void) {
        extern "C" {
            fn corvid_release(descriptor: *const u8);
        }
        corvid_release(payload as *const u8);
    }

    /// Same trick for `corvid_string_from_bytes` — `ffi_bridge.rs`
    /// declares it returning `*const u8`; cast to `*mut c_void` here
    /// so the test code reads as one consistent pointer kind.
    unsafe fn corvid_string_from_bytes(bytes: *const u8, len: i64) -> *mut std::ffi::c_void {
        extern "C" {
            fn corvid_string_from_bytes(bytes: *const u8, length: i64) -> *const u8;
        }
        corvid_string_from_bytes(bytes, len) as *mut std::ffi::c_void
    }

    /// Borrow the bytes inside a Corvid String descriptor without
    /// changing its refcount. The String descriptor layout has
    /// bytes_ptr at offset 0 and length at offset 8.
    unsafe fn descriptor_text(desc: *mut std::ffi::c_void) -> String {
        let bytes_ptr = *(desc as *const *const u8);
        let length = *((desc as *const u8).add(8) as *const i64);
        let slice = std::slice::from_raw_parts(bytes_ptr, length as usize);
        std::str::from_utf8(slice).expect("utf8").to_owned()
    }

    /// Allocate a fresh Corvid String via the runtime's regular
    /// allocator so the JSON helpers see the same descriptor layout
    /// codegen would feed them.
    unsafe fn make_string(s: &str) -> *mut std::ffi::c_void {
        corvid_string_from_bytes(s.as_ptr(), s.len() as i64)
    }

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

    #[test]
    fn json_buffer_roundtrips_struct_shape() {
        // Mirrors what codegen emits for `Refund(id="r-001", amount=42)`:
        // open delimiter + field-name literals + scalar appends + close.
        unsafe {
            let buf = corvid_json_buffer_new();
            let open_id = make_string("{\"id\":");
            corvid_json_buffer_append_raw(buf, open_id);
            let id_value = make_string("r-001");
            corvid_json_buffer_append_string(buf, id_value);
            let next_amount = make_string(",\"amount\":");
            corvid_json_buffer_append_raw(buf, next_amount);
            corvid_json_buffer_append_int(buf, 42);
            let close = make_string("}");
            corvid_json_buffer_append_raw(buf, close);
            let result = corvid_json_buffer_finish(buf);
            let text = descriptor_text(result);
            corvid_release(result);
            corvid_release(open_id);
            corvid_release(id_value);
            corvid_release(next_amount);
            corvid_release(close);
            assert_eq!(text, r#"{"id":"r-001","amount":42}"#);
        }
    }

    #[test]
    fn json_buffer_escapes_string_payload() {
        unsafe {
            let buf = corvid_json_buffer_new();
            let s = make_string("a\"b\\c\nd");
            corvid_json_buffer_append_string(buf, s);
            let result = corvid_json_buffer_finish(buf);
            let text = descriptor_text(result);
            corvid_release(result);
            corvid_release(s);
            assert_eq!(text, "\"a\\\"b\\\\c\\nd\"");
        }
    }

    #[test]
    fn json_buffer_emits_null_for_non_finite_floats() {
        unsafe {
            let buf = corvid_json_buffer_new();
            corvid_json_buffer_append_float(buf, f64::NAN);
            let nan_result = corvid_json_buffer_finish(buf);
            let nan_text = descriptor_text(nan_result);
            corvid_release(nan_result);
            assert_eq!(nan_text, "null");

            let buf = corvid_json_buffer_new();
            corvid_json_buffer_append_float(buf, f64::INFINITY);
            let inf_result = corvid_json_buffer_finish(buf);
            let inf_text = descriptor_text(inf_result);
            corvid_release(inf_result);
            assert_eq!(inf_text, "null");

            let buf = corvid_json_buffer_new();
            corvid_json_buffer_append_null(buf);
            let null_result = corvid_json_buffer_finish(buf);
            let null_text = descriptor_text(null_result);
            corvid_release(null_result);
            assert_eq!(null_text, "null");
        }
    }

    #[test]
    fn json_buffer_renders_bools_and_finite_floats_round_trip_through_serde() {
        unsafe {
            let buf = corvid_json_buffer_new();
            let open = make_string("{\"flag\":");
            corvid_json_buffer_append_raw(buf, open);
            corvid_json_buffer_append_bool(buf, 1);
            let next = make_string(",\"ratio\":");
            corvid_json_buffer_append_raw(buf, next);
            corvid_json_buffer_append_float(buf, 0.25);
            let close = make_string("}");
            corvid_json_buffer_append_raw(buf, close);
            let result = corvid_json_buffer_finish(buf);
            let text = descriptor_text(result);
            corvid_release(result);
            corvid_release(open);
            corvid_release(next);
            corvid_release(close);
            let parsed: serde_json::Value =
                serde_json::from_str(&text).expect("buffer output must be valid JSON");
            assert_eq!(parsed, json!({"flag": true, "ratio": 0.25}));
        }
    }

    #[test]
    fn decode_trace_values_handles_j_tag_via_descriptor_pointer() {
        // Build a JSON String through the same surface codegen uses,
        // place its descriptor pointer in a single 8-byte slot, and
        // confirm the trace decoder routes 'j' through serde_json.
        unsafe {
            let buf = corvid_json_buffer_new();
            let open = make_string("{\"id\":");
            corvid_json_buffer_append_raw(buf, open);
            let id_val = make_string("r-001");
            corvid_json_buffer_append_string(buf, id_val);
            let close = make_string("}");
            corvid_json_buffer_append_raw(buf, close);
            let json_desc = corvid_json_buffer_finish(buf);

            let slot: i64 = json_desc as usize as i64;
            let values = decode_trace_values("j", 1, &slot as *const i64 as usize as i64);

            corvid_release(json_desc);
            corvid_release(open);
            corvid_release(id_val);
            corvid_release(close);

            assert_eq!(values, vec![json!({"id": "r-001"})]);
        }
    }
}
