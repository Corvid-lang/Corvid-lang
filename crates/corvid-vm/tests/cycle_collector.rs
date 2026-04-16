use corvid_resolve::DefId;
use corvid_vm::{collect_cycles, StructValue, Value};
use std::sync::{Mutex, OnceLock};

fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().expect("test lock poisoned")
}

fn weak_of_struct(value: &StructValue) -> corvid_vm::Value {
    Value::Struct(value.clone())
}

#[test]
fn two_block_cycle_collects_after_external_refs_drop() {
    let _guard = test_lock();
    let a = StructValue::new(DefId(1), "Node", []);
    let b = StructValue::new(DefId(1), "Node", []);
    a.set_field("next", Value::Struct(b.clone()));
    b.set_field("next", Value::Struct(a.clone()));

    let weak_a = weak_of_struct(&a).downgrade().expect("weak a");
    let weak_b = weak_of_struct(&b).downgrade().expect("weak b");

    drop(a);
    drop(b);

    assert!(weak_a.upgrade().is_some(), "cycle should survive refcount alone");
    assert!(weak_b.upgrade().is_some(), "cycle should survive refcount alone");

    assert_eq!(collect_cycles(), 2);
    assert!(weak_a.upgrade().is_none(), "A should be reclaimed");
    assert!(weak_b.upgrade().is_none(), "B should be reclaimed");
}

#[test]
fn three_block_cycle_collects_after_external_refs_drop() {
    let _guard = test_lock();
    let a = StructValue::new(DefId(1), "Node", []);
    let b = StructValue::new(DefId(1), "Node", []);
    let c = StructValue::new(DefId(1), "Node", []);
    a.set_field("next", Value::Struct(b.clone()));
    b.set_field("next", Value::Struct(c.clone()));
    c.set_field("next", Value::Struct(a.clone()));

    let weak_a = weak_of_struct(&a).downgrade().expect("weak a");
    let weak_b = weak_of_struct(&b).downgrade().expect("weak b");
    let weak_c = weak_of_struct(&c).downgrade().expect("weak c");

    drop(a);
    drop(b);
    drop(c);

    assert!(weak_a.upgrade().is_some(), "cycle should survive refcount alone");
    assert!(weak_b.upgrade().is_some(), "cycle should survive refcount alone");
    assert!(weak_c.upgrade().is_some(), "cycle should survive refcount alone");

    assert_eq!(collect_cycles(), 3);
    assert!(weak_a.upgrade().is_none());
    assert!(weak_b.upgrade().is_none());
    assert!(weak_c.upgrade().is_none());
}

#[test]
fn acyclic_struct_frees_without_collection() {
    let _guard = test_lock();
    let node = StructValue::new(DefId(1), "Node", []);
    let weak = weak_of_struct(&node).downgrade().expect("weak");
    drop(node);
    assert!(weak.upgrade().is_none(), "acyclic value should free on refcount fast path");
    assert_eq!(collect_cycles(), 0, "nothing should remain for cycle collection");
}
