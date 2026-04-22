#![allow(unsafe_code)]

use crate::approvals::{ApprovalDecision, ApprovalRequest};
use crate::approver_bridge::{ApprovalDecisionInfo, ApprovalSiteInput};
use crate::catalog::{
    call_agent, descriptor_hash, descriptor_json_ptr, list_agent_handles_owned, pre_flight,
    CorvidAgentHandle, CorvidApprovalDecision, CorvidApprovalRequired, CorvidApproverFn,
    CorvidCallStatus, CorvidFindAgentsResult, CorvidPreFlight, CorvidPreFlightStatus,
    OwnedApprovalRequired, OwnedPreFlight, ScalarAbiType, ScalarInvoker, ScalarReturnType,
};
use crate::grounded_handles;
use crate::observation_handles;
use crate::abi::CorvidString;
use crate::effect_filter::CorvidFindAgentsStatus;
use crate::errors::RuntimeError;
use crate::ffi_bridge::{bridge, read_corvid_string};
use corvid_abi::{read_embedded_section_from_library, EmbeddedDescriptorSection};
#[cfg(unix)]
use corvid_abi::{parse_embedded_section_bytes, CORVID_ABI_DESCRIPTOR_SYMBOL};
use corvid_trace_schema::TraceEvent;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::PathBuf;
use std::ptr;
use std::sync::{Arc, Mutex};

thread_local! {
    static TRANSIENT_STRINGS: RefCell<Vec<CString>> = RefCell::new(Vec::new());
    static PREAPPROVED_REQUESTS: RefCell<VecDeque<(String, Vec<serde_json::Value>, ApprovalDecisionInfo)>> =
        RefCell::new(VecDeque::new());
    static LAST_APPROVAL_DETAIL: RefCell<Option<ApprovalDecisionInfo>> = RefCell::new(None);
}

#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CorvidHostEventStatus {
    Ok = 0,
    BadJson = 1,
    TraceDisabled = 2,
    RuntimeError = 3,
}

#[derive(Default)]
struct ApproverRegistration {
    callback: Option<CorvidApproverFn>,
    user_data: usize,
}

static APPROVER_REGISTRATION: Mutex<ApproverRegistration> = Mutex::new(ApproverRegistration {
    callback: None,
    user_data: 0,
});

pub(crate) enum ApprovalRequestOutcome {
    Accepted(ApprovalDecisionInfo),
    MissingOrRejected,
}

pub(crate) fn request_host_approval(request: &OwnedApprovalRequired) -> ApprovalRequestOutcome {
    let args = match serde_json::from_str::<serde_json::Value>(&request.args_json) {
        Ok(serde_json::Value::Array(values)) => values,
        _ => Vec::new(),
    };
    if let Ok(Some(detail)) = crate::approver_bridge::evaluate_registered_approver(
        &ApprovalSiteInput::fallback(&request.site_name),
        &args,
    ) {
        return if detail.accepted {
            ApprovalRequestOutcome::Accepted(detail)
        } else {
            LAST_APPROVAL_DETAIL.with(|slot| *slot.borrow_mut() = Some(detail));
            ApprovalRequestOutcome::MissingOrRejected
        };
    }
    let registration = APPROVER_REGISTRATION.lock().unwrap();
    let Some(callback) = registration.callback else {
        let detail = ApprovalDecisionInfo {
            accepted: false,
            decider: "fail-closed-default".to_string(),
            rationale: None,
        };
        LAST_APPROVAL_DETAIL.with(|slot| *slot.borrow_mut() = Some(detail));
        return ApprovalRequestOutcome::MissingOrRejected;
    };
    reset_transients();
    let c_request = owned_approval_to_c(request);
    let decision = unsafe { callback(&c_request, registration.user_data as *mut c_void) };
    let detail = ApprovalDecisionInfo {
        accepted: matches!(decision, CorvidApprovalDecision::Accept),
        decider: "c-callback".to_string(),
        rationale: None,
    };
    if detail.accepted {
        ApprovalRequestOutcome::Accepted(detail)
    } else {
        LAST_APPROVAL_DETAIL.with(|slot| *slot.borrow_mut() = Some(detail));
        ApprovalRequestOutcome::MissingOrRejected
    }
}

