//! Runtime value representation.
//!
//! Values are cheap to clone via `Arc` for composite types. Primitives are
//! copied by value. This is the simplest correct design; a move to refcells
//! or unboxed representations is a future optimization and must preserve the
//! semantics asserted by this module's tests.

use corvid_resolve::DefId;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// A runtime value. Clone is cheap: primitives copy; composites share via `Arc`.
#[derive(Clone, Debug)]
pub enum Value {
    /// 64-bit signed integer.
    Int(i64),

    /// 64-bit float. IEEE 754 semantics — notably `NaN != NaN`.
    Float(f64),

    /// UTF-8 string. Shared via `Arc` so clone is O(1).
    String(Arc<str>),

    /// Boolean.
    Bool(bool),

    /// The single `nothing` value.
    Nothing,

    /// A struct instance. The `type_id` points to the `DefId` of the
    /// declaring `type`; `fields` holds field name → value.
    Struct(Arc<StructValue>),

    /// A list of homogeneous values. Homogeneity isn't enforced at runtime
    /// in v0.5 — the type checker already rejects heterogeneous literals.
    List(Arc<Vec<Value>>),
}

/// A struct instance.
#[derive(Clone, Debug)]
pub struct StructValue {
    pub type_id: DefId,
    pub type_name: String,
    pub fields: HashMap<String, Value>,
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
            Value::Struct(s) => s.type_name.clone(),
            Value::List(_) => "List".into(),
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
        Value::Struct(Arc::new(StructValue {
            type_id,
            type_name: type_name.into(),
            fields: fields.into_iter().collect(),
        }))
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
                write!(f, "{}(", s.type_name)?;
                let mut first = true;
                for (k, v) in &s.fields {
                    if !first {
                        write!(f, ", ")?;
                    }
                    first = false;
                    write!(f, "{k}: {v}")?;
                }
                write!(f, ")")
            }
            Value::List(items) => {
                write!(f, "[")?;
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{v}")?;
                }
                write!(f, "]")
            }
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
            (Value::Struct(a), Value::Struct(b)) => {
                a.type_id == b.type_id && a.fields == b.fields
            }
            (Value::List(a), Value::List(b)) => a == b,
            _ => false,
        }
    }
}
