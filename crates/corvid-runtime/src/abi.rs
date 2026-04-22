//! Typed C-ABI wrapper types shared between Cranelift codegen and the
//! `#[tool]` proc-macro.
//!
//! Tool calls use direct typed exports: each `#[tool]` declaration
//! becomes a `#[no_mangle] pub extern "C" fn` whose parameters and
//! return use the types in this module, and the Cranelift codegen
//! emits a direct call to the symbol with matching typed values.
//!
//! # Why typed ABI, not JSON
//!
//! See dev-log Day 31 for the full argument. Short version: both sides
//! know the schemas at compile time, both sides are ours, no LLM tokens
//! are counted on this boundary — JSON's compactness + universality
//! advantages don't apply, and its costs (heap alloc per call, UTF-8
//! parsing, type erasure, opacity to the optimizer) all do. The typed
//! ABI is what Rust FFI already uses idiomatically; we're just picking
//! it over the lazy default.
//!
//! # What types are supported today
//!
//! | Corvid type | ABI type         | Conversion        |
//! |-------------|------------------|-------------------|
//! | `Int`       | `i64`            | identity          |
//! | `Bool`      | `bool`           | identity          |
//! | `Float`     | `f64`            | identity          |
//! | `String`    | `CorvidString`   | refcount-aware    |
//!
//! `Struct` and `List` at the tool ABI are not implemented yet.
//!
//! # Safety rationale
//!
//! Like `ffi_bridge`, this module opts out of the crate-level
//! `deny(unsafe_code)`. Reading bytes out of a `CorvidString` requires
//! pointer dereferences; FFI on `corvid_release` / `corvid_string_from_bytes`
//! is the only way to talk to the C runtime that allocates Corvid
//! Strings. Every `unsafe` block has a SAFETY comment naming the
//! caller contract.

#![allow(unsafe_code)]

use crate::ffi_bridge::{
    corvid_free_string as _corvid_free_string_marker,
    corvid_runtime_embed_init_default as _corvid_runtime_embed_init_default_marker,
    corvid_string_into_cstr as _corvid_string_into_cstr_marker,
    tokio_handle as _tokio_handle_marker,
}; // keep the bridge module referenced
use crate::catalog_c_api::{
    corvid_abi_descriptor_hash as _corvid_abi_descriptor_hash_marker,
    corvid_abi_descriptor_json as _corvid_abi_descriptor_json_marker,
    corvid_abi_verify as _corvid_abi_verify_marker,
    corvid_agent_signature_json as _corvid_agent_signature_json_marker,
    corvid_approval_predicate_json as _corvid_approval_predicate_json_marker,
    corvid_call_agent as _corvid_call_agent_marker,
    corvid_clear_approver as _corvid_clear_approver_marker,
    corvid_evaluate_approval_predicate as _corvid_evaluate_approval_predicate_marker,
    corvid_find_agents_where as _corvid_find_agents_where_marker,
    corvid_free_result as _corvid_free_result_marker,
    corvid_list_agents as _corvid_list_agents_marker,
    corvid_pre_flight as _corvid_pre_flight_marker,
    corvid_register_approver as _corvid_register_approver_marker,
    corvid_register_approver_from_source as _corvid_register_approver_from_source_marker,
}; // keep the catalog C-API surface referenced for cdylib/staticlib linking
use std::marker::PhantomData;
use std::sync::atomic::{AtomicI64, Ordering};