pub(crate) fn mark_preapproved_request(
    label: String,
    args: Vec<serde_json::Value>,
    detail: ApprovalDecisionInfo,
) {
    PREAPPROVED_REQUESTS.with(|queue| queue.borrow_mut().push_back((label, args, detail)));
}

pub(crate) fn decide_registered_approval(req: &ApprovalRequest) -> ApprovalDecision {
    let preapproved = PREAPPROVED_REQUESTS.with(|queue| {
        let mut queue = queue.borrow_mut();
        match queue.front() {
            Some((label, args, _)) if *label == req.label && *args == req.args => {
                queue.pop_front()
            }
            _ => None,
        }
    });
    if let Some((_, _, detail)) = preapproved {
        LAST_APPROVAL_DETAIL.with(|slot| *slot.borrow_mut() = Some(detail.clone()));
        return if detail.accepted {
            ApprovalDecision::Approve
        } else {
            ApprovalDecision::Deny
        };
    }
    let request = OwnedApprovalRequired {
        site_name: req.label.clone(),
        predicate_json: serde_json::json!({
            "kind": "approval_request",
            "label": req.label,
        })
        .to_string(),
        args_json: serde_json::Value::Array(req.args.clone()).to_string(),
        rationale_prompt: format!("Approval required for `{}`.", req.label),
    };
    match request_host_approval(&request) {
        ApprovalRequestOutcome::Accepted(detail) => {
            LAST_APPROVAL_DETAIL.with(|slot| *slot.borrow_mut() = Some(detail));
            ApprovalDecision::Approve
        }
        ApprovalRequestOutcome::MissingOrRejected => ApprovalDecision::Deny,
    }
}

pub(crate) fn take_last_approval_detail() -> Option<ApprovalDecisionInfo> {
    LAST_APPROVAL_DETAIL.with(|slot| slot.borrow_mut().take())
}

pub(crate) fn register_corvid_approver_source(
    source_path: &std::path::Path,
    max_budget_usd_per_call: f64,
) -> Result<(), crate::approver_bridge::ApproverLoadError> {
    crate::approver_bridge::register_approver_from_source(source_path, max_budget_usd_per_call)
}

pub(crate) fn clear_corvid_approver_source() {
    crate::approver_bridge::clear_registered_approver();
}

pub(crate) fn approval_predicate_json(site_name: &str) -> Option<String> {
    crate::catalog::catalog_approval_sites()
        .ok()?
        .into_iter()
        .find(|site| site.label == site_name)
        .and_then(|site| site.predicate)
        .map(|value| value.to_string())
}

