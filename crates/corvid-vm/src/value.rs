//! Runtime value representation.
//!
//! Primitives copy by value. Cycle-capable composites (`Struct`, `List`,
//! `Result*`, `OptionSome`) ride on VM-owned heap cells with explicit
//! retain/release bookkeeping so the interpreter can own its refcount
//! semantics instead of delegating them to `Arc`.

use corvid_resolve::DefId;
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

/// A runtime value.
#[derive(Debug)]
pub enum Value {
    /// 64-bit signed integer.
    Int(i64),

    /// 64-bit float. IEEE 754 semantics — notably `NaN != NaN`.
    Float(f64),

    /// UTF-8 string. Strings are leaf heap objects, so `Arc` remains enough.
    String(Arc<str>),

    /// Boolean.
    Bool(bool),

    /// The single `nothing` value.
    Nothing,

    /// A struct instance.
    Struct(StructValue),

    /// A list of homogeneous values.
    List(ListValue),
    Weak(WeakValue),
    ResultOk(BoxedValue),
    ResultErr(BoxedValue),
    OptionSome(BoxedValue),
    OptionNone,
}

#[derive(Debug)]
struct HeapMeta {
    strong: AtomicUsize,
}

impl HeapMeta {
    fn new() -> Self {
        Self {
            strong: AtomicUsize::new(1),
        }
    }

    fn retain(&self) {
        self.strong.fetch_add(1, Ordering::Relaxed);
    }

    fn release(&self) -> bool {
        self.strong.fetch_sub(1, Ordering::AcqRel) == 1
    }

    fn strong_count(&self) -> usize {
        self.strong.load(Ordering::Acquire)
    }
}

#[derive(Debug)]
struct StructInner {
    meta: HeapMeta,
    type_id: DefId,
    type_name: String,
    fields: Mutex<Option<HashMap<String, Value>>>,
}

#[derive(Debug)]
struct ListInner {
    meta: HeapMeta,
    items: Mutex<Option<Vec<Value>>>,
}

#[derive(Debug)]
struct BoxedInner {
    meta: HeapMeta,
    value: Mutex<Option<Value>>,
}

#[derive(Debug)]
pub struct StructValue(Arc<StructInner>);

#[derive(Debug)]
pub struct ListValue(Arc<ListInner>);

#[derive(Debug)]
pub struct BoxedValue(Arc<BoxedInner>);

pub enum WeakValue {
    String(Weak<str>),
    Struct(StructWeakValue),
    List(ListWeakValue),
}

pub struct StructWeakValue(Weak<StructInner>);

pub struct ListWeakValue(Weak<ListInner>);

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

    #[cfg(test)]
    fn strong_count(&self) -> usize {
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
        if self.0.meta.release() {
            let fields = self
                .0
                .fields
                .lock()
                .expect("struct fields lock poisoned")
                .take();
            drop(fields);
        }
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
        if self.0.meta.release() {
            let items = self
                .0
                .items
                .lock()
                .expect("list items lock poisoned")
                .take();
            drop(items);
        }
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
        if self.0.meta.release() {
            let value = self
                .0
                .value
                .lock()
                .expect("boxed value lock poisoned")
                .take();
            drop(value);
        }
    }
}

impl PartialEq for BoxedValue {
    fn eq(&self, other: &Self) -> bool {
        self.get() == other.get()
    }
}

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

impl fmt::Debug for WeakValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.upgrade() {
            Some(value) => f.debug_tuple("WeakValue").field(&value).finish(),
            None => f.write_str("WeakValue(<cleared>)"),
        }
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
        }
    }
}

impl Value {
    /// Human-readable name of the value's dynamic type. Used in diagnostics.
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
        }
    }

    /// Produce a new struct value. Intended for tests and the runtime's
    /// tool-boundary code; user programs construct structs via tool/prompt
    /// returns (or later via a constructor syntax in v0.4+).
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
}

/// Pretty printing for debug output and CLI `corvid run` result display.
impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(n) => write!(f, "{n}"),
            Value::Float(x) => write!(f, "{x}"),
            Value::String(s) => write!(f, "\"{}\"", escape_display(s)),
            Value::Bool(b) => write!(f, "{}", if *b { "true" } else { "false" }),
            Value::Nothing => write!(f, "nothing"),
            Value::Struct(s) => {
                write!(f, "{}(", s.type_name())?;
                let mut first = true;
                s.with_fields(|fields| {
                    for (k, v) in fields {
                        if !first {
                            let _ = write!(f, ", ");
                        }
                        first = false;
                        let _ = write!(f, "{k}: {v}");
                    }
                });
                write!(f, ")")
            }
            Value::List(items) => {
                write!(f, "[")?;
                for (i, v) in items.iter_cloned().iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{v}")?;
                }
                write!(f, "]")
            }
            Value::Weak(w) => match w.upgrade() {
                Some(value) => write!(f, "Weak({value})"),
                None => write!(f, "Weak(<cleared>)"),
            },
            Value::ResultOk(v) => write!(f, "Ok({})", v.get()),
            Value::ResultErr(v) => write!(f, "Err({})", v.get()),
            Value::OptionSome(v) => write!(f, "Some({})", v.get()),
            Value::OptionNone => write!(f, "None"),
        }
    }
}

fn escape_display(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            other => out.push(other),
        }
    }
    out
}

/// Equality that matches Corvid's `==` semantics. Used by interpreter
/// binops and by tests.
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
            _ => false,
        }
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

    fn ptr_eq(&self, other: &WeakValue) -> bool {
        match (self, other) {
            (WeakValue::String(a), WeakValue::String(b)) => Weak::ptr_eq(a, b),
            (WeakValue::Struct(a), WeakValue::Struct(b)) => Weak::ptr_eq(&a.0, &b.0),
            (WeakValue::List(a), WeakValue::List(b)) => Weak::ptr_eq(&a.0, &b.0),
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
            Value::Struct(s) => s.strong_count(),
            _ => unreachable!(),
        };
        assert_eq!(strong, 1);

        let mut clones = Vec::new();
        for _ in 0..1000 {
            clones.push(value.clone());
        }

        let strong = match &value {
            Value::Struct(s) => s.strong_count(),
            _ => unreachable!(),
        };
        assert_eq!(strong, 1001);

        drop(clones);

        let strong = match &value {
            Value::Struct(s) => s.strong_count(),
            _ => unreachable!(),
        };
        assert_eq!(strong, 1);
    }
}