// Suppress the "imported but unused" warning — the import above
// documents that `abi` module depends on the ffi_bridge module's
// public surface existing.
#[allow(dead_code)]
fn _depends_on_ffi_bridge() {
    let _ = _tokio_handle_marker;
    let _ = _corvid_runtime_embed_init_default_marker as extern "C" fn() -> i32;
    let _ =
        _corvid_string_into_cstr_marker as unsafe extern "C" fn(CorvidString) -> *mut std::ffi::c_char;
    let _ = _corvid_free_string_marker as unsafe extern "C" fn(*const std::ffi::c_char);
    let _ = _corvid_abi_descriptor_json_marker as unsafe extern "C" fn(*mut usize) -> *const std::ffi::c_char;
    let _ = _corvid_abi_descriptor_hash_marker as extern "C" fn(*mut u8);
    let _ = _corvid_abi_verify_marker as extern "C" fn(*const u8) -> i32;
    let _ = _corvid_list_agents_marker
        as unsafe extern "C" fn(*mut crate::catalog::CorvidAgentHandle, usize) -> usize;
    let _ = _corvid_find_agents_where_marker
        as unsafe extern "C" fn(
            *const std::ffi::c_char,
            usize,
            *mut usize,
            usize,
        ) -> crate::catalog::CorvidFindAgentsResult;
    let _ = _corvid_agent_signature_json_marker
        as unsafe extern "C" fn(*const std::ffi::c_char, *mut usize) -> *const std::ffi::c_char;
    let _ = _corvid_pre_flight_marker
        as unsafe extern "C" fn(
            *const std::ffi::c_char,
            *const std::ffi::c_char,
            usize,
        ) -> crate::catalog::CorvidPreFlight;
    let _ = _corvid_call_agent_marker
        as unsafe extern "C" fn(
            *const std::ffi::c_char,
            *const std::ffi::c_char,
            usize,
            *mut *mut std::ffi::c_char,
            *mut usize,
            *mut crate::catalog::CorvidApprovalRequired,
        ) -> crate::catalog::CorvidCallStatus;
    let _ = _corvid_free_result_marker as unsafe extern "C" fn(*mut std::ffi::c_char);
    let _ = _corvid_register_approver_marker
        as extern "C" fn(
            Option<crate::catalog::CorvidApproverFn>,
            *mut std::ffi::c_void,
        );
    let _ = _corvid_register_approver_from_source_marker
        as unsafe extern "C" fn(
            *const std::ffi::c_char,
            f64,
            *mut *mut std::ffi::c_char,
        ) -> crate::approver_bridge::CorvidApproverLoadStatus;
    let _ = _corvid_clear_approver_marker as extern "C" fn();
    let _ = _corvid_approval_predicate_json_marker
        as unsafe extern "C" fn(*const std::ffi::c_char, *mut usize) -> *const std::ffi::c_char;
    let _ = _corvid_evaluate_approval_predicate_marker
        as unsafe extern "C" fn(
            *const std::ffi::c_char,
            *const std::ffi::c_char,
            usize,
        ) -> crate::approver_bridge::CorvidPredicateResult;
}

// ------------------------------------------------------------
// Conversion traits. Implemented for each ABI↔native pair.
// The macro calls `FromCorvidAbi::from_corvid_abi(abi_value)` on
// parameters and `IntoCorvidAbi::into_corvid_abi(native)` on returns.
// ------------------------------------------------------------

/// Convert from the C-ABI representation into the Rust-native
/// representation. Implemented by the ABI type (e.g. `CorvidString`).
pub trait FromCorvidAbi<Abi> {
    fn from_corvid_abi(abi: Abi) -> Self;
}

/// Convert a Rust-native value into its C-ABI representation.
/// Implemented by the Rust-native type (e.g. `String`).
pub trait IntoCorvidAbi<Abi> {
    fn into_corvid_abi(self) -> Abi;
}

// Identity scalar conversions — i64, f64, bool.
impl FromCorvidAbi<i64> for i64 {
    #[inline(always)]
    fn from_corvid_abi(abi: i64) -> Self {
        abi
    }
}
impl IntoCorvidAbi<i64> for i64 {
    #[inline(always)]
    fn into_corvid_abi(self) -> i64 {
        self
    }
}
impl FromCorvidAbi<f64> for f64 {
    #[inline(always)]
    fn from_corvid_abi(abi: f64) -> Self {
        abi
    }
}
impl IntoCorvidAbi<f64> for f64 {
    #[inline(always)]
    fn into_corvid_abi(self) -> f64 {
        self
    }
}
impl FromCorvidAbi<bool> for bool {
    #[inline(always)]
    fn from_corvid_abi(abi: bool) -> Self {
        abi
    }
}
impl IntoCorvidAbi<bool> for bool {
    #[inline(always)]
    fn into_corvid_abi(self) -> bool {
        self
    }
}

