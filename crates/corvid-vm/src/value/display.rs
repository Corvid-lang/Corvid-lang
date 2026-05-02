//! `Display` and `Debug` impls for runtime `Value`s, plus the
//! small `value_confidence` helper.
//!
//! `Display` for `Value` is the user-facing rendering — what
//! `print` shows, what test snapshots compare against, what
//! diagnostics quote. `escape_display` is the JSON-style
//! string-literal escape used by the `Display` arm for
//! `Value::String`. `Debug` for `WeakValue` and `StreamValue`
//! is the developer-facing rendering used in panics and
//! `println!("{:?}", ...)` debug prints. `value_confidence`
//! pulls the calibration confidence out of `Value::Grounded`
//! values (and returns 1.0 for everything else).

use std::fmt;

use super::{PartialFieldValue, StreamValue, Value, WeakValue};

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
            Value::Grounded(g) => {
                write!(f, "Grounded({}, sources: [", g.inner.get())?;
                for (i, entry) in g.provenance.entries.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}:{}", entry.kind.label(), entry.name)?;
                }
                write!(f, "])")
            }
            Value::Partial(p) => {
                write!(f, "Partial<{}>(", p.type_name())?;
                let mut first = true;
                p.with_fields(|fields| {
                    for (k, v) in fields {
                        if !first {
                            let _ = write!(f, ", ");
                        }
                        first = false;
                        match v {
                            PartialFieldValue::Complete(value) => {
                                let _ = write!(f, "{k}: Complete({value})");
                            }
                            PartialFieldValue::Streaming => {
                                let _ = write!(f, "{k}: Streaming");
                            }
                        }
                    }
                });
                write!(f, ")")
            }
            Value::ResumeToken(token) => write!(
                f,
                "ResumeToken(prompt: {}, delivered: {})",
                token.prompt_name,
                token.delivered.len()
            ),
            Value::Stream(stream) => write!(f, "Stream({})", stream.backpressure_label()),
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

impl fmt::Debug for WeakValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.upgrade() {
            Some(value) => f.debug_tuple("WeakValue").field(&value).finish(),
            None => f.write_str("WeakValue(<cleared>)"),
        }
    }
}

impl fmt::Debug for StreamValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamValue")
            .field("backpressure", self.backpressure())
            .finish()
    }
}

pub fn value_confidence(value: &Value) -> f64 {
    match value {
        Value::Grounded(g) => g.confidence,
        _ => 1.0,
    }
}
