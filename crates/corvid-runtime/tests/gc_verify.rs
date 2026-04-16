//! Ã¢â‚¬â€ refcount verifier + trigger-log tests.
//!
//! The verifier runs during GC mark walk, computes expected refcount
//! from reachability, diffs against actual refcount. These tests
//! cover:
//!
//! 1. Clean graph Ã¢â€ â€™ zero drift reported.
//! 2. Deliberately corrupted refcount Ã¢â€ â€™ drift is detected, blame
//! PCs are non-null, drift count increments.
//! 3. Trigger-log records every GC call with alloc_count snapshot.
//!
//! The verifier is mode-gated via `corvid_gc_verify_mode` (normally
//! set by `corvid_init` from CORVID_GC_VERIFY; here we flip it by
//! direct FFI write so the test is self-contained and doesn't depend
//! on env-var propagation from the test harness). use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock}; #[repr(C)]
struct CorvidTypeinfo { size: u32, flags: u32, destroy_fn: Option<unsafe extern "C" fn(*mut u8)>, trace_fn: Option< unsafe extern "C" fn(*mut u8, Option<unsafe extern "C" fn(*mut u8, *mut u8)>, *mut u8,), >, weak_fn: Option<unsafe extern "C" fn(*mut u8)>, elem_typeinfo: *const CorvidTypeinfo, name: *const u8,
} unsafe impl Sync for CorvidTypeinfo {} #[link(name = "corvid_c_runtime", kind = "static")]
extern "C" { fn corvid_alloc_typed(payload_bytes: i64, typeinfo: *const CorvidTypeinfo) -> *mut u8; fn corvid_release(payload: *mut u8); fn corvid_gc_from_roots(roots: *mut *mut u8, n_roots: usize); fn corvid_verify_corrupt_rc(payload: *mut u8, delta: i64); fn corvid_pool_cached_blocks_for_size(payload_bytes: i64) -> i64; fn corvid_pool_cached_cap_for_size(payload_bytes: i64) -> i64; fn corvid_gc_trigger_log_length -> i64; fn corvid_gc_trigger_log_at(index: i64, out_alloc: *mut i64, out_safepoint: *mut i64, out_cycle: *mut i64,) -> i32; static mut corvid_gc_verify_mode: i32; static corvid_gc_verify_drift_count: i64;
} unsafe extern "C" fn cell_trace(payload: *mut u8, marker: Option<unsafe extern "C" fn(*mut u8, *mut u8)>, ctx: *mut u8,) { let slots = payload as *mut *mut u8; for i in 0..2 { let ptr = *slots.add(i); if !ptr.is_null { if let Some(m) = marker { m(ptr, ctx); } } }
} static CELL_DESTROY_COUNT: AtomicUsize = AtomicUsize::new(0); unsafe extern "C" fn cell_destroy(payload: *mut u8) { CELL_DESTROY_COUNT.fetch_add(1, Ordering::SeqCst); let slots = payload as *mut *mut u8; for i in 0..2 { let ptr = *slots.add(i); if !ptr.is_null { corvid_release(ptr); } }
} static CELL_TYPEINFO: CorvidTypeinfo = CorvidTypeinfo { size: 16, flags: 0x01, destroy_fn: Some(cell_destroy), trace_fn: Some(cell_trace), weak_fn: None, elem_typeinfo: std::ptr::null, name: c"Cell".as_ptr as *const u8,
}; static BOX24_TYPEINFO: CorvidTypeinfo = CorvidTypeinfo { size: 24, flags: 0, destroy_fn: None, trace_fn: None, weak_fn: None, elem_typeinfo: std::ptr::null, name: c"Box24".as_ptr as *const u8,
}; fn test_lock -> std::sync::MutexGuard<'static, > { static LOCK: OnceLock<Mutex<>> = OnceLock::new; LOCK.get_or_init(|| Mutex::new()).lock.expect("test lock poisoned")
} /// Clean graph Ã¢â‚¬â€ refcount invariant holds Ã¢â‚¬â€ verifier reports zero drift.
#[test]
fn verifier_clean_graph_no_drift { let _guard = test_lock; let start_drift: i64; unsafe { corvid_gc_verify_mode = 1; // warn mode start_drift = corvid_gc_verify_drift_count; } unsafe { // Root-held acyclic: root Ã¢â€ â€™ a Ã¢â€ â€™ b, refcount(a)=1 from root, // refcount(b)=1 from a. Verifier should see: a expected=1, // b expected=1. No drift. let a = corvid_alloc_typed(16, &CELL_TYPEINFO); let b = corvid_alloc_typed(16, &CELL_TYPEINFO); (a as *mut *mut u8).write(b); (a as *mut *mut u8).add(1).write(std::ptr::null_mut); (b as *mut *mut u8).write(std::ptr::null_mut); (b as *mut *mut u8).add(1).write(std::ptr::null_mut); let mut roots: [*mut u8; 1] = [a]; corvid_gc_from_roots(roots.as_mut_ptr, 1); assert_eq!(corvid_gc_verify_drift_count - start_drift, 0, "clean graph must report zero drift"); corvid_release(a); corvid_gc_verify_mode = 0; }
} /// Corrupt a block's refcount, run GC, expect verifier to fire.
#[test]
fn verifier_catches_injected_drift { let _guard = test_lock; let start_drift: i64; unsafe { corvid_gc_verify_mode = 1; // warn mode Ã¢â‚¬â€ don't abort the test start_drift = corvid_gc_verify_drift_count; } unsafe { let a = corvid_alloc_typed(16, &CELL_TYPEINFO); (a as *mut *mut u8).write(std::ptr::null_mut); (a as *mut *mut u8).add(1).write(std::ptr::null_mut); // Expected refcount under the root is 1. Inject +2 Ã¢â€ â€™ actual // becomes 3, over-count drift. corvid_verify_corrupt_rc(a, 2); let mut roots: [*mut u8; 1] = [a]; corvid_gc_from_roots(roots.as_mut_ptr, 1); assert!(corvid_gc_verify_drift_count - start_drift >= 1, "verifier must detect injected over-count drift"); // Restore so the block frees cleanly (release checks for // over-release and we don't want the test to abort there). corvid_verify_corrupt_rc(a, -2); corvid_release(a); corvid_gc_verify_mode = 0; }
} /// Every corvid_gc / corvid_gc_from_roots call appends one record
/// to the trigger log.
#[test]
fn trigger_log_grows_per_cycle { let _guard = test_lock; unsafe { let start_len = corvid_gc_trigger_log_length; let empty: &mut [*mut u8] = &mut []; corvid_gc_from_roots(empty.as_mut_ptr, 0); corvid_gc_from_roots(empty.as_mut_ptr, 0); let end_len = corvid_gc_trigger_log_length; assert_eq!(end_len - start_len, 2, "two GC calls must append two trigger records"); // Read back the newest record; alloc_count should be >= 0 and // cycle_index should be strictly monotonic. let mut alloc_a: i64 = 0; let mut sp_a: i64 = 0; let mut cyc_a: i64 = 0; let ok_a = corvid_gc_trigger_log_at(end_len - 2, &mut alloc_a, &mut sp_a, &mut cyc_a,); let mut alloc_b: i64 = 0; let mut sp_b: i64 = 0; let mut cyc_b: i64 = 0; let ok_b = corvid_gc_trigger_log_at(end_len - 1, &mut alloc_b, &mut sp_b, &mut cyc_b,); assert_eq!(ok_a, 1, "log accessor must succeed for valid index"); assert_eq!(ok_b, 1, "log accessor must succeed for valid index"); assert!(cyc_b > cyc_a, "cycle_index must be strictly monotonic (got {} then {})", cyc_a, cyc_b); assert!(alloc_a >= 0 && alloc_b >= 0); assert!(sp_a >= 0 && sp_b >= 0); // Out-of-range index returns 0. let ok_oob = corvid_gc_trigger_log_at(end_len + 100, std::ptr::null_mut, std::ptr::null_mut, std::ptr::null_mut,); assert_eq!(ok_oob, 0, "out-of-range index must report failure"); }
} #[test]
fn verifier_recycled_block_starts_from_clean_scratch_state { let _guard = test_lock; let start_drift: i64; unsafe { corvid_gc_verify_mode = 1; start_drift = corvid_gc_verify_drift_count; let a = corvid_alloc_typed(16, &CELL_TYPEINFO); (a as *mut *mut u8).write(std::ptr::null_mut); (a as *mut *mut u8).add(1).write(std::ptr::null_mut); let mut roots_a: [*mut u8; 1] = [a]; corvid_gc_from_roots(roots_a.as_mut_ptr, 1); corvid_release(a); let b = corvid_alloc_typed(16, &CELL_TYPEINFO); (b as *mut *mut u8).write(std::ptr::null_mut); (b as *mut *mut u8).add(1).write(std::ptr::null_mut); let mut roots_b: [*mut u8; 1] = [b]; corvid_gc_from_roots(roots_b.as_mut_ptr, 1); assert_eq!(corvid_gc_verify_drift_count - start_drift, 0, "recycled tracking nodes must not carry stale verifier state"); corvid_release(b); corvid_gc_verify_mode = 0; }
} #[test]
fn fixed_size_pool_respects_per_bucket_upper_bound { let _guard = test_lock; unsafe { let cap = corvid_pool_cached_cap_for_size(24); assert!(cap > 0, "24-byte payload should be pool-eligible"); let before = corvid_pool_cached_blocks_for_size(24); assert!(before >= 0 && before <= cap, "unexpected pre-test pool count: {before}"); let mut ptrs = Vec::with_capacity((cap as usize) + 64); for _ in 0..((cap as usize) + 64) { ptrs.push(corvid_alloc_typed(24, &BOX24_TYPEINFO)); } for ptr in ptrs { corvid_release(ptr); } let after = corvid_pool_cached_blocks_for_size(24); assert_eq!(after, cap, "pool bucket should saturate at its configured cap"); }
}

