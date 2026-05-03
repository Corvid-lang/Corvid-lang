//! Approval host bridge and C approval exports.

use crate::approvals::{ApprovalDecision, ApprovalRequest};
use crate::approver_bridge::{ApprovalDecisionInfo, ApprovalSiteInput};
use crate::catalog::{
    CorvidApprovalDecision, CorvidApprovalRequired, CorvidApproverFn, CorvidPreFlight,
    OwnedApprovalRequired, OwnedPreFlight,
};
use crate::ffi_bridge::bridge;
use corvid_trace_schema::TraceEvent;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::{c_char, c_void, CString};
use std::ptr;
use std::sync::Mutex;

thread_local! {
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
    super::reset_transients();
    let c_request = owned_approval_to_c(request);
    let decision = unsafe { callback(&c_request, registration.user_data as *mut c_void) };
    let detail = ApprovalDecisionInfo {
        accepted: decision == CorvidApprovalDecision::Accept as i32,
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
            Some((label, args, _)) if *label == req.label && *args == req.args => queue.pop_front(),
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
                bad_args_message: super::stash_transient("args_json must be a JSON array"),
            }
        }
        Err(err) => {
            return crate::approver_bridge::CorvidPredicateResult {
                status: crate::approver_bridge::CorvidPredicateStatus::BadArgs,
                requires_approval: 0,
                bad_args_message: super::stash_transient(&format!(
                    "args_json must be a JSON array: {err}"
                )),
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
            bad_args_message: super::stash_transient(&format!(
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

pub(super) fn owned_approval_to_c(value: &OwnedApprovalRequired) -> CorvidApprovalRequired {
    CorvidApprovalRequired {
        site_name: super::stash_transient(&value.site_name),
        predicate_json: super::stash_transient(&value.predicate_json),
        args_json: super::stash_transient(&value.args_json),
        rationale_prompt: super::stash_transient(&value.rationale_prompt),
    }
}

pub(super) fn owned_preflight_to_c(value: &OwnedPreFlight) -> CorvidPreFlight {
    CorvidPreFlight {
        status: value.status,
        cost_bound_usd: value.cost_bound_usd,
        requires_approval: value.requires_approval as u8,
        effect_row_json: super::stash_transient(&value.effect_row_json),
        grounded_source_set_json: super::stash_transient(&value.grounded_source_set_json),
        bad_args_message: value
            .bad_args_message
            .as_deref()
            .map(super::stash_transient)
            .unwrap_or(ptr::null()),
    }
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
pub unsafe extern "C" fn corvid_record_host_event(
    name: *const c_char,
    payload_json: *const c_char,
    payload_len: usize,
) -> CorvidHostEventStatus {
    let Ok(name) = super::read_c_string(name) else {
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
pub extern "C" fn corvid_register_approver(
    fn_ptr: Option<CorvidApproverFn>,
    user_data: *mut c_void,
) {
    let mut registration = APPROVER_REGISTRATION.lock().unwrap();
    registration.callback = fn_ptr;
    registration.user_data = user_data as usize;
    clear_corvid_approver_source();
}

#[no_mangle]
pub unsafe extern "C" fn corvid_mark_preapproved_request(
    site_name: *const c_char,
    args_json: *const c_char,
    args_len: usize,
) -> bool {
    let Ok(site_name) = super::read_c_string(site_name) else {
        return false;
    };
    if args_json.is_null() {
        return false;
    }
    let bytes = std::slice::from_raw_parts(args_json as *const u8, args_len);
    let args_json = String::from_utf8_lossy(bytes).into_owned();
    let Ok(serde_json::Value::Array(args)) = serde_json::from_str::<serde_json::Value>(&args_json)
    else {
        return false;
    };
    mark_preapproved_request(
        site_name,
        args,
        ApprovalDecisionInfo {
            accepted: true,
            decider: "host-binding-local-approver".to_string(),
            rationale: None,
        },
    );
    true
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
    let Ok(source_path) = super::read_c_string(source_path) else {
        return crate::approver_bridge::CorvidApproverLoadStatus::IoError;
    };
    match register_corvid_approver_source(
        std::path::Path::new(&source_path),
        max_budget_usd_per_call,
    ) {
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
    let mut registration = APPROVER_REGISTRATION.lock().unwrap();
    registration.callback = None;
    registration.user_data = 0;
    drop(registration);
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
    let Ok(site_name) = super::read_c_string(site_name) else {
        return ptr::null();
    };
    super::reset_transients();
    let Some(json) = approval_predicate_json(&site_name) else {
        return ptr::null();
    };
    let ptr = super::stash_transient(&json);
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
    let Ok(site_name) = super::read_c_string(site_name) else {
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
    super::reset_transients();
    evaluate_approval_predicate(&site_name, &args_json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CALLBACK_CALLS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn counting_approver(
        _request: *const CorvidApprovalRequired,
        _user_data: *mut c_void,
    ) -> i32 {
        CALLBACK_CALLS.fetch_add(1, Ordering::SeqCst);
        CorvidApprovalDecision::Accept as i32
    }

    #[test]
    fn request_host_approval_callback_can_run_nontrivial_host_code() {
        CALLBACK_CALLS.store(0, Ordering::SeqCst);
        corvid_register_approver(Some(counting_approver), std::ptr::null_mut());
        let request = OwnedApprovalRequired {
            site_name: "EchoString".to_string(),
            predicate_json: "{\"kind\":\"approval_contract\"}".to_string(),
            args_json: "[\"vip\"]".to_string(),
            rationale_prompt: "Approval required".to_string(),
        };
        let outcome = request_host_approval(&request);
        corvid_clear_approver();
        match outcome {
            ApprovalRequestOutcome::Accepted(_) => {}
            ApprovalRequestOutcome::MissingOrRejected => {
                panic!("host callback unexpectedly rejected request")
            }
        }
        assert_eq!(CALLBACK_CALLS.load(Ordering::SeqCst), 1);
    }
}
