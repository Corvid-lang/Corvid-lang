#![allow(unsafe_code)]

mod approval_bridge;
mod invoke_matrix;
pub use approval_bridge::{
    corvid_approval_predicate_json, corvid_clear_approver, corvid_evaluate_approval_predicate,
    corvid_mark_preapproved_request, corvid_record_host_event, corvid_register_approver,
    corvid_register_approver_from_source, CorvidHostEventStatus,
};
pub(crate) use approval_bridge::{
    decide_registered_approval, mark_preapproved_request, request_host_approval,
    take_last_approval_detail, ApprovalRequestOutcome,
};
use approval_bridge::{owned_approval_to_c, owned_preflight_to_c};
pub(crate) use invoke_matrix::build_scalar_invoker;

use crate::abi::CorvidString;
use crate::catalog::{
    call_agent, descriptor_hash, descriptor_json_ptr, list_agent_handles_owned, pre_flight,
    CorvidAgentHandle, CorvidApprovalRequired, CorvidCallStatus, CorvidFindAgentsResult,
    CorvidPreFlight, CorvidPreFlightStatus,
};
use crate::effect_filter::CorvidFindAgentsStatus;
use crate::errors::RuntimeError;
use crate::ffi_bridge::read_corvid_string;
use crate::grounded_handles;
use crate::observation_handles;
#[cfg(unix)]
use corvid_abi::{parse_embedded_section_bytes, CORVID_ABI_DESCRIPTOR_SYMBOL};
use corvid_abi::{read_embedded_section_from_library, EmbeddedDescriptorSection};
use std::cell::RefCell;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::PathBuf;
use std::ptr;

thread_local! {
    static TRANSIENT_STRINGS: RefCell<Vec<CString>> = RefCell::new(Vec::new());
}

pub(crate) fn load_embedded_descriptor_from_current_library(
) -> Result<EmbeddedDescriptorSection, RuntimeError> {
    #[cfg(unix)]
    unsafe {
        let ptr = resolve_current_library_symbol(CORVID_ABI_DESCRIPTOR_SYMBOL)?;
        let header = std::slice::from_raw_parts(ptr.cast::<u8>(), 16);
        let json_len = u64::from_le_bytes(header[8..16].try_into().expect("len width"));
        let total_len = usize::try_from(json_len)
            .ok()
            .and_then(|len| len.checked_add(16 + 32))
            .ok_or_else(|| {
                RuntimeError::Other(format!("embedded descriptor length overflow: {json_len}"))
            })?;
        let bytes = std::slice::from_raw_parts(ptr.cast::<u8>(), total_len);
        return parse_embedded_section_bytes(bytes)
            .map_err(|err| RuntimeError::Other(format!("parse embedded descriptor: {err}")));
    }

    #[cfg(windows)]
    {
        let path = current_library_path()?;
        return read_embedded_section_from_library(&path).map_err(|err| {
            RuntimeError::Other(format!(
                "read embedded descriptor from `{}`: {err}",
                path.display()
            ))
        });
    }
}

#[cfg(windows)]
fn current_library_path() -> Result<PathBuf, RuntimeError> {
    use windows_sys::Win32::Foundation::HMODULE;
    use windows_sys::Win32::System::LibraryLoader::{
        GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
        GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    };

    unsafe {
        let mut module: HMODULE = std::ptr::null_mut();
        let anchor = corvid_abi_descriptor_json as *const ();
        let ok = GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            anchor.cast(),
            &mut module,
        );
        if ok == 0 || module.is_null() {
            return Err(RuntimeError::Other(
                "resolve current Corvid module handle".to_string(),
            ));
        }

        let mut buf = vec![0u16; 260];
        loop {
            let written = GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32);
            if written == 0 {
                return Err(RuntimeError::Other(
                    "resolve current Corvid module path".to_string(),
                ));
            }
            let written = written as usize;
            if written < buf.len() - 1 {
                buf.truncate(written);
                let path = String::from_utf16(&buf).map_err(|err| {
                    RuntimeError::Other(format!("module path UTF-16 decode: {err}"))
                })?;
                return Ok(PathBuf::from(path));
            }
            buf.resize(buf.len() * 2, 0);
        }
    }
}

