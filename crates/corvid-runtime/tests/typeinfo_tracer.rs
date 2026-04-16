//! Phase 17a — typeinfo-driven alloc/release/trace path.
//!
//! These tests exercise the C runtime directly: they construct Corvid-
//! layout heap objects, install custom typeinfo blocks, and invoke
//! `corvid_alloc_typed` / `corvid_release` / `corvid_trace_list` via
//! FFI. The goal is to pin the new typeinfo machinery with tests that
//! don't depend on full codegen — if the codegen breaks but the C
//! contract is preserved, these still pass; conversely, if the C
//! contract drifts these catch it without needing a full Corvid
//! program to compile and run.
//!
//! What's covered:
//!
//!   1. `corvid_typeinfo_String` is the built-in symbol, callable,
//!      empty trace body, NULL destroy/elem, and a non-NULL weak
//!      clear hook.
//!   2. `corvid_alloc_typed` + `corvid_release` round-trip invokes
//!      the typeinfo's destroy_fn exactly once when rc→0.
//!   3. `corvid_trace_list` walks refcounted-element lists, calling
//!      the marker once per non-NULL element, passing the ctx through.
//!   4. `corvid_trace_list` no-ops for primitive-element lists (NULL
//!      `elem_typeinfo`) — prevents the `List<Int>` mis-trace bug.
//!   5. Refcount bit-packing: retain/release on an allocation with
//!      bit 61 (mark) or bit 62 (color) set externally preserves
//!      those bits. Guards 17d's collector against retain/release
//!      stomping GC state.
//!   6. `corvid_release` on a refcount=1 allocation dispatches the
//!      destructor exactly once; on rc>1 does not.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Layout mirror of the C `corvid_typeinfo` struct. `repr(C)` so the
/// in-memory layout matches what alloc.c expects.
#[repr(C)]
struct CorvidTypeinfo {
    size: u32,
    flags: u32,
    destroy_fn: Option<unsafe extern "C" fn(*mut u8)>,
    trace_fn: Option<
        unsafe extern "C" fn(
            *mut u8,
            Option<unsafe extern "C" fn(*mut u8, *mut u8)>,
            *mut u8,
        ),
    >,
    weak_fn: Option<unsafe extern "C" fn(*mut u8)>,
    elem_typeinfo: *const CorvidTypeinfo,
    name: *const u8,
}

// SAFETY: CorvidTypeinfo instances used in tests are declared `static`
// with function pointers that have no global mutable state. Sharing
// across threads (none of these tests actually do) would be safe.
unsafe impl Sync for CorvidTypeinfo {}

#[link(name = "corvid_c_runtime", kind = "static")]
extern "C" {
    fn corvid_alloc_typed(payload_bytes: i64, typeinfo: *const CorvidTypeinfo) -> *mut u8;
    fn corvid_retain(payload: *mut u8);
    fn corvid_release(payload: *mut u8);
    fn corvid_trace_list(
        payload: *mut u8,
        marker: Option<unsafe extern "C" fn(*mut u8, *mut u8)>,
        ctx: *mut u8,
    );

    static corvid_typeinfo_String: CorvidTypeinfo;
}

/// (1) Built-in String typeinfo.
#[test]
fn string_typeinfo_has_expected_shape() {
    // SAFETY: `corvid_typeinfo_String` is a `const` symbol in alloc.c's
    // `.rodata`. Reading its fields is sound as long as the layout
    // matches (see `CorvidTypeinfo` comment).
    unsafe {
        let ti = &corvid_typeinfo_String;
        assert_eq!(ti.size, 0, "String is variable-length, size=0");
        assert_eq!(ti.flags, 0, "String is a leaf, no flags set");
        assert!(ti.destroy_fn.is_none(), "String has no refcounted children");
        assert!(ti.trace_fn.is_some(), "String trace_fn is always emitted");
        assert!(
            ti.weak_fn.is_some(),
            "String weak_fn clears weak slots before the payload is freed"
        );
        assert!(ti.elem_typeinfo.is_null(), "String is not a list");
    }
}