pub(crate) fn evaluate_approval_predicate(
    site_name: &str,
    args_json: &str,
) -> crate::approver_bridge::CorvidPredicateResult {
    let Some(predicate_json) = approval_predicate_json(site_name) else {
        return crate::approver_bridge::CorvidPredicateResult {
            status: crate::approver_bridge::CorvidPredicateStatus::SiteNotFound,
            requires_approval: 0,
            bad_args_message: ptr::null(),
        };
    };
    let predicate: serde_json::Value = match serde_json::from_str(&predicate_json) {
        Ok(value) => value,
        Err(_) => {
            return crate::approver_bridge::CorvidPredicateResult {
                status: crate::approver_bridge::CorvidPredicateStatus::Unevaluable,
                requires_approval: 0,
                bad_args_message: ptr::null(),
            }
        }
    };
    let args = match serde_json::from_str::<serde_json::Value>(args_json) {
        Ok(serde_json::Value::Array(values)) => values,
        Ok(_) => {
            return crate::approver_bridge::CorvidPredicateResult {
                status: crate::approver_bridge::CorvidPredicateStatus::BadArgs,
                requires_approval: 0,
                bad_args_message: stash_transient("args_json must be a JSON array"),
            }
        }
        Err(err) => {
            return crate::approver_bridge::CorvidPredicateResult {
                status: crate::approver_bridge::CorvidPredicateStatus::BadArgs,
                requires_approval: 0,
                bad_args_message: stash_transient(&format!("args_json must be a JSON array: {err}")),
            }
        }
    };
    let Some(arity) = predicate.get("arity").and_then(|value| value.as_u64()) else {
        return crate::approver_bridge::CorvidPredicateResult {
            status: crate::approver_bridge::CorvidPredicateStatus::Unevaluable,
            requires_approval: 0,
            bad_args_message: ptr::null(),
        };
    };
    if args.len() != arity as usize {
        return crate::approver_bridge::CorvidPredicateResult {
            status: crate::approver_bridge::CorvidPredicateStatus::BadArgs,
            requires_approval: 0,
            bad_args_message: stash_transient(&format!(
                "arity mismatch for approval site `{site_name}`: expected {arity}, got {}",
                args.len()
            )),
        };
    }
    crate::approver_bridge::CorvidPredicateResult {
        status: crate::approver_bridge::CorvidPredicateStatus::Ok,
        requires_approval: 1,
        bad_args_message: ptr::null(),
    }
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
            .ok_or_else(|| RuntimeError::Other(format!("embedded descriptor length overflow: {json_len}")))?;
        let bytes = std::slice::from_raw_parts(ptr.cast::<u8>(), total_len);
        return parse_embedded_section_bytes(bytes)
            .map_err(|err| RuntimeError::Other(format!("parse embedded descriptor: {err}")));
    }

    #[cfg(windows)]
    {
        let path = current_library_path()?;
        return read_embedded_section_from_library(&path)
            .map_err(|err| RuntimeError::Other(format!("read embedded descriptor from `{}`: {err}", path.display())));
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
                let path = String::from_utf16(&buf)
                    .map_err(|err| RuntimeError::Other(format!("module path UTF-16 decode: {err}")))?;
                return Ok(PathBuf::from(path));
            }
            buf.resize(buf.len() * 2, 0);
        }
    }
}

pub(crate) fn build_scalar_invoker(
    symbol: &str,
    params: &[ScalarAbiType],
    ret: ScalarReturnType,
) -> Result<ScalarInvoker, RuntimeError> {
    unsafe {
        let address = resolve_current_library_symbol(symbol)? as usize;
        if address == 0 {
            return Err(RuntimeError::Other(format!("symbol `{symbol}` resolved to null")));
        }
        match params {
            [] => build_invoker0(symbol.to_string(), address, ret),
            [a0] => build_invoker1(symbol.to_string(), address, *a0, ret),
            [a0, a1] => build_invoker2(symbol.to_string(), address, *a0, *a1, ret),
            _ => Err(RuntimeError::Other(format!(
                "catalog host dispatch currently supports up to two scalar parameters; `{symbol}` has {}",
                params.len()
            ))),
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
            return Err(RuntimeError::Other(format!("resolve symbol `{symbol}`: not found")));
        };
        return Ok(ptr as *const c_void);
    }
}

unsafe fn build_invoker0(
    symbol: String,
    address: usize,
    ret: ScalarReturnType,
) -> Result<ScalarInvoker, RuntimeError> {
    Ok(Arc::new(move |args| {
        if !args.is_empty() {
            return Err(RuntimeError::Marshal(format!(
                "agent `{symbol}` expected 0 args, got {}",
                args.len()
            )));
        }
        invoke0(&symbol, address, ret)
    }))
}

unsafe fn build_invoker1(
    symbol: String,
    address: usize,
    a0: ScalarAbiType,
    ret: ScalarReturnType,
) -> Result<ScalarInvoker, RuntimeError> {
    Ok(Arc::new(move |args| {
        if args.len() != 1 {
            return Err(RuntimeError::Marshal(format!(
                "agent `{symbol}` expected 1 arg, got {}",
                args.len()
            )));
        }
        invoke1(&symbol, address, a0, ret, &args[0])
    }))
}

unsafe fn build_invoker2(
    symbol: String,
    address: usize,
    a0: ScalarAbiType,
    a1: ScalarAbiType,
    ret: ScalarReturnType,
) -> Result<ScalarInvoker, RuntimeError> {
    Ok(Arc::new(move |args| {
        if args.len() != 2 {
            return Err(RuntimeError::Marshal(format!(
                "agent `{symbol}` expected 2 args, got {}",
                args.len()
            )));
        }
        invoke2(&symbol, address, a0, a1, ret, &args[0], &args[1])
    }))
}

