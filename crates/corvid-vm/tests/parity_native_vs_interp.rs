//! Cross-tier cycle parity over the strongest surface current Corvid can
//! express honestly: synthetic heap graphs.
//!
//! Current Corvid syntax still has no field mutation, so neither tier can
//! construct a refcount cycle from source text alone. The native C runtime's
//! own 17d tests therefore build cycles via FFI; the VM side does the same
//! here through `StructValue::set_field`.

use corvid_resolve::DefId;
use corvid_vm::{collect_cycles, StructValue, Value};
use std::sync::{Mutex, OnceLock};

fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().expect("test lock poisoned")
}

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

    static corvid_release_count: i64;
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
            if let Some(marker) = marker {
                marker(ptr, ctx);
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

static CELL_TYPEINFO: CorvidTypeinfo = CorvidTypeinfo {
    size: 16,
    flags: 0x01,
    destroy_fn: Some(cell_destroy),
    trace_fn: Some(cell_trace),
    weak_fn: None,
    elem_typeinfo: std::ptr::null(),
    name: c"Cell".as_ptr() as *const u8,
};

fn vm_release_delta_for_two_block_cycle() -> usize {
    let a = StructValue::new(DefId(1), "Node", []);
    let b = StructValue::new(DefId(1), "Node", []);
    let weak_a = Value::Struct(a.clone()).downgrade().expect("weak a");
    let weak_b = Value::Struct(b.clone()).downgrade().expect("weak b");

    a.set_field("next", Value::Struct(b.clone()));
    b.set_field("next", Value::Struct(a.clone()));
    drop(a);
    drop(b);

    assert!(weak_a.upgrade().is_some());
    assert!(weak_b.upgrade().is_some());
    collect_cycles()
}

fn vm_release_delta_for_three_block_cycle() -> usize {
    let a = StructValue::new(DefId(1), "Node", []);
    let b = StructValue::new(DefId(1), "Node", []);
    let c = StructValue::new(DefId(1), "Node", []);
    let weak_a = Value::Struct(a.clone()).downgrade().expect("weak a");
    let weak_b = Value::Struct(b.clone()).downgrade().expect("weak b");
    let weak_c = Value::Struct(c.clone()).downgrade().expect("weak c");

    a.set_field("next", Value::Struct(b.clone()));
    b.set_field("next", Value::Struct(c.clone()));
    c.set_field("next", Value::Struct(a.clone()));
    drop(a);
    drop(b);
    drop(c);

    assert!(weak_a.upgrade().is_some());
    assert!(weak_b.upgrade().is_some());
    assert!(weak_c.upgrade().is_some());
    collect_cycles()
}

unsafe fn native_release_delta_for_cycle(blocks: usize) -> i64 {
    let start = corvid_release_count;
    let mut nodes = Vec::with_capacity(blocks);
    for _ in 0..blocks {
        let node = corvid_alloc_typed(16, &CELL_TYPEINFO);
        let slots = node as *mut *mut u8;
        slots.write(std::ptr::null_mut());
        slots.add(1).write(std::ptr::null_mut());
        nodes.push(node);
    }
    for i in 0..blocks {
        let next = nodes[(i + 1) % blocks];
        (nodes[i] as *mut *mut u8).write(next);
    }
    let empty: &mut [*mut u8] = &mut [];
    corvid_gc_from_roots(empty.as_mut_ptr(), 0);
    corvid_release_count - start
}

#[test]
fn two_block_cycle_has_same_reclamation_cardinality() {
    let _guard = test_lock();
    let vm_collected = vm_release_delta_for_two_block_cycle();
    let native_collected = unsafe { native_release_delta_for_cycle(2) };
    assert_eq!(vm_collected as i64, native_collected);
    assert_eq!(native_collected, 2);
}

#[test]
fn three_block_cycle_has_same_reclamation_cardinality() {
    let _guard = test_lock();
    let vm_collected = vm_release_delta_for_three_block_cycle();
    let native_collected = unsafe { native_release_delta_for_cycle(3) };
    assert_eq!(vm_collected as i64, native_collected);
    assert_eq!(native_collected, 3);
}

#[test]
fn acyclic_fast_path_matches_no_cycle_work() {
    let _guard = test_lock();
    let node = StructValue::new(DefId(1), "Node", []);
    let weak = Value::Struct(node.clone()).downgrade().expect("weak");
    drop(node);
    assert!(weak.upgrade().is_none());
    assert_eq!(collect_cycles(), 0);
}
