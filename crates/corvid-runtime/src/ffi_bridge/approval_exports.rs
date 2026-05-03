use super::*;

#[no_mangle]
pub unsafe extern "C" fn corvid_citation_verify_or_panic(
    prompt_name: CorvidString,
    context: CorvidString,
    response: CorvidString,
) -> bool {
    let prompt_name_ref = unsafe { borrow_corvid_string(&prompt_name) };
    let context_ref = unsafe { borrow_corvid_string(&context) };
    let response_ref = unsafe { borrow_corvid_string(&response) };
    if crate::citation::citation_verified(context_ref, response_ref) {
        return true;
    }

    panic!(
        "citation verification failed for prompt `{prompt_name_ref}`: response does not reference content from the cited context parameter"
    );
}

#[no_mangle]
pub unsafe extern "C" fn corvid_approve_sync(
    label: CorvidString,
    arg_types: CorvidString,
    argc: i64,
    args_ptr: i64,
) -> bool {
    let label = unsafe { read_corvid_string(label) };
    let arg_tags = unsafe { borrow_corvid_string(&arg_types) };
    let args = unsafe { crate::native_trace::decode_trace_values(arg_tags, argc, args_ptr) };
    let state = bridge();
    let runtime = state.corvid_runtime();
    let label_for_call = label.clone();
    let result = state
        .tokio_handle()
        .block_on(async move { runtime.approval_gate(&label_for_call, args).await });
    match result {
        Ok(()) => true,
        Err(e) => {
            panic_if_replay_runtime_error(
                &format!("corvid_approve_sync: approval `{label}` failed"),
                &e,
            );
            eprintln!("corvid_approve_sync: approval `{label}` failed: {e}");
            false
        }
    }
}