unsafe fn invoke0(
    symbol: &str,
    address: usize,
    ret: ScalarReturnType,
) -> Result<serde_json::Value, RuntimeError> {
    match ret {
        ScalarReturnType::Int => {
            let func: unsafe extern "C" fn() -> i64 = std::mem::transmute(address);
            Ok(serde_json::Value::from(func()))
        }
        ScalarReturnType::Float => {
            let func: unsafe extern "C" fn() -> f64 = std::mem::transmute(address);
            float_json(symbol, func())
        }
        ScalarReturnType::Bool => {
            let func: unsafe extern "C" fn() -> bool = std::mem::transmute(address);
            Ok(serde_json::Value::Bool(func()))
        }
        ScalarReturnType::String => {
            let func: unsafe extern "C" fn() -> *const c_char = std::mem::transmute(address);
            string_json(symbol, func())
        }
        ScalarReturnType::Nothing => {
            let func: unsafe extern "C" fn() = std::mem::transmute(address);
            func();
            Ok(serde_json::Value::Null)
        }
    }
}

unsafe fn invoke1(
    symbol: &str,
    address: usize,
    a0: ScalarAbiType,
    ret: ScalarReturnType,
    arg0: &serde_json::Value,
) -> Result<serde_json::Value, RuntimeError> {
    match a0 {
        ScalarAbiType::Int => invoke1_int(symbol, address, ret, parse_i64_arg(arg0, symbol, 0)?),
        ScalarAbiType::Float => invoke1_float(symbol, address, ret, parse_f64_arg(arg0, symbol, 0)?),
        ScalarAbiType::Bool => invoke1_bool(symbol, address, ret, parse_bool_arg(arg0, symbol, 0)?),
        ScalarAbiType::String => {
            let arg0 = parse_string_arg(arg0, symbol, 0)?;
            invoke1_string(symbol, address, ret, arg0.as_ptr())
        }
    }
}

unsafe fn invoke2(
    symbol: &str,
    address: usize,
    a0: ScalarAbiType,
    a1: ScalarAbiType,
    ret: ScalarReturnType,
    arg0: &serde_json::Value,
    arg1: &serde_json::Value,
) -> Result<serde_json::Value, RuntimeError> {
    match a0 {
        ScalarAbiType::Int => {
            let arg0 = parse_i64_arg(arg0, symbol, 0)?;
            invoke2_after_int(symbol, address, a1, ret, arg0, arg1)
        }
        ScalarAbiType::Float => {
            let arg0 = parse_f64_arg(arg0, symbol, 0)?;
            invoke2_after_float(symbol, address, a1, ret, arg0, arg1)
        }
        ScalarAbiType::Bool => {
            let arg0 = parse_bool_arg(arg0, symbol, 0)?;
            invoke2_after_bool(symbol, address, a1, ret, arg0, arg1)
        }
        ScalarAbiType::String => {
            let arg0 = parse_string_arg(arg0, symbol, 0)?;
            invoke2_after_string(symbol, address, a1, ret, arg0.as_ptr(), arg1)
        }
    }
}

macro_rules! impl_invoke1 {
    ($name:ident, $arg_ty:ty) => {
        unsafe fn $name(
            symbol: &str,
            address: usize,
            ret: ScalarReturnType,
            arg0: $arg_ty,
        ) -> Result<serde_json::Value, RuntimeError> {
            match ret {
                ScalarReturnType::Int => {
                    let func: unsafe extern "C" fn($arg_ty) -> i64 = std::mem::transmute(address);
                    Ok(serde_json::Value::from(func(arg0)))
                }
                ScalarReturnType::Float => {
                    let func: unsafe extern "C" fn($arg_ty) -> f64 = std::mem::transmute(address);
                    float_json(symbol, func(arg0))
                }
                ScalarReturnType::Bool => {
                    let func: unsafe extern "C" fn($arg_ty) -> bool = std::mem::transmute(address);
                    Ok(serde_json::Value::Bool(func(arg0)))
                }
                ScalarReturnType::String => {
                    let func: unsafe extern "C" fn($arg_ty) -> *const c_char = std::mem::transmute(address);
                    string_json(symbol, func(arg0))
                }
                ScalarReturnType::Nothing => {
                    let func: unsafe extern "C" fn($arg_ty) = std::mem::transmute(address);
                    func(arg0);
                    Ok(serde_json::Value::Null)
                }
            }
        }
    };
}

