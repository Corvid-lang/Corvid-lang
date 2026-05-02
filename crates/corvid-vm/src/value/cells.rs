//! Refcounted heap-cell value types — `StructValue`, `ListValue`,
//! and `BoxedValue` along with their `*Inner` Arc-payloads.
//!
//! Each public handle owns one strong refcount tracked through
//! `HeapMeta` rather than the `Arc`'s internal counter, so the
//! cycle collector can re-target `strong` during reclamation
//! while leaving the `Arc` to manage the underlying allocation.
//! `Clone` retains, `Drop` hands the cell to
//! `cycle_collector::release_object` which decides between an
//! immediate free-zero-path and adding the cell to the candidate
//! buffer for trial deletion.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use corvid_resolve::DefId;

use crate::cycle_collector;

use super::heap::HeapMeta;
use super::{ObjectRef, Value};

#[derive(Debug)]
pub(crate) struct StructInner {
    pub(super) meta: HeapMeta,
    pub(super) type_id: DefId,
    pub(super) type_name: String,
    pub(super) fields: Mutex<Option<HashMap<String, Value>>>,
}

#[derive(Debug)]
pub(crate) struct ListInner {
    pub(super) meta: HeapMeta,
    pub(super) items: Mutex<Option<Vec<Value>>>,
}

#[derive(Debug)]
pub(crate) struct BoxedInner {
    pub(super) meta: HeapMeta,
    pub(super) value: Mutex<Option<Value>>,
}

#[derive(Debug)]
pub struct StructValue(pub(super) Arc<StructInner>);

#[derive(Debug)]
pub struct ListValue(pub(super) Arc<ListInner>);

#[derive(Debug)]
pub struct BoxedValue(pub(super) Arc<BoxedInner>);

impl StructValue {
    pub fn new(
        type_id: DefId,
        type_name: impl Into<String>,
        fields: impl IntoIterator<Item = (String, Value)>,
    ) -> Self {
        Self(Arc::new(StructInner {
            meta: HeapMeta::new(),
            type_id,
            type_name: type_name.into(),
            fields: Mutex::new(Some(fields.into_iter().collect())),
        }))
    }

    pub fn type_id(&self) -> DefId {
        self.0.type_id
    }

    pub fn type_name(&self) -> &str {
        &self.0.type_name
    }

    pub fn get_field(&self, field: &str) -> Option<Value> {
        self.0
            .fields
            .lock()
            .expect("struct fields lock poisoned")
            .as_ref()
            .and_then(|fields| fields.get(field).cloned())
    }

    pub fn with_fields<R>(&self, f: impl FnOnce(&HashMap<String, Value>) -> R) -> R {
        let guard = self.0.fields.lock().expect("struct fields lock poisoned");
        let fields = guard.as_ref().expect("struct payload already freed");
        f(fields)
    }

    pub fn ptr_key(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }

    #[doc(hidden)]
    pub fn set_field(&self, field: impl Into<String>, value: Value) {
        let mut guard = self.0.fields.lock().expect("struct fields lock poisoned");
        let fields = guard.as_mut().expect("struct payload already freed");
        fields.insert(field.into(), value);
    }

    #[doc(hidden)]
    pub fn strong_count_for_tests(&self) -> usize {
        self.0.meta.strong_count()
    }
}

impl Clone for StructValue {
    fn clone(&self) -> Self {
        self.0.meta.retain();
        Self(self.0.clone())
    }
}

impl Drop for StructValue {
    fn drop(&mut self) {
        cycle_collector::release_object(ObjectRef::Struct(self.0.clone()));
    }
}

impl PartialEq for StructValue {
    fn eq(&self, other: &Self) -> bool {
        self.type_id() == other.type_id()
            && self.with_fields(|a| other.with_fields(|b| a == b))
    }
}

impl ListValue {
    pub fn new(items: impl IntoIterator<Item = Value>) -> Self {
        Self(Arc::new(ListInner {
            meta: HeapMeta::new(),
            items: Mutex::new(Some(items.into_iter().collect())),
        }))
    }

    pub fn len(&self) -> usize {
        self.0
            .items
            .lock()
            .expect("list items lock poisoned")
            .as_ref()
            .expect("list payload already freed")
            .len()
    }

    pub fn iter_cloned(&self) -> Vec<Value> {
        self.0
            .items
            .lock()
            .expect("list items lock poisoned")
            .as_ref()
            .expect("list payload already freed")
            .clone()
    }

    pub fn get(&self, idx: usize) -> Option<Value> {
        self.0
            .items
            .lock()
            .expect("list items lock poisoned")
            .as_ref()
            .and_then(|items| items.get(idx).cloned())
    }

    pub fn ptr_key(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }
}

impl Clone for ListValue {
    fn clone(&self) -> Self {
        self.0.meta.retain();
        Self(self.0.clone())
    }
}

impl Drop for ListValue {
    fn drop(&mut self) {
        cycle_collector::release_object(ObjectRef::List(self.0.clone()));
    }
}

impl PartialEq for ListValue {
    fn eq(&self, other: &Self) -> bool {
        self.iter_cloned() == other.iter_cloned()
    }
}

impl BoxedValue {
    pub fn new(value: Value) -> Self {
        Self(Arc::new(BoxedInner {
            meta: HeapMeta::new(),
            value: Mutex::new(Some(value)),
        }))
    }

    pub fn get(&self) -> Value {
        self.0
            .value
            .lock()
            .expect("boxed value lock poisoned")
            .as_ref()
            .expect("boxed payload already freed")
            .clone()
    }

    pub fn ptr_key(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }
}

impl Clone for BoxedValue {
    fn clone(&self) -> Self {
        self.0.meta.retain();
        Self(self.0.clone())
    }
}

impl Drop for BoxedValue {
    fn drop(&mut self) {
        cycle_collector::release_object(ObjectRef::Boxed(self.0.clone()));
    }
}

impl PartialEq for BoxedValue {
    fn eq(&self, other: &Self) -> bool {
        self.get() == other.get()
    }
}