static DESTROYED_COUNT: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn counting_destroy(_payload: *mut u8) {
    DESTROYED_COUNT.fetch_add(1, Ordering::SeqCst);
}

unsafe extern "C" fn empty_trace(
    _payload: *mut u8,
    _marker: Option<unsafe extern "C" fn(*mut u8, *mut u8)>,
    _ctx: *mut u8,
) {
}

static TEST_TI_WITH_DESTROY: CorvidTypeinfo = CorvidTypeinfo {
    size: 16,
    flags: 0,
    destroy_fn: Some(counting_destroy),
    trace_fn: Some(empty_trace),
    weak_fn: None,
    elem_typeinfo: std::ptr::null(),
    name: std::ptr::null(),
};

/// (2) alloc + release dispatches destroy_fn.
#[test]
fn alloc_typed_then_release_runs_destructor() {
    DESTROYED_COUNT.store(0, Ordering::SeqCst);
    unsafe {
        let p = corvid_alloc_typed(16, &TEST_TI_WITH_DESTROY);
        assert!(!p.is_null());
        corvid_release(p);
    }
    assert_eq!(
        DESTROYED_COUNT.load(Ordering::SeqCst),
        1,
        "destroy_fn must fire exactly once when rc→0"
    );
}

/// (6) rc>1 does NOT dispatch destructor until final release.
#[test]
fn retain_defers_destruction_until_final_release() {
    DESTROYED_COUNT.store(0, Ordering::SeqCst);
    unsafe {
        let p = corvid_alloc_typed(16, &TEST_TI_WITH_DESTROY);
        corvid_retain(p);
        corvid_retain(p); // rc = 3
        corvid_release(p); // rc = 2
        assert_eq!(DESTROYED_COUNT.load(Ordering::SeqCst), 0);
        corvid_release(p); // rc = 1
        assert_eq!(DESTROYED_COUNT.load(Ordering::SeqCst), 0);
        corvid_release(p); // rc = 0 → destroy
    }
    assert_eq!(DESTROYED_COUNT.load(Ordering::SeqCst), 1);
}

/// (3,4) `corvid_trace_list` behavior on primitive vs refcounted lists.
///
/// A Corvid list payload: `[length (i64)][elem_0 (i64)][elem_1 (i64)]...`
/// For this test we allocate the payload manually via `corvid_alloc_typed`
/// with a list typeinfo we construct here.

static MARKED_POINTERS: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());

unsafe extern "C" fn counting_marker(obj: *mut u8, ctx: *mut u8) {
    let ctx_val = ctx as usize;
    let mut v = MARKED_POINTERS.lock().unwrap();
    // Encode obj-pointer XOR ctx so the test can assert ctx was threaded
    // through.
    v.push((obj as usize) ^ ctx_val);
}

/// Minimal trace_fn that matches the runtime's `corvid_trace_list`
/// shape — needed for the list typeinfo blocks we construct in tests.
/// Delegates to `corvid_trace_list` itself.
static LIST_PRIMITIVE_TI: CorvidTypeinfo = CorvidTypeinfo {
    size: 0,
    flags: 0x04, // IS_LIST
    destroy_fn: None,
    trace_fn: Some(corvid_trace_list_raw),
    weak_fn: None,
    elem_typeinfo: std::ptr::null(), // primitive elements
    name: std::ptr::null(),
};

static LIST_REFCOUNTED_TI: CorvidTypeinfo = CorvidTypeinfo {
    size: 0,
    flags: 0x04, // IS_LIST
    destroy_fn: None, // tests don't exercise the destructor path here
    trace_fn: Some(corvid_trace_list_raw),
    weak_fn: None,
    elem_typeinfo: unsafe { &corvid_typeinfo_String as *const _ },
    name: std::ptr::null(),
};

// Wrapper so the function pointer's static item form matches.
unsafe extern "C" fn corvid_trace_list_raw(
    payload: *mut u8,
    marker: Option<unsafe extern "C" fn(*mut u8, *mut u8)>,
    ctx: *mut u8,
) {
    corvid_trace_list(payload, marker, ctx);
}