// ------------------------------------------------------------
// CorvidString — the ABI representation of a Corvid `String`.
// ------------------------------------------------------------
//
// A Corvid String has this shape in memory:
//
//   offset -16: refcount    (i64)   — via descriptor - 16
//   offset  -8: reserved    (i64)
//   offset   0: bytes_ptr   (*const u8)   <-- descriptor points here
//   offset   8: length      (i64)
//   offset  16: bytes...    (inline, for heap strings; .rodata for literals)
//
// The value an agent holds is a pointer to offset 0 (the descriptor).
// `CorvidString` wraps that pointer — `#[repr(transparent)]` so at the
// C-ABI boundary it's exactly `*const u8` width.

/// Descriptor layout the codegen emits. Same field order + sizes as
/// the refcounted-heap allocation; `#[repr(C)]` guarantees stable
/// layout across compiler versions.
#[repr(C)]
struct CorvidStringDescriptor {
    bytes_ptr: *const u8,
    length: i64,
}

/// Corvid `String` at the C-ABI boundary.
///
/// `#[repr(transparent)]` so the ABI width and passing convention
/// match `*const CorvidStringDescriptor` — which matches what Cranelift
/// passes for `String` values (a pointer to a 16-byte descriptor).
#[repr(transparent)]
#[derive(Debug, Clone, Copy)]
pub struct CorvidString {
    descriptor: *const CorvidStringDescriptor,
    // PhantomData<&()> ensures the type stays `Send + Sync`-checkable
    // while keeping `#[repr(transparent)]` — `*const` is Send/Sync
    // already, this is belt-and-braces for clarity.
    _marker: PhantomData<()>,
}

// SAFETY: CorvidString is a pointer to refcounted memory. Crossing a
// tokio task boundary is safe today because Corvid program code runs
// single-threaded (tools dispatch through the tokio runtime but the
// Corvid values themselves are owned by one Rust task at a time). The
// payload bytes are immutable after construction; Strings are shared-
// immutable. Future multi-agent work will revisit this with a proper
// multi-threaded refcount design (biased RC / per-arena locks); the
// Send/Sync impls here are scoped to the current single-threaded model.
unsafe impl Send for CorvidString {}
unsafe impl Sync for CorvidString {}

impl CorvidString {
    /// Read the bytes backing this string as a slice. Valid for as
    /// long as the caller holds a reference that prevents the refcount
    /// from dropping to zero.
    ///
    /// # Safety
    ///
    /// `descriptor` must point at a valid Corvid String descriptor —
    /// which it does by construction when a compiled Corvid binary
    /// passes a String value across the ABI. Callers outside the
    /// FFI bridge should not construct `CorvidString` directly.
    unsafe fn as_bytes(&self) -> &[u8] {
        if self.descriptor.is_null() {
            return &[];
        }
        // SAFETY: Contract — descriptor points at a CorvidStringDescriptor
        // laid out per the Corvid ABI spec. Reading bytes_ptr + length
        // observes the immutable payload.
        unsafe {
            let desc = &*self.descriptor;
            if desc.bytes_ptr.is_null() || desc.length <= 0 {
                &[]
            } else {
                std::slice::from_raw_parts(desc.bytes_ptr, desc.length as usize)
            }
        }
    }

    /// Borrow this Corvid string as UTF-8 without allocating.
    ///
    /// # Safety
    ///
    /// Same as [`CorvidString::as_bytes`]: `descriptor` must point at a
    /// valid Corvid string descriptor and the caller must ensure the
    /// underlying string stays alive for the lifetime of the borrow.
    pub(crate) unsafe fn as_str(&self) -> &str {
        let bytes = unsafe { self.as_bytes() };
        match std::str::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => panic!("CorvidString contained invalid UTF-8"),
        }
    }
}

// ------------------------------------------------------------
// CorvidString ↔ String conversions.
//
// Receive-side (from_corvid_abi): wrapper received a CorvidString from
// Cranelift. Per Corvid's +0 ABI ("caller passes without
// bumping; callee retains on entry"), we need to retain here to own
// the reference. On conversion to a native `String` we copy the bytes
// (so the native String owns its own memory) and release our retained
// reference — leaving no net refcount change across the wrapper.
//
// Return-side (into_corvid_abi): wrapper returns a `String` to
// Cranelift. Allocate a fresh refcounted Corvid String via the
// runtime's `corvid_string_from_bytes` helper. Caller receives it with
// refcount 1 (matching the +0 convention — they'll "retain on entry"
// which is the +1 we gave them).
// ------------------------------------------------------------