unsafe fn resolve_current_library_symbol(symbol: &str) -> Result<*const c_void, RuntimeError> {
    #[cfg(unix)]
    {
        let lib = libloading::os::unix::Library::this();
        let export = lib
            .get::<*const c_void>(format!("{symbol}\0").as_bytes())
            .map_err(|err| RuntimeError::Other(format!("resolve symbol `{symbol}`: {err}")))?;
        return Ok(*export);
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::HMODULE;
        use windows_sys::Win32::System::LibraryLoader::{
            GetModuleHandleExA, GetProcAddress, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
            GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        };

        let mut module: HMODULE = std::ptr::null_mut();
        let anchor = corvid_register_approver as *const ();
        let ok = GetModuleHandleExA(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            anchor.cast(),
            &mut module,
        );
        if ok == 0 || module.is_null() {
            return Err(RuntimeError::Other(
                "resolve current Corvid module handle".to_string(),
            ));
        }
        let symbol_c = CString::new(symbol)
            .map_err(|err| RuntimeError::Other(format!("symbol name contained NUL: {err}")))?;
        let ptr = GetProcAddress(module, symbol_c.as_ptr().cast());
        let Some(ptr) = ptr else {
            return Err(RuntimeError::Other(format!(
                "resolve symbol `{symbol}`: not found"
            )));
        };
        return Ok(ptr as *const c_void);
    }
}

fn reset_transients() {
    TRANSIENT_STRINGS.with(|arena| arena.borrow_mut().clear());
}

fn stash_transient(text: &str) -> *const c_char {
    if text.is_empty() {
        return ptr::null();
    }
    TRANSIENT_STRINGS.with(|arena| {
        let mut arena = arena.borrow_mut();
        let c = CString::new(text).expect("transient text contained NUL");
        let ptr = c.as_ptr();
        arena.push(c);
        ptr
    })
}

unsafe fn read_c_string(ptr: *const c_char) -> Result<String, CorvidCallStatus> {
    if ptr.is_null() {
        return Err(CorvidCallStatus::BadArgs);
    }
    CStr::from_ptr(ptr)
        .to_str()
        .map(|value| value.to_owned())
        .map_err(|_| CorvidCallStatus::BadArgs)
}

fn grounded_source_pointers(handle: u64) -> Option<Vec<*const c_char>> {
    let sources = grounded_handles::sources_for_handle(handle)?;
    reset_transients();
    Some(
        sources
            .into_iter()
            .map(|source| stash_transient(&source))
            .collect(),
    )
}

#[no_mangle]
pub unsafe extern "C" fn corvid_grounded_sources(
    handle: u64,
    out: *mut *const c_char,
    capacity: usize,
) -> i32 {
    let Some(sources) = grounded_source_pointers(handle) else {
        return -1;
    };
    if !out.is_null() {
        let count = sources.len().min(capacity);
        ptr::copy_nonoverlapping(sources.as_ptr(), out, count);
    }
    sources.len() as i32
}

#[no_mangle]
pub extern "C" fn corvid_grounded_confidence(handle: u64) -> f64 {
    grounded_handles::confidence_for_handle(handle).unwrap_or(f64::NAN)
}

#[no_mangle]
pub extern "C" fn corvid_grounded_release(handle: u64) {
    let released = grounded_handles::release_handle(handle);
    if cfg!(debug_assertions) && handle != grounded_handles::NULL_GROUNDED_HANDLE && !released {
        eprintln!("warning: grounded handle {handle} was already released or never existed");
    }
}

#[no_mangle]
pub extern "C" fn corvid_observation_cost_usd(handle: u64) -> f64 {
    observation_handles::cost_usd_for_handle(handle).unwrap_or(f64::NAN)
}