impl_invoke1!(invoke1_int, i64);
impl_invoke1!(invoke1_float, f64);
impl_invoke1!(invoke1_bool, bool);
impl_invoke1!(invoke1_string, *const c_char);

macro_rules! impl_invoke2_matrix {
    ($name:ident, $arg0_ty:ty, $arg1_ty:ty) => {
        unsafe fn $name(
            symbol: &str,
            address: usize,
            ret: ScalarReturnType,
            arg0: $arg0_ty,
            arg1: $arg1_ty,
        ) -> Result<serde_json::Value, RuntimeError> {
            match ret {
                ScalarReturnType::Int => {
                    let func: unsafe extern "C" fn($arg0_ty, $arg1_ty) -> i64 = std::mem::transmute(address);
                    Ok(serde_json::Value::from(func(arg0, arg1)))
                }
                ScalarReturnType::Float => {
                    let func: unsafe extern "C" fn($arg0_ty, $arg1_ty) -> f64 = std::mem::transmute(address);
                    float_json(symbol, func(arg0, arg1))
                }
                ScalarReturnType::Bool => {
                    let func: unsafe extern "C" fn($arg0_ty, $arg1_ty) -> bool = std::mem::transmute(address);
                    Ok(serde_json::Value::Bool(func(arg0, arg1)))
                }
                ScalarReturnType::String => {
                    let func: unsafe extern "C" fn($arg0_ty, $arg1_ty) -> *const c_char = std::mem::transmute(address);
                    string_json(symbol, func(arg0, arg1))
                }
                ScalarReturnType::Nothing => {
                    let func: unsafe extern "C" fn($arg0_ty, $arg1_ty) = std::mem::transmute(address);
                    func(arg0, arg1);
                    Ok(serde_json::Value::Null)
                }
            }
        }
    };
}

impl_invoke2_matrix!(invoke2_i64_i64, i64, i64);
impl_invoke2_matrix!(invoke2_i64_f64, i64, f64);
impl_invoke2_matrix!(invoke2_i64_bool, i64, bool);
impl_invoke2_matrix!(invoke2_i64_string, i64, *const c_char);
impl_invoke2_matrix!(invoke2_f64_i64, f64, i64);
impl_invoke2_matrix!(invoke2_f64_f64, f64, f64);
impl_invoke2_matrix!(invoke2_f64_bool, f64, bool);
impl_invoke2_matrix!(invoke2_f64_string, f64, *const c_char);
impl_invoke2_matrix!(invoke2_bool_i64, bool, i64);
impl_invoke2_matrix!(invoke2_bool_f64, bool, f64);
impl_invoke2_matrix!(invoke2_bool_bool, bool, bool);
impl_invoke2_matrix!(invoke2_bool_string, bool, *const c_char);
impl_invoke2_matrix!(invoke2_string_i64, *const c_char, i64);
impl_invoke2_matrix!(invoke2_string_f64, *const c_char, f64);
impl_invoke2_matrix!(invoke2_string_bool, *const c_char, bool);
impl_invoke2_matrix!(invoke2_string_string, *const c_char, *const c_char);

unsafe fn invoke2_after_int(
    symbol: &str,
    address: usize,
    a1: ScalarAbiType,
    ret: ScalarReturnType,
    arg0: i64,
    arg1: &serde_json::Value,
) -> Result<serde_json::Value, RuntimeError> {
    match a1 {
        ScalarAbiType::Int => invoke2_i64_i64(symbol, address, ret, arg0, parse_i64_arg(arg1, symbol, 1)?),
        ScalarAbiType::Float => invoke2_i64_f64(symbol, address, ret, arg0, parse_f64_arg(arg1, symbol, 1)?),
        ScalarAbiType::Bool => invoke2_i64_bool(symbol, address, ret, arg0, parse_bool_arg(arg1, symbol, 1)?),
        ScalarAbiType::String => {
            let arg1 = parse_string_arg(arg1, symbol, 1)?;
            invoke2_i64_string(symbol, address, ret, arg0, arg1.as_ptr())
        }
    }
}

