#![allow(unsafe_code)]

mod approval_bridge;
mod catalog_exports;
mod grounded_bridge;
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
pub use catalog_exports::{
    corvid_agent_signature_json, corvid_call_agent, corvid_find_agents_where, corvid_free_result,
    corvid_list_agents, corvid_pre_flight,
};
pub use grounded_bridge::{
    corvid_begin_direct_observation, corvid_finish_direct_observation, corvid_grounded_attest_bool,
    corvid_grounded_attest_float, corvid_grounded_attest_int, corvid_grounded_attest_string,
    corvid_grounded_capture_scalar_handle, corvid_grounded_capture_string_handle,
    corvid_grounded_confidence, corvid_grounded_release, corvid_grounded_sources,
    corvid_observation_cost_usd, corvid_observation_exceeded_bound, corvid_observation_latency_ms,
    corvid_observation_release, corvid_observation_tokens_in, corvid_observation_tokens_out,
};
pub(crate) use invoke_matrix::build_scalar_invoker;

use crate::catalog::{descriptor_hash, descriptor_json_ptr};
use crate::errors::RuntimeError;
#[cfg(unix)]
use corvid_abi::{parse_embedded_section_bytes, CORVID_ABI_DESCRIPTOR_SYMBOL};
use corvid_abi::{read_embedded_section_from_library, EmbeddedDescriptorSection};
use std::ffi::{c_char, c_void, CString};
use std::path::PathBuf;
use std::ptr;

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