#[test]
fn trace_list_primitive_elements_no_ops() {
    MARKED_POINTERS.lock().unwrap().clear();
    unsafe {
        // length=3, three garbage i64s that would be Ints.
        let p = corvid_alloc_typed(8 + 3 * 8, &LIST_PRIMITIVE_TI);
        let words = p as *mut i64;
        *words = 3;
        *words.add(1) = 0x123456789ABCDEF0;
        *words.add(2) = -1;
        *words.add(3) = 42;

        corvid_trace_list(p, Some(counting_marker), std::ptr::null_mut());

        corvid_release(p);
    }
    assert_eq!(
        MARKED_POINTERS.lock().unwrap().len(),
        0,
        "primitive-element list must not invoke marker — prevents List<Int> mis-trace"
    );
}

#[test]
fn trace_list_refcounted_elements_invokes_marker() {
    MARKED_POINTERS.lock().unwrap().clear();
    unsafe {
        // length=2; pretend pointers (we're not releasing them, just
        // testing that the tracer dispatches).
        let p = corvid_alloc_typed(8 + 2 * 8, &LIST_REFCOUNTED_TI);
        let words = p as *mut i64;
        *words = 2;
        let fake_a = 0xDEAD_BEEF_0000_0010u64 as i64;
        let fake_b = 0xCAFE_F00D_0000_0020u64 as i64;
        *words.add(1) = fake_a;
        *words.add(2) = fake_b;

        let ctx = 0x55u64 as *mut u8;
        corvid_trace_list(p, Some(counting_marker), ctx);

        // release skips destroy (destroy_fn is None on our test ti)
        // so the fake pointers never get dereferenced.
        corvid_release(p);
    }
    let marked = MARKED_POINTERS.lock().unwrap();
    assert_eq!(marked.len(), 2, "one marker call per element");
    // ctx was XOR'd into each recorded value; unXOR to recover elem ptrs.
    let ctx_val = 0x55usize;
    let a = marked[0] ^ ctx_val;
    let b = marked[1] ^ ctx_val;
    assert_eq!(a, 0xDEAD_BEEF_0000_0010);
    assert_eq!(b, 0xCAFE_F00D_0000_0020);
}

/// (5) Refcount bit-packing: retain/release preserve bits 61-62
/// when they're set externally (as 17d's mark/color bits will be).
/// Without this, the collector would lose its state whenever user
/// code did a retain or release.
#[test]
fn retain_release_preserves_high_bits() {
    DESTROYED_COUNT.store(0, Ordering::SeqCst);
    unsafe {
        let p = corvid_alloc_typed(16, &TEST_TI_WITH_DESTROY);
        // Reach into the header and set bit 61 (mark bit). The header
        // sits 16 bytes before the payload.
        let header_rc = (p as *mut i64).offset(-2);
        let before = *header_rc;
        assert_eq!(before & 0x1FFF_FFFF_FFFF_FFFF, 1, "rc starts at 1");
        *header_rc |= 1i64 << 61; // set mark bit

        // Retain → refcount goes 1→2, mark bit must survive.
        corvid_retain(p);
        let after_retain = *header_rc;
        assert_eq!(
            after_retain & 0x1FFF_FFFF_FFFF_FFFF, 2,
            "retain increments only the refcount portion"
        );
        assert_ne!(after_retain & (1i64 << 61), 0, "mark bit preserved after retain");

        // Release → refcount goes 2→1, no destruction yet.
        corvid_release(p);
        let after_release = *header_rc;
        assert_eq!(after_release & 0x1FFF_FFFF_FFFF_FFFF, 1);
        assert_ne!(after_release & (1i64 << 61), 0, "mark bit preserved after release");
        assert_eq!(DESTROYED_COUNT.load(Ordering::SeqCst), 0);

        // Final release — destructor fires.
        corvid_release(p);
    }
    assert_eq!(DESTROYED_COUNT.load(Ordering::SeqCst), 1);
}
