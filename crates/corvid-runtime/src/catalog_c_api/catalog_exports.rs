//! Catalog C exports for listing, filtering, preflight, and calls.

use crate::catalog::{
    call_agent, list_agent_handles_owned, pre_flight, CorvidAgentHandle, CorvidApprovalRequired,
    CorvidCallStatus, CorvidFindAgentsResult, CorvidPreFlight, CorvidPreFlightStatus,
};
use crate::effect_filter::CorvidFindAgentsStatus;
use crate::observation_handles;
use std::ffi::{c_char, CString};
use std::ptr;

#[no_mangle]
pub unsafe extern "C" fn corvid_list_agents(out: *mut CorvidAgentHandle, capacity: usize) -> usize {
    let Ok(handles) = list_agent_handles_owned() else {
        return 0;
    };
    if !out.is_null() {
        let count = handles.len().min(capacity);
        ptr::copy_nonoverlapping(handles.as_ptr(), out, count);
    }
    handles.len()
}

#[no_mangle]
pub unsafe extern "C" fn corvid_find_agents_where(
    filter_ptr: *const c_char,
    filter_len: usize,
    out_indices: *mut usize,
    out_cap: usize,
) -> CorvidFindAgentsResult {
    super::grounded_bridge::reset_transients();
    if filter_ptr.is_null() {
        return CorvidFindAgentsResult {
            status: CorvidFindAgentsStatus::BadJson,
            matched_count: 0,
            error_message: super::grounded_bridge::stash_transient("filter JSON pointer was null"),
        };
    }
    let bytes = std::slice::from_raw_parts(filter_ptr as *const u8, filter_len);
    let filter_json = String::from_utf8_lossy(bytes).into_owned();
    let outcome = crate::catalog::find_agents_where(&filter_json);
    if !out_indices.is_null() {
        let count = outcome.matched_indices.len().min(out_cap);
        ptr::copy_nonoverlapping(outcome.matched_indices.as_ptr(), out_indices, count);
    }
    CorvidFindAgentsResult {
        status: outcome.status,
        matched_count: outcome.matched_indices.len(),
        error_message: outcome
            .error_message
            .as_deref()
            .map(super::grounded_bridge::stash_transient)
            .unwrap_or(ptr::null()),
    }
}

#[no_mangle]
pub unsafe extern "C" fn corvid_agent_signature_json(
    agent_name: *const c_char,
    out_len: *mut usize,
) -> *const c_char {
    let Ok(agent_name) = super::grounded_bridge::read_c_string(agent_name) else {
        if !out_len.is_null() {
            *out_len = 0;
        }
        return ptr::null();
    };
    match crate::catalog::agent_signature_json(&agent_name) {
        Ok(Some((_json, len, ptr))) => {
            if !out_len.is_null() {
                *out_len = len;
            }
            ptr
        }
        _ => {
            if !out_len.is_null() {
                *out_len = 0;
            }
            ptr::null()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn corvid_pre_flight(
    agent_name: *const c_char,
    args_json: *const c_char,
    args_len: usize,
) -> CorvidPreFlight {
    let Ok(agent_name) = super::grounded_bridge::read_c_string(agent_name) else {
        return CorvidPreFlight {
            status: CorvidPreFlightStatus::BadArgs,
            cost_bound_usd: f64::NAN,
            requires_approval: 0,
            effect_row_json: ptr::null(),
            grounded_source_set_json: ptr::null(),
            bad_args_message: ptr::null(),
        };
    };
    let args_json = if args_json.is_null() {
        String::new()
    } else {
        let bytes = std::slice::from_raw_parts(args_json as *const u8, args_len);
        String::from_utf8_lossy(bytes).into_owned()
    };
    super::grounded_bridge::reset_transients();
    super::approval_bridge::owned_preflight_to_c(&pre_flight(&agent_name, &args_json))
}

#[no_mangle]
pub unsafe extern "C" fn corvid_call_agent(
    agent_name: *const c_char,
    args_json: *const c_char,
    args_len: usize,
    out_result: *mut *mut c_char,
    out_result_len: *mut usize,
    out_observation_handle: *mut u64,
    out_approval: *mut CorvidApprovalRequired,
) -> CorvidCallStatus {
    if !out_result.is_null() {
        *out_result = ptr::null_mut();
    }
    if !out_result_len.is_null() {
        *out_result_len = 0;
    }
    if !out_observation_handle.is_null() {
        *out_observation_handle = observation_handles::NULL_OBSERVATION_HANDLE;
    }
    let Ok(agent_name) = super::grounded_bridge::read_c_string(agent_name) else {
        return CorvidCallStatus::BadArgs;
    };
    let args_json = if args_json.is_null() {
        String::new()
    } else {
        let bytes = std::slice::from_raw_parts(args_json as *const u8, args_len);
        String::from_utf8_lossy(bytes).into_owned()
    };
    super::grounded_bridge::reset_transients();
    let outcome = call_agent(&agent_name, &args_json);
    if !out_observation_handle.is_null() {
        *out_observation_handle = outcome.observation_handle;
    }
    if let Some(approval) = &outcome.approval {
        if !out_approval.is_null() {
            *out_approval = super::approval_bridge::owned_approval_to_c(approval);
        }
    }
    if let Some(result_json) = &outcome.result_json {
        if let Ok(c_result) = CString::new(result_json.as_str()) {
            let len = result_json.len();
            let raw = c_result.into_raw();
            if !out_result.is_null() {
                *out_result = raw;
            }
            if !out_result_len.is_null() {
                *out_result_len = len;
            }
        }
    }
    outcome.status
}

#[no_mangle]
pub unsafe extern "C" fn corvid_free_result(result: *mut c_char) {
    if result.is_null() {
        return;
    }
    let _ = CString::from_raw(result);
}
