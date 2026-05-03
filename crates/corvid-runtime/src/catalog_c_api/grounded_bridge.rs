//! Grounded and observation C bridge exports.

use crate::abi::CorvidString;
use crate::catalog::CorvidCallStatus;
use crate::ffi_bridge::read_corvid_string;
use crate::grounded_handles;
use crate::observation_handles;
use std::cell::RefCell;
use std::ffi::{c_char, CStr, CString};
use std::ptr;

thread_local! {
    static TRANSIENT_STRINGS: RefCell<Vec<CString>> = RefCell::new(Vec::new());
}

pub(super) fn reset_transients() {
    TRANSIENT_STRINGS.with(|arena| arena.borrow_mut().clear());
}

pub(super) fn stash_transient(text: &str) -> *const c_char {
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

pub(super) unsafe fn read_c_string(ptr: *const c_char) -> Result<String, CorvidCallStatus> {
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