impl FromCorvidAbi<CorvidString> for String {
    fn from_corvid_abi(abi: CorvidString) -> Self {
        // SAFETY: The only valid sources of CorvidString values are
        // (a) Cranelift code calling into a `#[tool]` wrapper, (b)
        // runtime helpers constructing descriptors. Both respect the
        // descriptor layout. See `as_bytes` for the pointer-validity
        // contract.
        //
        // Tool-call ABI is BORROW-ONLY on arguments: the Cranelift
        // caller keeps ownership of the refcount and passes us a +0
        // borrow, and we neither retain nor release. That means we
        // can copy bytes out safely while `abi` stays alive
        // (Cranelift guarantees it until the call returns), but must
        // NOT touch the refcount. If we released here, the caller's
        // subsequent end-of-scope release would double-free. Agent
        // calls use a different +0-with-retain-on-entry convention
        //; tool calls deliberately diverge for FFI
        // simplicity.
        let bytes = unsafe { abi.as_bytes() };
        match std::str::from_utf8(bytes) {
            Ok(s) => s.to_owned(),
            Err(_) => {
                // Corvid String bytes are declared UTF-8 by construction
                // (the runtime only constructs them from valid UTF-8
                // sources). Panic rather than return replacement chars —
                // silent corruption is worse than a loud abort when the
                // invariant breaks.
                panic!("CorvidString contained invalid UTF-8");
            }
        }
    }
}

impl IntoCorvidAbi<CorvidString> for String {
    fn into_corvid_abi(self) -> CorvidString {
        // Allocate a refcount-1 Corvid String from the bytes. Caller
        // takes ownership of that +1 per the return-value ABI.
        crate::ffi_bridge::string_from_rust(self)
    }
}

// Also allow `&str` returns, which desugar to the String impl via
// `to_string`. Costs one allocation, matches idiomatic Rust.
impl IntoCorvidAbi<CorvidString> for &str {
    fn into_corvid_abi(self) -> CorvidString {
        self.to_owned().into_corvid_abi()
    }
}

// ------------------------------------------------------------
// Tool metadata — what the `#[tool]` macro registers via `inventory`.
// `corvid_runtime_init` reads these at startup to build the effect-
// policy table and the tracer's tool-name registry.
// ------------------------------------------------------------

/// Metadata about a `#[tool]` function. One entry per attribute
/// invocation, collected at link time via the `inventory` crate.
///
/// The actual dispatch is not through this struct — codegen emits
/// direct calls by symbol name. This struct exists only for runtime
/// observability + policy lookup.
#[derive(Debug, Clone, Copy)]
pub struct ToolMetadata {
    /// Corvid-source name the tool is registered under.
    pub name: &'static str,
    /// Linker-visible symbol of the `#[no_mangle]` wrapper fn the
    /// macro emits. Codegen looks up by this symbol when lowering
    /// `IrCallKind::Tool`.
    pub symbol: &'static str,
    /// Number of parameters. Cross-checked against the Corvid
    /// declaration at runtime-init to catch macro-vs-source drift.
    pub arity: usize,
}

// Alloc-aware counter of registered tools — used by diagnostics to
// surface "N tools linked in" during `corvid doctor`. Not on the
// dispatch path. AtomicI64 because it's written once per registration
// and read during diagnostic output.
pub(crate) static REGISTERED_TOOL_COUNT: AtomicI64 = AtomicI64::new(0);

/// Fetch the count of registered tools. Returns 0 until
/// `corvid_runtime_init` has walked the inventory.
pub fn registered_tool_count() -> i64 {
    REGISTERED_TOOL_COUNT.load(Ordering::Relaxed)
}

// Declare `ToolMetadata` as an inventory collection type. Every
// `#[tool]` macro invocation expands to an `inventory::submit!` block
// whose value ends up in this collection at link time. Readers call
// `inventory::iter::<ToolMetadata>()` (wrapped by `iter_registered_tools`
// in `ffi_bridge`) during `corvid_runtime_init`.
inventory::collect!(ToolMetadata);