unsafe fn invoke2_after_float(
    symbol: &str,
    address: usize,
    a1: ScalarAbiType,
    ret: ScalarReturnType,
    arg0: f64,
    arg1: &serde_json::Value,
) -> Result<serde_json::Value, RuntimeError> {
    match a1 {
        ScalarAbiType::Int => invoke2_f64_i64(symbol, address, ret, arg0, parse_i64_arg(arg1, symbol, 1)?),
        ScalarAbiType::Float => invoke2_f64_f64(symbol, address, ret, arg0, parse_f64_arg(arg1, symbol, 1)?),
        ScalarAbiType::Bool => invoke2_f64_bool(symbol, address, ret, arg0, parse_bool_arg(arg1, symbol, 1)?),
        ScalarAbiType::String => {
            let arg1 = parse_string_arg(arg1, symbol, 1)?;
            invoke2_f64_string(symbol, address, ret, arg0, arg1.as_ptr())
        }
    }
}

unsafe fn invoke2_after_bool(
    symbol: &str,
    address: usize,
    a1: ScalarAbiType,
    ret: ScalarReturnType,
    arg0: bool,
    arg1: &serde_json::Value,
) -> Result<serde_json::Value, RuntimeError> {
    match a1 {
        ScalarAbiType::Int => invoke2_bool_i64(symbol, address, ret, arg0, parse_i64_arg(arg1, symbol, 1)?),
        ScalarAbiType::Float => invoke2_bool_f64(symbol, address, ret, arg0, parse_f64_arg(arg1, symbol, 1)?),
        ScalarAbiType::Bool => invoke2_bool_bool(symbol, address, ret, arg0, parse_bool_arg(arg1, symbol, 1)?),
        ScalarAbiType::String => {
            let arg1 = parse_string_arg(arg1, symbol, 1)?;
            invoke2_bool_string(symbol, address, ret, arg0, arg1.as_ptr())
        }
    }
}

unsafe fn invoke2_after_string(
    symbol: &str,
    address: usize,
    a1: ScalarAbiType,
    ret: ScalarReturnType,
    arg0: *const c_char,
    arg1: &serde_json::Value,
) -> Result<serde_json::Value, RuntimeError> {
    match a1 {
        ScalarAbiType::Int => invoke2_string_i64(symbol, address, ret, arg0, parse_i64_arg(arg1, symbol, 1)?),
        ScalarAbiType::Float => invoke2_string_f64(symbol, address, ret, arg0, parse_f64_arg(arg1, symbol, 1)?),
        ScalarAbiType::Bool => invoke2_string_bool(symbol, address, ret, arg0, parse_bool_arg(arg1, symbol, 1)?),
        ScalarAbiType::String => {
            let arg1 = parse_string_arg(arg1, symbol, 1)?;
            invoke2_string_string(symbol, address, ret, arg0, arg1.as_ptr())
        }
    }
}

fn float_json(symbol: &str, value: f64) -> Result<serde_json::Value, RuntimeError> {
    let Some(number) = serde_json::Number::from_f64(value) else {
        return Err(RuntimeError::Marshal(format!(
            "agent `{symbol}` returned non-finite Float {value}"
        )));
    };
    Ok(serde_json::Value::Number(number))
}

unsafe fn string_json(symbol: &str, value: *const c_char) -> Result<serde_json::Value, RuntimeError> {
    if value.is_null() {
        return Err(RuntimeError::Marshal(format!(
            "agent `{symbol}` returned null String pointer"
        )));
    }
    let text = CStr::from_ptr(value)
        .to_str()
        .map_err(|err| RuntimeError::Marshal(format!(
            "agent `{symbol}` returned non-UTF8 String: {err}"
        )))?
        .to_owned();
    crate::ffi_bridge::corvid_free_string(value);
    Ok(serde_json::Value::String(text))
}