#[no_mangle]
pub extern "C" fn corvid_observation_latency_ms(handle: u64) -> u64 {
    observation_handles::latency_ms_for_handle(handle).unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn corvid_observation_tokens_in(handle: u64) -> u64 {
    observation_handles::tokens_in_for_handle(handle).unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn corvid_observation_tokens_out(handle: u64) -> u64 {
    observation_handles::tokens_out_for_handle(handle).unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn corvid_observation_exceeded_bound(handle: u64) -> bool {
    observation_handles::exceeded_bound_for_handle(handle).unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn corvid_observation_release(handle: u64) {
    let released = observation_handles::release_handle(handle);
    if cfg!(debug_assertions) && handle != observation_handles::NULL_OBSERVATION_HANDLE && !released
    {
        eprintln!("warning: observation handle {handle} was already released or never existed");
    }
}

#[no_mangle]
pub extern "C" fn corvid_begin_direct_observation(declared_bound_usd: f64) {
    let declared_bound = if declared_bound_usd.is_finite() {
        Some(declared_bound_usd)
    } else {
        None
    };
    observation_handles::begin_direct_observation(declared_bound);
}

#[no_mangle]
pub unsafe extern "C" fn corvid_finish_direct_observation(out_handle: *mut u64) {
    if out_handle.is_null() {
        let _ = observation_handles::finish_direct_observation();
        return;
    }
    *out_handle = observation_handles::finish_direct_observation();
}

#[no_mangle]
pub unsafe extern "C" fn corvid_grounded_attest_int(
    value: i64,
    source_name: CorvidString,
    confidence: f64,
) -> i64 {
    let source = read_corvid_string(source_name);
    let chain =
        crate::provenance::ProvenanceChain::with_retrieval(&source, crate::tracing::now_ms());
    grounded_handles::set_last_scalar_attestation(grounded_handles::make_attestation(
        chain, confidence,
    ));
    value
}

#[no_mangle]
pub unsafe extern "C" fn corvid_grounded_attest_float(
    value: f64,
    source_name: CorvidString,
    confidence: f64,
) -> f64 {
    let source = read_corvid_string(source_name);
    let chain =
        crate::provenance::ProvenanceChain::with_retrieval(&source, crate::tracing::now_ms());
    grounded_handles::set_last_scalar_attestation(grounded_handles::make_attestation(
        chain, confidence,
    ));
    value
}

#[no_mangle]
pub unsafe extern "C" fn corvid_grounded_attest_bool(
    value: bool,
    source_name: CorvidString,
    confidence: f64,
) -> bool {
    let source = read_corvid_string(source_name);
    let chain =
        crate::provenance::ProvenanceChain::with_retrieval(&source, crate::tracing::now_ms());
    grounded_handles::set_last_scalar_attestation(grounded_handles::make_attestation(
        chain, confidence,
    ));
    value
}

#[no_mangle]
pub unsafe extern "C" fn corvid_grounded_attest_string(
    value: CorvidString,
    source_name: CorvidString,
    confidence: f64,
) -> CorvidString {
    let source = read_corvid_string(source_name);
    let chain =
        crate::provenance::ProvenanceChain::with_retrieval(&source, crate::tracing::now_ms());
    grounded_handles::attach_string_attestation(
        value.descriptor_key(),
        grounded_handles::make_attestation(chain, confidence),
    );
    value
}

#[no_mangle]
pub extern "C" fn corvid_grounded_capture_scalar_handle() -> u64 {
    grounded_handles::register_handle_for_last_scalar()
}

#[no_mangle]
pub unsafe extern "C" fn corvid_grounded_capture_string_handle(value: CorvidString) -> u64 {
    grounded_handles::register_handle_for_string_ptr(value.descriptor_key())
}

#[no_mangle]
pub unsafe extern "C" fn corvid_abi_descriptor_json(out_len: *mut usize) -> *const c_char {
    match descriptor_json_ptr() {
        Ok((ptr, len)) => {
            if !out_len.is_null() {
                *out_len = len;
            }
            ptr
        }
        Err(_) => {
            if !out_len.is_null() {
                *out_len = 0;
            }
            ptr::null()
        }
    }
}

#[no_mangle]
pub extern "C" fn corvid_abi_descriptor_hash(out_hash: *mut u8) {
    if out_hash.is_null() {
        return;
    }
    if let Ok(hash) = descriptor_hash() {
        unsafe {
            ptr::copy_nonoverlapping(hash.as_ptr(), out_hash, hash.len());
        }
    }
}

#[no_mangle]
pub extern "C" fn corvid_abi_verify(expected: *const u8) -> i32 {
    if expected.is_null() {
        return 0;
    }
    let mut expected_hash = [0u8; 32];
    unsafe {
        ptr::copy_nonoverlapping(expected, expected_hash.as_mut_ptr(), expected_hash.len());
    }
    match crate::catalog::verify_hash(&expected_hash) {
        Ok(true) => 1,
        Ok(false) | Err(_) => 0,
    }
}

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
    reset_transients();
    if filter_ptr.is_null() {
        return CorvidFindAgentsResult {
            status: CorvidFindAgentsStatus::BadJson,
            matched_count: 0,
            error_message: stash_transient("filter JSON pointer was null"),
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
            .map(stash_transient)
            .unwrap_or(ptr::null()),
    }
}

#[no_mangle]
pub unsafe extern "C" fn corvid_agent_signature_json(
    agent_name: *const c_char,
    out_len: *mut usize,
) -> *const c_char {
    let Ok(agent_name) = read_c_string(agent_name) else {
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
    let Ok(agent_name) = read_c_string(agent_name) else {
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
    reset_transients();
    owned_preflight_to_c(&pre_flight(&agent_name, &args_json))
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
    let Ok(agent_name) = read_c_string(agent_name) else {
        return CorvidCallStatus::BadArgs;
    };
    let args_json = if args_json.is_null() {
        String::new()
    } else {
        let bytes = std::slice::from_raw_parts(args_json as *const u8, args_len);
        String::from_utf8_lossy(bytes).into_owned()
    };
    reset_transients();
    let outcome = call_agent(&agent_name, &args_json);
    if !out_observation_handle.is_null() {
        *out_observation_handle = outcome.observation_handle;
    }
    if let Some(approval) = &outcome.approval {
        if !out_approval.is_null() {
            *out_approval = owned_approval_to_c(approval);
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
