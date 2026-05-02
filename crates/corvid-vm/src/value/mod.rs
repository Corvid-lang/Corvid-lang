//! Runtime value representation.
//!
//! Primitives copy by value. Cycle-capable composites (`Struct`, `List`,
//! `Result*`, `OptionSome`) ride on VM-owned heap cells with explicit
//! retain/release bookkeeping so the interpreter can own its refcount
//! semantics instead of delegating them to `Arc`.
//!
//! `String` intentionally stays `Arc<str>` for now because it is a leaf
//! payload with no outgoing refcounted edges. If a future string-like type
//! ever gains outgoing refcounted edges (rope fragments, parent-backed
//! string views, lazy concat nodes), it must migrate to `HeapHandle` style
//! ownership and participate in the VM collector.

use crate::errors::InterpError;
use corvid_ast::BackpressurePolicy;
use corvid_resolve::DefId;
use corvid_runtime::{ProvenanceChain, ProvenanceEntry, ProvenanceKind};
use std::collections::HashMap;
use std::sync::Arc;

mod cells;
mod display;
mod heap;
mod object_ref;
mod stream;
mod weak;
pub use cells::{BoxedValue, ListValue, StructValue};
pub use display::value_confidence;
pub(crate) use heap::Color;
pub(crate) use object_ref::{ObjectRef, WeakObjectRef};
pub(crate) use stream::{StreamChunk, StreamSender};
pub use stream::StreamValue;
pub use weak::{ListWeakValue, StructWeakValue, WeakValue};

/// A runtime value.
#[derive(Debug)]
pub enum Value {
    Int(i64),
    Float(f64),
    String(Arc<str>),
    Bool(bool),
    Nothing,
    Struct(StructValue),
    List(ListValue),
    Weak(WeakValue),
    ResultOk(BoxedValue),
    ResultErr(BoxedValue),
    OptionSome(BoxedValue),
    OptionNone,
    Grounded(GroundedValue),
    Partial(PartialValue),
    ResumeToken(ResumeTokenValue),
    Stream(StreamValue),
}

pub(super) const UNBOUNDED_STREAM_WARN_THRESHOLD: usize = 1024;

/// A value with a provenance chain proving it derives from a grounded source.
#[derive(Debug, Clone)]
pub struct GroundedValue {
    pub inner: BoxedValue,
    pub provenance: ProvenanceChain,
    /// LLM-reported or deterministic confidence, composed via Min
    /// through the call graph. Defaults to 1.0 for deterministic tool
    /// results; prompts can set lower values from self-reported
    /// confidence or logprobs.
    pub confidence: f64,
}

impl GroundedValue {
    pub fn new(inner: Value, provenance: ProvenanceChain) -> Self {
        Self {
            inner: BoxedValue::new(inner),
            provenance,
            confidence: 1.0,
        }
    }

    pub fn with_confidence(inner: Value, provenance: ProvenanceChain, confidence: f64) -> Self {
        Self {
            inner: BoxedValue::new(inner),
            provenance,
            confidence: confidence.clamp(0.0, 1.0),
        }
    }

    pub fn sources(&self) -> &[ProvenanceEntry] {
        &self.provenance.entries
    }

