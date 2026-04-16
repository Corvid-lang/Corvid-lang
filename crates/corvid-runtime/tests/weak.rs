use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

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

unsafe impl Sync for CorvidTypeinfo {}

#[link(name = "corvid_c_runtime", kind = "static")]
extern "C" {
    fn corvid_alloc_typed(payload_bytes: i64, typeinfo: *const CorvidTypeinfo) -> *mut u8;
    fn corvid_release(payload: *mut u8);
    fn corvid_gc_from_roots(roots: *mut *mut u8, n_roots: usize);

    fn corvid_weak_new(strong_payload: *mut u8) -> *mut u8;
    fn corvid_weak_upgrade(weak_payload: *mut u8) -> *mut u8;
    fn corvid_weak_clear_self(strong_payload: *mut u8);
}

unsafe extern "C" fn cell_trace(
    payload: *mut u8,
    marker: Option<unsafe extern "C" fn(*mut u8, *mut u8)>,
    ctx: *mut u8,
) {
    let slots = payload as *mut *mut u8;
    for i in 0..2 {
        let ptr = *slots.add(i);
        if !ptr.is_null() {
            if let Some(mark) = marker {
                mark(ptr, ctx);
            }
        }
    }
}

unsafe extern "C" fn cell_destroy(payload: *mut u8) {
    let slots = payload as *mut *mut u8;
    for i in 0..2 {
        let ptr = *slots.add(i);
        if !ptr.is_null() {
            corvid_release(ptr);
        }
    }
}

static BOX_TYPEINFO: CorvidTypeinfo = CorvidTypeinfo {
    size: 8,
    flags: 0,
    destroy_fn: None,
    trace_fn: None,
    weak_fn: Some(corvid_weak_clear_self),
    elem_typeinfo: ptr::null(),
    name: ptr::null(),
};

static CELL_TYPEINFO: CorvidTypeinfo = CorvidTypeinfo {
    size: 16,
    flags: 0x01,
    destroy_fn: Some(cell_destroy),
    trace_fn: Some(cell_trace),
    weak_fn: Some(corvid_weak_clear_self),
    elem_typeinfo: ptr::null(),
    name: ptr::null(),
};

static REENTRANT_SAW_NONE: AtomicBool = AtomicBool::new(false);
static mut REENTRANT_WEAK: *mut u8 = ptr::null_mut();

unsafe extern "C" fn reentrant_destroy(_payload: *mut u8) {
    let upgraded = corvid_weak_upgrade(REENTRANT_WEAK);
    if upgraded.is_null() {
        REENTRANT_SAW_NONE.store(true, Ordering::SeqCst);
    } else {
        corvid_release(upgraded);
    }
}

static REENTRANT_TYPEINFO: CorvidTypeinfo = CorvidTypeinfo {
    size: 8,
    flags: 0,
    destroy_fn: Some(reentrant_destroy),
    trace_fn: None,
    weak_fn: Some(corvid_weak_clear_self),
    elem_typeinfo: ptr::null(),
    name: ptr::null(),
};

#[test]
fn weak_upgrade_succeeds_while_strong_is_alive() {
    unsafe {
        let strong = corvid_alloc_typed(8, &BOX_TYPEINFO);
        let weak = corvid_weak_new(strong);

        let upgraded = corvid_weak_upgrade(weak);
        assert_eq!(upgraded, strong, "upgrade should recover the live target");

        corvid_release(upgraded);
        corvid_release(weak);
        corvid_release(strong);
    }
}

#[test]
fn weak_upgrade_returns_null_after_strong_drop() {
    unsafe {
        let strong = corvid_alloc_typed(8, &BOX_TYPEINFO);
        let weak = corvid_weak_new(strong);

        corvid_release(strong);

        let upgraded = corvid_weak_upgrade(weak);
        assert!(
            upgraded.is_null(),
            "weak should be cleared once the strong refcount reaches zero"
        );

        corvid_release(weak);
    }
}

#[test]
fn weak_is_cleared_before_destroy_fn_reenters_upgrade() {
    unsafe {
        REENTRANT_SAW_NONE.store(false, Ordering::SeqCst);
        let strong = corvid_alloc_typed(8, &REENTRANT_TYPEINFO);
        let weak = corvid_weak_new(strong);
        REENTRANT_WEAK = weak;

        corvid_release(strong);

        assert!(
            REENTRANT_SAW_NONE.load(Ordering::SeqCst),
            "destroy_fn should observe the weak as already cleared"
        );

        corvid_release(weak);
        REENTRANT_WEAK = ptr::null_mut();
    }
}

#[test]
fn cycle_collector_sweep_clears_weak_slots() {
    unsafe {
        let a = corvid_alloc_typed(16, &CELL_TYPEINFO);
        let b = corvid_alloc_typed(16, &CELL_TYPEINFO);
        (a as *mut *mut u8).write(b);
        (a as *mut *mut u8).add(1).write(ptr::null_mut());
        (b as *mut *mut u8).write(a);
        (b as *mut *mut u8).add(1).write(ptr::null_mut());

        let weak = corvid_weak_new(a);
        let mut roots = [weak];
        corvid_gc_from_roots(roots.as_mut_ptr(), roots.len());

        let upgraded = corvid_weak_upgrade(weak);
        assert!(
            upgraded.is_null(),
            "cycle sweep should clear weak slots before freeing the strong blocks"
        );

        corvid_release(weak);
    }
}
