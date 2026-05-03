use super::llm_dispatch::{
    build_system_prompt, call_llm_once, format_instruction_bool, format_instruction_float,
    format_instruction_int, parse_bool, parse_float, parse_int, prompt_max_retries,
    trace_mock_llm_attempt, using_env_mock_llm,
};
use super::*;
use crate::llm::mock::{bench_mock_dispatch_ns, bench_prompt_wait_ns, env_mock_string_reply_sync};
use std::time::Instant;

// Typed prompt-dispatch bridges.
//
// One bridge per return type, mirroring the typed-ABI tool design.
// Each takes 4 CorvidString args (prompt name, signature string,
// rendered prompt body, model name) and returns the typed value.
//
// All four bridges follow the same shape internally:
//   1. Read CorvidString args as Rust Strings (borrow, no refcount poke).
//   2. Build a system prompt: function-signature context +
//      return-type-specific format instruction.
//   3. Loop up to CORVID_PROMPT_MAX_RETRIES (default 3):
//      a. Call the adapter via block_on.
//      b. Parse the response into the typed value.
//      c. On parse success, return.
//      d. On parse failure, capture last response for next retry's
//         stronger system prompt.
//   4. After max retries, panic with a clear message including the
//      last LLM response — compiled binary aborts with stderr trail
//      so the user can see what went wrong.
//
// String returns skip the parse-retry loop entirely (a String response
// is by definition parseable as String). The shape stays uniform so
// codegen has the same call pattern for every return type.
//
// Function-signature context is the inventive piece: the system
// prompt explicitly tells the LLM "you are a function with signature
// X — return the appropriate value." Codegen knows the signature at
// compile time and embeds it as a literal. Same prompt body, much
// better LLM behavior because the model has the type contract.
// ------------------------------------------------------------
#[no_mangle]
pub unsafe extern "C" fn corvid_prompt_call_int(
    prompt_name: CorvidString,
    signature: CorvidString,
    rendered: CorvidString,
    model: CorvidString,
    arg_types: CorvidString,
    argc: i64,
    args_ptr: i64,
) -> i64 {
    let bridge_start = Instant::now();
    let prompt_wait_before = bench_prompt_wait_ns();
    let mock_dispatch_before = bench_mock_dispatch_ns();
    let prompt_name_ref = unsafe { borrow_corvid_string(&prompt_name) };
    let rendered_ref = unsafe { borrow_corvid_string(&rendered) };
    let model_ref = unsafe { borrow_corvid_string(&model) };
    let arg_tags = unsafe { borrow_corvid_string(&arg_types) };
    let llm_args = unsafe { crate::native_trace::decode_trace_values(arg_tags, argc, args_ptr) };

    if using_env_mock_llm() {
        let state = bridge();
        let max_retries = prompt_max_retries();
        let mut last_response: Option<String> = None;
        for _attempt in 0..=max_retries {
            match env_mock_string_reply_sync(prompt_name_ref) {
                Some(reply) => {
                    let reply_text = unsafe { borrow_corvid_string(&reply) }.to_owned();
                    let parsed = parse_int(&reply_text);
                    if let Some(value) = parsed {
                        trace_mock_llm_attempt(
                            state,
                            prompt_name_ref,
                            model_ref,
                            rendered_ref,
                            &llm_args,
                            serde_json::Value::from(value),
                        );
                        unsafe { release_string(reply) };
                        record_json_bridge_ns(
                            bridge_start,
                            prompt_wait_before,
                            mock_dispatch_before,
                        );
                        return value;
                    }
                    trace_mock_llm_attempt(
                        state,
                        prompt_name_ref,
                        model_ref,
                        rendered_ref,
                        &llm_args,
                        serde_json::Value::String(reply_text.clone()),
                    );
                    unsafe { release_string(reply) };
                    last_response = Some(reply_text);
                }
                None => break,
            }
        }
        if last_response.is_some() {
            record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
            panic!(
                "corvid prompt `{prompt_name_ref}`: could not parse Int from env-mock response after {} attempts. Last response: {:?}",
                max_retries + 1,
                last_response.as_deref().unwrap_or("(none)")
            );
        }
    }

    let state = bridge();
    let prompt_name = prompt_name_ref.to_owned();
    let signature = unsafe { read_corvid_string(signature) };
    let rendered = unsafe { read_corvid_string(rendered) };
    let model = unsafe { read_corvid_string(model) };

    let max_retries = prompt_max_retries();
    let mut last_response: Option<String> = None;
    for attempt in 0..=max_retries {
        let sys = build_system_prompt(
            &signature,
            format_instruction_int(),
            attempt,
            last_response.as_deref(),
        );
        match call_llm_once(state, &prompt_name, &model, &rendered, &llm_args, &sys) {
            Err(e) => {
                if attempt == max_retries {
                    record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
                    panic!(
                        "corvid prompt `{prompt_name}` (model `{model}`): adapter failed after {} attempts: {e}",
                        attempt + 1
                    );
                }
                continue;
            }
            Ok(text) => match parse_int(&text) {
                Some(v) => {
                    record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
                    return v;
                }
                None => last_response = Some(text),
            },
        }
    }
    record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
    panic!(
        "corvid prompt `{prompt_name}` (model `{model}`): could not parse Int from LLM response after {} attempts. Last response: {:?}",
        max_retries + 1,
        last_response.as_deref().unwrap_or("(none)")
    );
}