    pub fn unwrap_with_reason(self, reason: &str) -> (Value, ProvenanceEntry) {
        let severed = ProvenanceEntry {
            kind: ProvenanceKind::Severed { reason: reason.to_string() },
            name: "unwrap".to_string(),
            timestamp_ms: 0,
        };
        (self.inner.get(), severed)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PartialFieldValue {
    Complete(Value),
    Streaming,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartialValue {
    type_id: DefId,
    type_name: String,
    fields: HashMap<String, PartialFieldValue>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResumeTokenValue {
    pub(crate) prompt_name: String,
    pub(crate) args: Vec<Value>,
    pub(crate) delivered: Vec<StreamChunk>,
    pub(crate) provider_session: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct StreamResumeContext {
    pub prompt_name: String,
    pub args: Vec<Value>,
    pub provider_session: Option<String>,
}

impl PartialValue {
    pub fn new(
        type_id: DefId,
        type_name: impl Into<String>,
        fields: impl IntoIterator<Item = (String, PartialFieldValue)>,
    ) -> Self {
        Self {
            type_id,
            type_name: type_name.into(),
            fields: fields.into_iter().collect(),
        }
    }

    pub fn type_name(&self) -> &str {
        &self.type_name
    }

    pub fn get_field(&self, field: &str) -> Option<Value> {
        match self.fields.get(field)? {
            PartialFieldValue::Complete(value) => {
                Some(Value::OptionSome(BoxedValue::new(value.clone())))
            }
            PartialFieldValue::Streaming => Some(Value::OptionNone),
        }
    }

    pub fn with_fields<R>(&self, f: impl FnOnce(&HashMap<String, PartialFieldValue>) -> R) -> R {
        f(&self.fields)
    }
}


impl Clone for Value {
    fn clone(&self) -> Self {
        match self {
            Value::Int(n) => Value::Int(*n),
            Value::Float(x) => Value::Float(*x),
            Value::String(s) => Value::String(s.clone()),
            Value::Bool(b) => Value::Bool(*b),
            Value::Nothing => Value::Nothing,
            Value::Struct(s) => Value::Struct(s.clone()),
            Value::List(items) => Value::List(items.clone()),
            Value::Weak(w) => Value::Weak(w.clone()),
            Value::ResultOk(v) => Value::ResultOk(v.clone()),
            Value::ResultErr(v) => Value::ResultErr(v.clone()),
            Value::OptionSome(v) => Value::OptionSome(v.clone()),
            Value::OptionNone => Value::OptionNone,
            Value::Grounded(g) => Value::Grounded(g.clone()),
            Value::Partial(p) => Value::Partial(p.clone()),
            Value::ResumeToken(token) => Value::ResumeToken(token.clone()),
            Value::Stream(stream) => Value::Stream(stream.clone()),
        }
    }
}

impl Value {
    pub fn type_name(&self) -> String {
        match self {
            Value::Int(_) => "Int".into(),
            Value::Float(_) => "Float".into(),
            Value::String(_) => "String".into(),
            Value::Bool(_) => "Bool".into(),
            Value::Nothing => "Nothing".into(),
            Value::Struct(s) => s.type_name().to_string(),
            Value::List(_) => "List".into(),
            Value::Weak(_) => "Weak".into(),
            Value::ResultOk(_) | Value::ResultErr(_) => "Result".into(),
            Value::OptionSome(_) | Value::OptionNone => "Option".into(),
            Value::Grounded(g) => format!("Grounded<{}>", g.inner.get().type_name()),
            Value::Partial(p) => format!("Partial<{}>", p.type_name()),
            Value::ResumeToken(_) => "ResumeToken".into(),
            Value::Stream(stream) => {
                format!("Stream<{}>", stream.backpressure_label())
            }
        }
    }

    pub fn new_struct(
        type_id: DefId,
        type_name: impl Into<String>,
        fields: impl IntoIterator<Item = (String, Value)>,
    ) -> Value {
        Value::Struct(StructValue::new(type_id, type_name, fields))
    }

    pub fn downgrade(&self) -> Option<WeakValue> {
        match self {
            Value::String(s) => Some(WeakValue::String(Arc::downgrade(s))),
            Value::Struct(s) => Some(WeakValue::Struct(StructWeakValue(Arc::downgrade(&s.0)))),
            Value::List(items) => Some(WeakValue::List(ListWeakValue(Arc::downgrade(&items.0)))),
            _ => None,
        }
    }

    pub(crate) fn as_object_ref(&self) -> Option<ObjectRef> {
        match self {
            Value::Struct(s) => Some(ObjectRef::Struct(s.0.clone())),
            Value::List(items) => Some(ObjectRef::List(items.0.clone())),
            Value::ResultOk(v) | Value::ResultErr(v) | Value::OptionSome(v) => {
                Some(ObjectRef::Boxed(v.0.clone()))
            }
            Value::Grounded(g) => Some(ObjectRef::Boxed(g.inner.0.clone())),
            Value::Partial(_) => None,
            Value::ResumeToken(_) => None,
            _ => None,
        }
    }
}



impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Int(a), Value::Float(b)) => (*a as f64) == *b,
            (Value::Float(a), Value::Int(b)) => *a == (*b as f64),
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Nothing, Value::Nothing) => true,
            (Value::Struct(a), Value::Struct(b)) => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Weak(a), Value::Weak(b)) => a.ptr_eq(b),
            (Value::ResultOk(a), Value::ResultOk(b)) => a == b,
            (Value::ResultErr(a), Value::ResultErr(b)) => a == b,
            (Value::OptionSome(a), Value::OptionSome(b)) => a == b,
            (Value::OptionNone, Value::OptionNone) => true,
            (Value::Grounded(a), Value::Grounded(b)) => a.inner == b.inner,
            (Value::Partial(a), Value::Partial(b)) => a == b,
            (Value::ResumeToken(a), Value::ResumeToken(b)) => a == b,
            (Value::Stream(a), Value::Stream(b)) => a == b,
            _ => false,
        }
    }
}




#[cfg(test)]
mod tests {
    use super::{StructValue, Value};
    use corvid_resolve::DefId;
    use std::sync::Arc;

    #[test]
    fn struct_handle_refcount_tracks_value_clones() {
        let value = Value::Struct(StructValue::new(
            DefId(1),
            "Node",
            [("label".to_string(), Value::String(Arc::from("root")))],
        ));
        let strong = match &value {
            Value::Struct(s) => s.strong_count_for_tests(),
            _ => unreachable!(),
        };
        assert_eq!(strong, 1);

        let mut clones = Vec::new();
        for _ in 0..1000 {
            clones.push(value.clone());
        }

        let strong = match &value {
            Value::Struct(s) => s.strong_count_for_tests(),
            _ => unreachable!(),
        };
        assert_eq!(strong, 1001);

        drop(clones);

        let strong = match &value {
            Value::Struct(s) => s.strong_count_for_tests(),
            _ => unreachable!(),
        };
        assert_eq!(strong, 1);
    }
}
