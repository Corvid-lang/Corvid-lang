//! Ã¢â‚¬â€ cycle collector end-to-end test.
//!
//! Current Corvid syntax has no field mutation, so users can't
//! construct refcounted cycles from Corvid source today. To exercise
//! the collector we build the cycle directly via FFI and call
//! `corvid_gc_from_roots` (the deterministic, no-stack-walk variant).
//!
//! Why `_from_roots` and not `corvid_gc`:
//!
//! The stack walk in `corvid_gc` depends on frame pointers being
//! preserved up the chain. Cranelift-compiled Corvid code does
//! (we enabled `preserve_frame_pointers` in module.rs); Rust test
//! release builds do NOT by default. Calling `corvid_gc` from
//! Rust test code can non-deterministically segfault on stacks
//! without proper RBP chains.
//!
//! `corvid_gc_from_roots` sidesteps this: the test names the
//! roots explicitly (or passes none), the collector marks from
//! those alone, sweep runs the same way. Test correctness is
//! independent of Rust's frame-pointer choices.
//!
//! Real Corvid programs running natively invoke the full
//! `corvid_gc` (triggered by the allocation-pressure threshold in
//! `corvid_alloc_typed`), whose mark walk traverses Cranelift-emitted
//! frames with their preserved RBP chains. That path is exercised
//! by `crates/corvid-codegen-cl/tests/` fixtures and later work
//! that can construct cycles in Corvid source (post-field-mutation). use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock}; #[repr(C)]
struct CorvidTypeinfo { size: u32, flags: u32, destroy_fn: Option<unsafe extern "C" fn(*mut u8)>, trace_fn: Option< unsafe extern "C" fn(*mut u8, Option<unsafe extern "C" fn(*mut u8, *mut u8)>, *mut u8,), >, weak_fn: Option<unsafe extern "C" fn(*mut u8)>, elem_typeinfo: *const CorvidTypeinfo, name: *const u8,
} unsafe impl Sync for CorvidTypeinfo {} #[link(name = "corvid_c_runtime", kind = "static")]
extern "C" { fn corvid_alloc_typed(payload_bytes: i64, typeinfo: *const CorvidTypeinfo) -> *mut u8; fn corvid_retain(payload: *mut u8); fn corvid_release(payload: *mut u8); fn corvid_gc_from_roots(roots: *mut *mut u8, n_roots: usize); fn corvid_pool_cached_blocks_for_size(payload_bytes: i64) -> i64; static corvid_alloc_count: i64; static corvid_release_count: i64;
} fn test_lock -> std::sync::MutexGuard<'static, > { static LOCK: OnceLock<Mutex<>> = OnceLock::new; LOCK.get_or_init(|| Mutex::new()).lock.expect("test lock poisoned")
} /// Trace fn for our synthetic 2-pointer "Cell" type: fields at
/// offsets 0 and 8. Called by the collector's mark + sweep-
/// decrement passes.
unsafe extern "C" fn cell_trace(payload: *mut u8, marker: Option<unsafe extern "C" fn(*mut u8, *mut u8)>, ctx: *mut u8,) { let slots = payload as *mut *mut u8; for i in 0..2 { let ptr = *slots.add(i); if !ptr.is_null { if let Some(m) = marker { m(ptr, ctx); } } }
} static CELL_DESTROY_COUNT: AtomicUsize = AtomicUsize::new(0); unsafe extern "C" fn cell_destroy(payload: *mut u8) { CELL_DESTROY_COUNT.fetch_add(1, Ordering::SeqCst); let slots = payload as *mut *mut u8; for i in 0..2 { let ptr = *slots.add(i); if !ptr.is_null { corvid_release(ptr); } }
} static CELL_TYPEINFO: CorvidTypeinfo = CorvidTypeinfo { size: 16, flags: 0x01, // CYCLIC_CAPABLE destroy_fn: Some(cell_destroy), trace_fn: Some(cell_trace), weak_fn: None, elem_typeinfo: std::ptr::null, name: std::ptr::null,
}; static CYCLE24_TYPEINFO: CorvidTypeinfo = CorvidTypeinfo { size: 24, flags: 0x01, destroy_fn: Some(cell_destroy), trace_fn: Some(cell_trace), weak_fn: None, elem_typeinfo: std::ptr::null, name: c"Cycle24".as_ptr as *const u8,
}; /// Unreachable 2-block cycle with NO roots Ã¢â€ â€™ collector frees both.
#[test]
fn cycle_with_no_roots_is_collected { let _guard = test_lock; let start_allocs: i64; let start_releases: i64; unsafe { start_allocs = corvid_alloc_count; start_releases = corvid_release_count; } unsafe { let a = corvid_alloc_typed(16, &CELL_TYPEINFO); let b = corvid_alloc_typed(16, &CELL_TYPEINFO); // field0 points at the other block, field1 is NULL // (zero-init required because corvid_alloc_typed doesn't // zero payload; cell_destroy walks both fields). (a as *mut *mut u8).write(b); (a as *mut *mut u8).add(1).write(std::ptr::null_mut); (b as *mut *mut u8).write(a); (b as *mut *mut u8).add(1).write(std::ptr::null_mut); // a and b go out of scope here Ã¢â‚¬â€ our refs drop, but each // is still held by the other's field. Refcount alone // can't reclaim this. } // Verify refcount alone didn't free the cycle. unsafe { assert_eq!(corvid_alloc_count - start_allocs, 2); assert_eq!(corvid_release_count - start_releases, 0, "cycle leaks under refcount alone; that's why 17d exists"); } // Collect with NO roots Ã¢â‚¬â€ both blocks must be freed. unsafe { let empty: &mut [*mut u8] = &mut []; corvid_gc_from_roots(empty.as_mut_ptr, 0); assert_eq!(corvid_release_count - start_releases, 2, "2-block cycle reclaimed by sweep"); }
} /// Cycle with ONE root held externally Ã¢â€ â€™ mark walk traces from
/// the root through the cycle, sweep finds nothing unmarked, both
/// blocks survive.
#[test]
fn cycle_with_external_root_survives { let _guard = test_lock; let start_allocs: i64; let start_releases: i64; unsafe { start_allocs = corvid_alloc_count; start_releases = corvid_release_count; } let root_a: *mut u8; unsafe { let a = corvid_alloc_typed(16, &CELL_TYPEINFO); let b = corvid_alloc_typed(16, &CELL_TYPEINFO); (a as *mut *mut u8).write(b); (a as *mut *mut u8).add(1).write(std::ptr::null_mut); (b as *mut *mut u8).write(a); (b as *mut *mut u8).add(1).write(std::ptr::null_mut); corvid_retain(a); // external ref; a's refcount = 2 root_a = a; } // Collect with `a` as the root Ã¢â‚¬â€ both blocks must survive // (b is reachable via a->field0). unsafe { let mut roots: [*mut u8; 1] = [root_a]; corvid_gc_from_roots(roots.as_mut_ptr, 1); assert_eq!(corvid_release_count - start_releases, 0, "cycle reachable from root Ã¢â‚¬â€ nothing freed"); } // Now drop our external ref. Refcount fast path fires // destroy_fn on `a`, which releases `b`, freeing it. Then `a` // is freed. Two deterministic frees via refcount alone Ã¢â‚¬â€ the // cycle's self-reference in `b` drops to 0 after a's destroy // calls release on b. // // Wait Ã¢â‚¬â€ not quite. After we release a, a's refcount drops // from 2 to 1 (b still holds a via b->field0). No free. The // cycle is now unreachable from any root but held alive by // itself. Another `corvid_gc_from_roots` call with empty // roots reclaims it. unsafe { corvid_release(root_a); let empty: &mut [*mut u8] = &mut []; corvid_gc_from_roots(empty.as_mut_ptr, 0); assert_eq!(corvid_release_count - start_releases, 2, "after releasing root + GC, cycle collected"); } let _ = start_allocs;
} /// Non-cycle with destroy_fn Ã¢â‚¬â€ refcount fast path still works
/// alongside the collector. Sanity check that adding cycle-
/// collection infrastructure didn't regress the simple case.
#[test]
fn acyclic_refcount_path_still_works { let _guard = test_lock; let start_releases: i64; unsafe { start_releases = corvid_release_count; } unsafe { let p = corvid_alloc_typed(16, &CELL_TYPEINFO); // Zero the fields Ã¢â‚¬â€ `corvid_alloc_typed` does NOT zero the // payload, and `cell_destroy` walks both fields calling // `corvid_release` on each. Uninitialized memory Ã¢â€ â€™ UB. let slots = p as *mut *mut u8; slots.write(std::ptr::null_mut); slots.add(1).write(std::ptr::null_mut); corvid_release(p); // refcount 1 Ã¢â€ â€™ 0, destroy_fn fires, free. } unsafe { assert_eq!(corvid_release_count - start_releases, 1, "acyclic block freed immediately by refcount Ã¢â‚¬â€ no GC needed"); }
} #[test]
fn gc_sweep_returns_cycle_blocks_to_pool { let _guard = test_lock; unsafe { let before = corvid_pool_cached_blocks_for_size(24); let a = corvid_alloc_typed(24, &CYCLE24_TYPEINFO); let b = corvid_alloc_typed(24, &CYCLE24_TYPEINFO); (a as *mut *mut u8).write(b); (a as *mut *mut u8).add(1).write(std::ptr::null_mut); (b as *mut *mut u8).write(a); (b as *mut *mut u8).add(1).write(std::ptr::null_mut); let empty: &mut [*mut u8] = &mut []; corvid_gc_from_roots(empty.as_mut_ptr, 0); let after = corvid_pool_cached_blocks_for_size(24); assert_eq!(after - before, 2, "GC sweep should recycle freed fixed-size cycle blocks into the pool"); }
}