#[no_mangle]
pub unsafe extern "C" fn corvid_prompt_call_bool(
    prompt_name: CorvidString,
    signature: CorvidString,
    rendered: CorvidString,
    model: CorvidString,
    arg_types: CorvidString,
    argc: i64,
    args_ptr: i64,
) -> bool {
    let bridge_start = Instant::now();
    let prompt_wait_before = bench_prompt_wait_ns();
    let mock_dispatch_before = bench_mock_dispatch_ns();
    let prompt_name_ref = unsafe { borrow_corvid_string(&prompt_name) };
    let rendered_ref = unsafe { borrow_corvid_string(&rendered) };
    let model_ref = unsafe { borrow_corvid_string(&model) };
    let arg_tags = unsafe { borrow_corvid_string(&arg_types) };
    let llm_args = unsafe { crate::native_trace::decode_trace_values(arg_tags, argc, args_ptr) };

    if using_env_mock_llm() {
        let state = bridge();
        let max_retries = prompt_max_retries();
        let mut last_response: Option<String> = None;
        for _attempt in 0..=max_retries {
            match env_mock_string_reply_sync(prompt_name_ref) {
                Some(reply) => {
                    let reply_text = unsafe { borrow_corvid_string(&reply) }.to_owned();
                    let parsed = parse_bool(&reply_text);
                    if let Some(value) = parsed {
                        trace_mock_llm_attempt(
                            state,
                            prompt_name_ref,
                            model_ref,
                            rendered_ref,
                            &llm_args,
                            serde_json::Value::from(value),
                        );
                        unsafe { release_string(reply) };
                        record_json_bridge_ns(
                            bridge_start,
                            prompt_wait_before,
                            mock_dispatch_before,
                        );
                        return value;
                    }
                    trace_mock_llm_attempt(
                        state,
                        prompt_name_ref,
                        model_ref,
                        rendered_ref,
                        &llm_args,
                        serde_json::Value::String(reply_text.clone()),
                    );
                    unsafe { release_string(reply) };
                    last_response = Some(reply_text);
                }
                None => break,
            }
        }
        if last_response.is_some() {
            record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
            panic!(
                "corvid prompt `{prompt_name_ref}`: could not parse Bool from env-mock response after {} attempts. Last: {:?}",
                max_retries + 1,
                last_response.as_deref().unwrap_or("(none)")
            );
        }
    }

    let state = bridge();
    let prompt_name = prompt_name_ref.to_owned();
    let signature = unsafe { read_corvid_string(signature) };
    let rendered = unsafe { read_corvid_string(rendered) };
    let model = unsafe { read_corvid_string(model) };

    let max_retries = prompt_max_retries();
    let mut last_response: Option<String> = None;
    for attempt in 0..=max_retries {
        let sys = build_system_prompt(
            &signature,
            format_instruction_bool(),
            attempt,
            last_response.as_deref(),
        );
        match call_llm_once(state, &prompt_name, &model, &rendered, &llm_args, &sys) {
            Err(e) => {
                if attempt == max_retries {
                    record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
                    panic!(
                        "corvid prompt `{prompt_name}`: adapter failed after {} attempts: {e}",
                        attempt + 1
                    );
                }
                continue;
            }
            Ok(text) => match parse_bool(&text) {
                Some(v) => {
                    record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
                    return v;
                }
                None => last_response = Some(text),
            },
        }
    }
    record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
    panic!(
        "corvid prompt `{prompt_name}`: could not parse Bool from LLM response after {} attempts. Last: {:?}",
        max_retries + 1,
        last_response.as_deref().unwrap_or("(none)")
    );
}

