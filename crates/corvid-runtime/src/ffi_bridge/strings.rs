//! `CorvidString` factories, descriptor reads, and the C-runtime
//! refcount calls bridge code uses to round-trip Rust strings
//! across the ABI.
//!
//! `string_from_rust` / `string_from_static_str` mint Corvid
//! Strings whose bytes are visible to compiled code as
//! refcounted descriptors. `read_corvid_string` and
//! `borrow_corvid_string` pull text back out for Rust-side
//! consumption. `release_string` retires a `+1` refcount, and
//! the two `corvid_*` extern entry points let compiled code
//! convert a Corvid String into a NUL-terminated C string and
//! free the result later.
//!
//! The underlying allocators (`corvid_string_from_bytes`,
//! `corvid_string_from_static_bytes`, `corvid_release`) live in
//! `runtime/strings.c`; the extern block here declares them.

use std::ffi::{c_char, CString};

use crate::abi::CorvidString;

/// Convert a Corvid-owned `String` descriptor into a NUL-terminated
/// C string.
///
/// Ownership transfer:
/// - input `value` is consumed and released
/// - returned pointer is heap-owned by Corvid and must be freed with
///   `corvid_free_string`
#[no_mangle]
pub unsafe extern "C" fn corvid_string_into_cstr(value: CorvidString) -> *mut c_char {
    let text = unsafe { read_corvid_string(value) };
    unsafe { release_string(value) };
    CString::new(text)
        .expect("Corvid string contained interior NUL; `extern \"c\"` string returns must be NUL-free")
        .into_raw()
}

#[no_mangle]
pub unsafe extern "C" fn corvid_free_string(value: *const c_char) {
    if value.is_null() {
        return;
    }
    // SAFETY: `value` must have come from `corvid_string_into_cstr`.
    unsafe {
        let _ = CString::from_raw(value as *mut c_char);
    }
}

extern "C" {
    /// Allocate a heap Corvid String from `bytes` + `length`.
    /// Implemented in C (`runtime/strings.c`). Returns a descriptor
    /// pointer with refcount 1.
    fn corvid_string_from_bytes(bytes: *const u8, length: i64) -> *const u8;

    /// Allocate an immortal Corvid String from `bytes` + `length`.
    /// Used for repeated runtime fixture values where per-use release
    /// work is pure overhead.
    fn corvid_string_from_static_bytes(bytes: *const u8, length: i64) -> *const u8;

    /// Decrement a Corvid String's refcount, freeing when it hits 0.
    /// The refcount sentinel `i64::MIN` short-circuits for immortal
    /// `.rodata` literals.
    fn corvid_release(descriptor: *const u8);
}

/// Allocate a Corvid String from a Rust `String`. The returned
/// `CorvidString` has refcount 1 — caller takes ownership.
pub fn string_from_rust(s: String) -> CorvidString {
    let bytes = s.as_bytes();
    let ptr = bytes.as_ptr();
    let len = bytes.len() as i64;
    // SAFETY: `corvid_string_from_bytes` reads `len` bytes starting
    // at `ptr`. We hold `s` alive for the duration of the call, so
    // the bytes are valid. The returned descriptor owns its own
    // allocation — caller is free to drop `s` after this returns.
    let descriptor = unsafe { corvid_string_from_bytes(ptr, len) };
    // SAFETY: `CorvidString` is `#[repr(transparent)]` over a
    // descriptor pointer; transmuting from a raw pointer of the same
    // layout is sound.
    unsafe { std::mem::transmute(descriptor) }
}

/// Allocate an immortal Corvid String from a borrowed Rust string.
/// The returned value can be copied and released arbitrarily; the
/// immortal refcount sentinel makes release a no-op.
pub fn string_from_static_str(s: &str) -> CorvidString {
    let bytes = s.as_bytes();
    let ptr = bytes.as_ptr();
    let len = bytes.len() as i64;
    let descriptor = unsafe { corvid_string_from_static_bytes(ptr, len) };
    unsafe { std::mem::transmute(descriptor) }
}

/// Release a `CorvidString`'s refcount. Used by the `FromCorvidAbi`
/// impl on `String` after copying bytes out — paired with the implicit
/// retain the caller's `+0 ABI` contract performed on entry.
///
/// # Safety
///
/// `cs` must come from valid codegen- or runtime-emitted source
/// (i.e. the caller followed the Corvid ABI when passing the value).
pub unsafe fn release_string(cs: CorvidString) {
    // SAFETY: Transmuting a `#[repr(transparent)]` wrapper back to its
    // single field is sound. `corvid_release` expects a descriptor
    // pointer (the type alias for "CorvidString at the ABI") and
    // tolerates null by short-circuiting.
    unsafe {
        let descriptor: *const u8 = std::mem::transmute(cs);
        corvid_release(descriptor);
    }
}

/// Read a `CorvidString` as an owned Rust `String`.
pub(crate) unsafe fn read_corvid_string(cs: CorvidString) -> String {
    use crate::abi::FromCorvidAbi;
    String::from_corvid_abi(cs)
}

/// Borrow a `CorvidString` as UTF-8 for the duration of the call.
pub(crate) unsafe fn borrow_corvid_string<'a>(cs: &'a CorvidString) -> &'a str {
    unsafe { cs.as_str() }
}
