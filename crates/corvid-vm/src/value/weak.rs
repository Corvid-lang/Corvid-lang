//! Weak references to runtime `Value`s — `WeakValue` and the
//! per-shape `StructWeakValue` / `ListWeakValue` newtype wrappers
//! that hold the typed `Weak<...Inner>` handles.
//!
//! `WeakValue::upgrade` is the inverse of `Value::downgrade`: it
//! attempts to revive a `Value` from a weak handle, returning
//! `None` when the underlying object has already been freed by
//! the cycle collector. The check on `meta.strong_count() == 0`
//! catches the race where the object's last strong reference was
//! retired between this `upgrade` call's `Weak::upgrade` and the
//! decision to retain.

use std::sync::Weak;

use super::{ListInner, ListValue, StructInner, StructValue, Value};

pub enum WeakValue {
    String(Weak<str>),
    Struct(StructWeakValue),
    List(ListWeakValue),
}

pub struct StructWeakValue(pub(super) Weak<StructInner>);
pub struct ListWeakValue(pub(super) Weak<ListInner>);

impl Clone for WeakValue {
    fn clone(&self) -> Self {
        match self {
            WeakValue::String(w) => WeakValue::String(w.clone()),
            WeakValue::Struct(w) => WeakValue::Struct(w.clone()),
            WeakValue::List(w) => WeakValue::List(w.clone()),
        }
    }
}

impl Clone for StructWeakValue {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl Clone for ListWeakValue {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl WeakValue {
    pub fn upgrade(&self) -> Option<Value> {
        match self {
            WeakValue::String(value) => value.upgrade().map(Value::String),
            WeakValue::Struct(value) => {
                let upgraded = value.0.upgrade()?;
                if upgraded.meta.strong_count() == 0 {
                    None
                } else {
                    upgraded.meta.retain();
                    Some(Value::Struct(StructValue(upgraded)))
                }
            }
            WeakValue::List(value) => {
                let upgraded = value.0.upgrade()?;
                if upgraded.meta.strong_count() == 0 {
                    None
                } else {
                    upgraded.meta.retain();
                    Some(Value::List(ListValue(upgraded)))
                }
            }
        }
    }

    pub(super) fn ptr_eq(&self, other: &WeakValue) -> bool {
        match (self, other) {
            (WeakValue::String(a), WeakValue::String(b)) => Weak::ptr_eq(a, b),
            (WeakValue::Struct(a), WeakValue::Struct(b)) => Weak::ptr_eq(&a.0, &b.0),
            (WeakValue::List(a), WeakValue::List(b)) => Weak::ptr_eq(&a.0, &b.0),
            _ => false,
        }
    }
}