fn parse_i64_arg(value: &serde_json::Value, symbol: &str, index: usize) -> Result<i64, RuntimeError> {
    value.as_i64().ok_or_else(|| {
        RuntimeError::Marshal(format!(
            "agent `{symbol}` argument {} expected Int",
            index + 1
        ))
    })
}

fn parse_f64_arg(value: &serde_json::Value, symbol: &str, index: usize) -> Result<f64, RuntimeError> {
    value.as_f64().ok_or_else(|| {
        RuntimeError::Marshal(format!(
            "agent `{symbol}` argument {} expected Float",
            index + 1
        ))
    })
}

fn parse_bool_arg(value: &serde_json::Value, symbol: &str, index: usize) -> Result<bool, RuntimeError> {
    value.as_bool().ok_or_else(|| {
        RuntimeError::Marshal(format!(
            "agent `{symbol}` argument {} expected Bool",
            index + 1
        ))
    })
}

fn parse_string_arg(value: &serde_json::Value, symbol: &str, index: usize) -> Result<CString, RuntimeError> {
    let text = value.as_str().ok_or_else(|| {
        RuntimeError::Marshal(format!(
            "agent `{symbol}` argument {} expected String",
            index + 1
        ))
    })?;
    CString::new(text)
        .map_err(|err| RuntimeError::Marshal(format!("agent `{symbol}` argument {} contained NUL: {err}", index + 1)))
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

fn owned_approval_to_c(value: &OwnedApprovalRequired) -> CorvidApprovalRequired {
    CorvidApprovalRequired {
        site_name: stash_transient(&value.site_name),
        predicate_json: stash_transient(&value.predicate_json),
        args_json: stash_transient(&value.args_json),
        rationale_prompt: stash_transient(&value.rationale_prompt),
    }
}

fn owned_preflight_to_c(value: &OwnedPreFlight) -> CorvidPreFlight {
    CorvidPreFlight {
        status: value.status,
        cost_bound_usd: value.cost_bound_usd,
        requires_approval: value.requires_approval as u8,
        effect_row_json: stash_transient(&value.effect_row_json),
        grounded_source_set_json: stash_transient(&value.grounded_source_set_json),
        bad_args_message: value
            .bad_args_message
            .as_deref()
            .map(stash_transient)
            .unwrap_or(ptr::null()),
    }
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

fn record_host_event(name: &str, payload: serde_json::Value) -> CorvidHostEventStatus {
    let _ = crate::ffi_bridge::corvid_runtime_embed_init_default();
    let runtime = bridge().corvid_runtime();
    let tracer = runtime.tracer();
    if !tracer.is_enabled() {
        return CorvidHostEventStatus::TraceDisabled;
    }
    tracer.emit(TraceEvent::HostEvent {
        ts_ms: crate::tracing::now_ms(),
        run_id: tracer.run_id().to_string(),
        name: name.to_string(),
        payload,
    });
    CorvidHostEventStatus::Ok
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
    if cfg!(debug_assertions)
        && handle != observation_handles::NULL_OBSERVATION_HANDLE
        && !released
    {
        eprintln!("warning: observation handle {handle} was already released or never existed");
    }
}

#[no_mangle]
pub unsafe extern "C" fn corvid_record_host_event(
    name: *const c_char,
    payload_json: *const c_char,
    payload_len: usize,
) -> CorvidHostEventStatus {
    let Ok(name) = read_c_string(name) else {
        return CorvidHostEventStatus::RuntimeError;
    };
    let _ = crate::ffi_bridge::corvid_runtime_embed_init_default();
    let runtime = bridge().corvid_runtime();
    if !runtime.tracer().is_enabled() {
        return CorvidHostEventStatus::TraceDisabled;
    }
    if payload_json.is_null() {
        return CorvidHostEventStatus::BadJson;
    }
    let bytes = std::slice::from_raw_parts(payload_json as *const u8, payload_len);
    let payload_text = String::from_utf8_lossy(bytes).into_owned();
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(&payload_text) else {
        return CorvidHostEventStatus::BadJson;
    };
    record_host_event(&name, payload)
}

#[no_mangle]
pub unsafe extern "C" fn corvid_grounded_attest_int(
    value: i64,
    source_name: CorvidString,
    confidence: f64,
) -> i64 {
    let source = read_corvid_string(source_name);
    let chain = crate::provenance::ProvenanceChain::with_retrieval(&source, crate::tracing::now_ms());
    grounded_handles::set_last_scalar_attestation(grounded_handles::make_attestation(chain, confidence));
    value
}

#[no_mangle]
pub unsafe extern "C" fn corvid_grounded_attest_float(
    value: f64,
    source_name: CorvidString,
    confidence: f64,
) -> f64 {
    let source = read_corvid_string(source_name);
    let chain = crate::provenance::ProvenanceChain::with_retrieval(&source, crate::tracing::now_ms());
    grounded_handles::set_last_scalar_attestation(grounded_handles::make_attestation(chain, confidence));
    value
}

#[no_mangle]
pub unsafe extern "C" fn corvid_grounded_attest_bool(
    value: bool,
    source_name: CorvidString,
    confidence: f64,
) -> bool {
    let source = read_corvid_string(source_name);
    let chain = crate::provenance::ProvenanceChain::with_retrieval(&source, crate::tracing::now_ms());
    grounded_handles::set_last_scalar_attestation(grounded_handles::make_attestation(chain, confidence));
    value
}

#[no_mangle]
pub unsafe extern "C" fn corvid_grounded_attest_string(
    value: CorvidString,
    source_name: CorvidString,
    confidence: f64,
) -> CorvidString {
    let source = read_corvid_string(source_name);
    let chain = crate::provenance::ProvenanceChain::with_retrieval(&source, crate::tracing::now_ms());
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
pub unsafe extern "C" fn corvid_list_agents(
    out: *mut CorvidAgentHandle,
    capacity: usize,
) -> usize {
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

#[no_mangle]
pub extern "C" fn corvid_register_approver(
    fn_ptr: Option<CorvidApproverFn>,
    user_data: *mut c_void,
) {
    let mut registration = APPROVER_REGISTRATION.lock().unwrap();
    registration.callback = fn_ptr;
    registration.user_data = user_data as usize;
}

#[no_mangle]
pub unsafe extern "C" fn corvid_register_approver_from_source(
    source_path: *const c_char,
    max_budget_usd_per_call: f64,
    out_error_message: *mut *mut c_char,
) -> crate::approver_bridge::CorvidApproverLoadStatus {
    if !out_error_message.is_null() {
        *out_error_message = ptr::null_mut();
    }
    let Ok(source_path) = read_c_string(source_path) else {
        return crate::approver_bridge::CorvidApproverLoadStatus::IoError;
    };
    match register_corvid_approver_source(std::path::Path::new(&source_path), max_budget_usd_per_call) {
        Ok(()) => crate::approver_bridge::CorvidApproverLoadStatus::Ok,
        Err(err) => {
            if !out_error_message.is_null() {
                if let Ok(message) = CString::new(err.message) {
                    *out_error_message = message.into_raw();
                }
            }
            err.status
        }
    }
}

#[no_mangle]
pub extern "C" fn corvid_clear_approver() {
    clear_corvid_approver_source();
}

#[no_mangle]
pub unsafe extern "C" fn corvid_approval_predicate_json(
    site_name: *const c_char,
    out_len: *mut usize,
) -> *const c_char {
    if !out_len.is_null() {
        *out_len = 0;
    }
    let Ok(site_name) = read_c_string(site_name) else {
        return ptr::null();
    };
    reset_transients();
    let Some(json) = approval_predicate_json(&site_name) else {
        return ptr::null();
    };
    let ptr = stash_transient(&json);
    if !out_len.is_null() {
        *out_len = json.len();
    }
    ptr
}

#[no_mangle]
pub unsafe extern "C" fn corvid_evaluate_approval_predicate(
    site_name: *const c_char,
    args_json: *const c_char,
    args_len: usize,
) -> crate::approver_bridge::CorvidPredicateResult {
    let Ok(site_name) = read_c_string(site_name) else {
        return crate::approver_bridge::CorvidPredicateResult {
            status: crate::approver_bridge::CorvidPredicateStatus::BadArgs,
            requires_approval: 0,
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
    evaluate_approval_predicate(&site_name, &args_json)
}
