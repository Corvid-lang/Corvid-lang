//! Cycle-collector-facing handle on a heap cell.
//!
//! `ObjectRef` is the type-erased view the cycle collector
//! consults during trial deletion: it answers strong/shadow
//! counts, advances the four-color state machine, walks
//! children, and runs the free-zero / clear-payload paths
//! without caring whether it's looking at a struct, list, or
//! boxed inner. `WeakObjectRef` is the matching
//! `Weak<...Inner>` variant the candidate-root buffer uses to
//! avoid pinning cycles it's about to reclaim.
//!
//! `children_from_map` and `children_from_slice` are the
//! per-payload edge walkers the collector calls through
//! `ObjectRef::children`.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use super::cells::{BoxedInner, ListInner, StructInner};
use super::heap::{Color, HeapMeta};
use super::Value;

#[derive(Clone)]
pub(crate) enum ObjectRef {
    Struct(Arc<StructInner>),
    List(Arc<ListInner>),
    Boxed(Arc<BoxedInner>),
}

#[derive(Clone)]
pub(crate) enum WeakObjectRef {
    Struct(Weak<StructInner>),
    List(Weak<ListInner>),
    Boxed(Weak<BoxedInner>),
}

impl ObjectRef {
    fn meta(&self) -> &HeapMeta {
        match self {
            ObjectRef::Struct(inner) => &inner.meta,
            ObjectRef::List(inner) => &inner.meta,
            ObjectRef::Boxed(inner) => &inner.meta,
        }
    }

    pub(crate) fn ptr_key(&self) -> usize {
        match self {
            ObjectRef::Struct(inner) => Arc::as_ptr(inner) as usize,
            ObjectRef::List(inner) => Arc::as_ptr(inner) as usize,
            ObjectRef::Boxed(inner) => Arc::as_ptr(inner) as usize,
        }
    }

    pub(crate) fn strong_count(&self) -> usize {
        self.meta().strong_count()
    }

    pub(crate) fn release_strong(&self) -> usize {
        self.meta().release()
    }

    pub(crate) fn set_strong_zero(&self) {
        self.meta().set_strong(0);
    }

    pub(crate) fn shadow_count(&self) -> usize {
        self.meta().shadow_count()
    }

    pub(crate) fn set_shadow(&self, value: usize) {
        self.meta().set_shadow(value);
    }

    pub(crate) fn inc_shadow(&self) {
        self.meta().inc_shadow();
    }

    pub(crate) fn dec_shadow(&self) {
        self.meta().dec_shadow();
    }

    pub(crate) fn color(&self) -> Color {
        self.meta().color()
    }

    pub(crate) fn set_color(&self, color: Color) {
        self.meta().set_color(color);
    }

    pub(crate) fn buffered(&self) -> bool {
        self.meta().buffered()
    }

    pub(crate) fn set_buffered(&self, value: bool) {
        self.meta().set_buffered(value);
    }

    pub(crate) fn downgrade(&self) -> WeakObjectRef {
        match self {
            ObjectRef::Struct(inner) => WeakObjectRef::Struct(Arc::downgrade(inner)),
            ObjectRef::List(inner) => WeakObjectRef::List(Arc::downgrade(inner)),
            ObjectRef::Boxed(inner) => WeakObjectRef::Boxed(Arc::downgrade(inner)),
        }
    }

    pub(crate) fn children(&self) -> Vec<ObjectRef> {
        match self {
            ObjectRef::Struct(inner) => inner
                .fields
                .lock()
                .expect("struct fields lock poisoned")
                .as_ref()
                .map(children_from_map)
                .unwrap_or_default(),
            ObjectRef::List(inner) => inner
                .items
                .lock()
                .expect("list items lock poisoned")
                .as_ref()
                .map(|items| children_from_slice(items))
                .unwrap_or_default(),
            ObjectRef::Boxed(inner) => inner
                .value
                .lock()
                .expect("boxed value lock poisoned")
                .as_ref()
                .and_then(Value::as_object_ref)
                .into_iter()
                .collect(),
        }
    }

    pub(crate) fn free_zero_path(&self) {
        self.set_buffered(false);
        self.set_shadow(0);
        self.set_color(Color::Black);
        self.clear_payload();
    }

    pub(crate) fn prepare_collect(&self) {
        self.set_buffered(false);
        self.set_shadow(0);
        self.set_color(Color::Black);
        self.set_strong_zero();
    }

    pub(crate) fn clear_payload(&self) {
        match self {
            ObjectRef::Struct(inner) => {
                let fields = inner
                    .fields
                    .lock()
                    .expect("struct fields lock poisoned")
                    .take();
                drop(fields);
            }
            ObjectRef::List(inner) => {
                let items = inner
                    .items
                    .lock()
                    .expect("list items lock poisoned")
                    .take();
                drop(items);
            }
            ObjectRef::Boxed(inner) => {
                let value = inner
                    .value
                    .lock()
                    .expect("boxed value lock poisoned")
                    .take();
                drop(value);
            }
        }
    }
}

impl WeakObjectRef {
    pub(crate) fn upgrade(&self) -> Option<ObjectRef> {
        match self {
            WeakObjectRef::Struct(inner) => inner.upgrade().map(ObjectRef::Struct),
            WeakObjectRef::List(inner) => inner.upgrade().map(ObjectRef::List),
            WeakObjectRef::Boxed(inner) => inner.upgrade().map(ObjectRef::Boxed),
        }
    }
}

fn children_from_map(fields: &HashMap<String, Value>) -> Vec<ObjectRef> {
    let mut out = Vec::new();
    for value in fields.values() {
        if let Some(child) = value.as_object_ref() {
            out.push(child);
        }
    }
    out
}

fn children_from_slice(items: &[Value]) -> Vec<ObjectRef> {
    let mut out = Vec::new();
    for value in items {
        if let Some(child) = value.as_object_ref() {
            out.push(child);
        }
    }
    out
}