#[no_mangle]
pub unsafe extern "C" fn corvid_prompt_call_float(
    prompt_name: CorvidString,
    signature: CorvidString,
    rendered: CorvidString,
    model: CorvidString,
    arg_types: CorvidString,
    argc: i64,
    args_ptr: i64,
) -> f64 {
    let bridge_start = Instant::now();
    let prompt_wait_before = bench_prompt_wait_ns();
    let mock_dispatch_before = bench_mock_dispatch_ns();
    let prompt_name_ref = unsafe { borrow_corvid_string(&prompt_name) };
    let rendered_ref = unsafe { borrow_corvid_string(&rendered) };
    let model_ref = unsafe { borrow_corvid_string(&model) };
    let arg_tags = unsafe { borrow_corvid_string(&arg_types) };
    let llm_args = unsafe { crate::native_trace::decode_trace_values(arg_tags, argc, args_ptr) };

    if using_env_mock_llm() {
        let state = bridge();
        let max_retries = prompt_max_retries();
        let mut last_response: Option<String> = None;
        for _attempt in 0..=max_retries {
            match env_mock_string_reply_sync(prompt_name_ref) {
                Some(reply) => {
                    let reply_text = unsafe { borrow_corvid_string(&reply) }.to_owned();
                    let parsed = parse_float(&reply_text);
                    if let Some(value) = parsed {
                        trace_mock_llm_attempt(
                            state,
                            prompt_name_ref,
                            model_ref,
                            rendered_ref,
                            &llm_args,
                            serde_json::Number::from_f64(value)
                                .map(serde_json::Value::Number)
                                .unwrap_or_else(|| serde_json::Value::String(value.to_string())),
                        );
                        unsafe { release_string(reply) };
                        record_json_bridge_ns(
                            bridge_start,
                            prompt_wait_before,
                            mock_dispatch_before,
                        );
                        return value;
                    }
                    trace_mock_llm_attempt(
                        state,
                        prompt_name_ref,
                        model_ref,
                        rendered_ref,
                        &llm_args,
                        serde_json::Value::String(reply_text.clone()),
                    );
                    unsafe { release_string(reply) };
                    last_response = Some(reply_text);
                }
                None => break,
            }
        }
        if last_response.is_some() {
            record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
            panic!(
                "corvid prompt `{prompt_name_ref}`: could not parse Float from env-mock response after {} attempts. Last: {:?}",
                max_retries + 1,
                last_response.as_deref().unwrap_or("(none)")
            );
        }
    }

    let state = bridge();
    let prompt_name = prompt_name_ref.to_owned();
    let signature = unsafe { read_corvid_string(signature) };
    let rendered = unsafe { read_corvid_string(rendered) };
    let model = unsafe { read_corvid_string(model) };

    let max_retries = prompt_max_retries();
    let mut last_response: Option<String> = None;
    for attempt in 0..=max_retries {
        let sys = build_system_prompt(
            &signature,
            format_instruction_float(),
            attempt,
            last_response.as_deref(),
        );
        match call_llm_once(state, &prompt_name, &model, &rendered, &llm_args, &sys) {
            Err(e) => {
                if attempt == max_retries {
                    record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
                    panic!(
                        "corvid prompt `{prompt_name}`: adapter failed after {} attempts: {e}",
                        attempt + 1
                    );
                }
                continue;
            }
            Ok(text) => match parse_float(&text) {
                Some(v) => {
                    record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
                    return v;
                }
                None => last_response = Some(text),
            },
        }
    }
    record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
    panic!(
        "corvid prompt `{prompt_name}`: could not parse Float from LLM response after {} attempts. Last: {:?}",
        max_retries + 1,
        last_response.as_deref().unwrap_or("(none)")
    );
}

#[no_mangle]
pub unsafe extern "C" fn corvid_prompt_call_string(
    prompt_name: CorvidString,
    signature: CorvidString,
    rendered: CorvidString,
    model: CorvidString,
    arg_types: CorvidString,
    argc: i64,
    args_ptr: i64,
) -> CorvidString {
    use crate::abi::IntoCorvidAbi;
    let bridge_start = Instant::now();
    let prompt_wait_before = bench_prompt_wait_ns();
    let mock_dispatch_before = bench_mock_dispatch_ns();
    let prompt_name_ref = unsafe { borrow_corvid_string(&prompt_name) };
    let rendered_ref = unsafe { borrow_corvid_string(&rendered) };
    let model_ref = unsafe { borrow_corvid_string(&model) };
    let arg_tags = unsafe { borrow_corvid_string(&arg_types) };
    let llm_args = unsafe { crate::native_trace::decode_trace_values(arg_tags, argc, args_ptr) };

    if using_env_mock_llm() {
        let state = bridge();
        if let Some(text) = env_mock_string_reply_sync(prompt_name_ref) {
            trace_mock_llm_attempt(
                state,
                prompt_name_ref,
                model_ref,
                rendered_ref,
                &llm_args,
                serde_json::Value::String(unsafe { borrow_corvid_string(&text) }.to_owned()),
            );
            record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
            return text;
        }
    }

    let state = bridge();
    let prompt_name = prompt_name_ref.to_owned();
    let signature = unsafe { read_corvid_string(signature) };
    let rendered = unsafe { read_corvid_string(rendered) };
    let model = unsafe { read_corvid_string(model) };

    // String return: no parse-retry loop. Whatever the LLM returns
    // IS the String. We still call once, and on adapter failure we
    // panic with a clear message — adapter errors are infrastructure
    // problems, not response-format problems.
    let sys = if using_env_mock_llm() {
        String::new()
    } else {
        format!(
            "You are a function with signature `{signature}`. Return the appropriate string value as your full response — no quotes around the value, no explanation, no formatting markers."
        )
    };
    match call_llm_once(state, &prompt_name, &model, &rendered, &llm_args, &sys) {
        Ok(text) => {
            record_json_bridge_ns(bridge_start, prompt_wait_before, mock_dispatch_before);
            text.into_corvid_abi()
        }
        Err(e) => panic!("corvid prompt `{prompt_name}` (model `{model}`): adapter failed: {e}"),
    }
}
