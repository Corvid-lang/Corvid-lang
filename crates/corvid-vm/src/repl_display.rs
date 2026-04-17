//! REPL-oriented value rendering with cycle and depth guards.

use crate::value::Value;
use std::collections::HashSet;

const MAX_DEPTH: usize = 32;

pub fn render_value(value: &Value) -> String {
    let mut visited = HashSet::new();
    render_value_inner(value, 0, &mut visited)
}

fn render_value_inner(
    value: &Value,
    depth: usize,
    visited: &mut HashSet<usize>,
) -> String {
    if depth >= MAX_DEPTH {
        return "<...>".to_string();
    }

    match value {
        Value::Int(n) => n.to_string(),
        Value::Float(f) => f.to_string(),
        Value::String(s) => format!("\"{}\"", escape_display(s)),
        Value::Bool(b) => {
            if *b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Value::Nothing => "nothing".to_string(),
        Value::Struct(s) => {
            let key = s.ptr_key();
            if !visited.insert(key) {
                return "<cycle>".to_string();
            }
            let rendered = s.with_fields(|fields| {
                let mut fields: Vec<_> = fields.iter().collect();
                fields.sort_by(|(a, _), (b, _)| a.cmp(b));
                fields
                    .into_iter()
                    .map(|(name, value)| {
                        format!(
                            "{name}: {}",
                            render_value_inner(value, depth + 1, visited)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            });
            visited.remove(&key);
            format!("{}({rendered})", s.type_name())
        }
        Value::List(items) => {
            let key = items.ptr_key();
            if !visited.insert(key) {
                return "<cycle>".to_string();
            }
            let rendered = items
                .iter_cloned()
                .into_iter()
                .map(|item| render_value_inner(&item, depth + 1, visited))
                .collect::<Vec<_>>()
                .join(", ");
            visited.remove(&key);
            format!("[{rendered}]")
        }
        Value::Weak(weak) => match weak.upgrade() {
            Some(value) => format!("Weak({})", render_value_inner(&value, depth + 1, visited)),
            None => "Weak(<cleared>)".to_string(),
        },
        Value::ResultOk(inner) => {
            let key = inner.ptr_key();
            if !visited.insert(key) {
                return "<cycle>".to_string();
            }
            let value = inner.get();
            let rendered = format!("Ok({})", render_value_inner(&value, depth + 1, visited));
            visited.remove(&key);
            rendered
        }
        Value::ResultErr(inner) => {
            let key = inner.ptr_key();
            if !visited.insert(key) {
                return "<cycle>".to_string();
            }
            let value = inner.get();
            let rendered = format!("Err({})", render_value_inner(&value, depth + 1, visited));
            visited.remove(&key);
            rendered
        }
        Value::OptionSome(inner) => {
            let key = inner.ptr_key();
            if !visited.insert(key) {
                return "<cycle>".to_string();
            }
            let value = inner.get();
            let rendered = format!("Some({})", render_value_inner(&value, depth + 1, visited));
            visited.remove(&key);
            rendered
        }
        Value::OptionNone => "None".to_string(),
        Value::Grounded(g) => {
            let inner = render_value_inner(&g.inner.get(), depth + 1, visited);
            let sources: Vec<String> = g.provenance.entries.iter().map(|e| {
                format!("{}:{}", e.kind.label(), e.name)
            }).collect();
            if sources.is_empty() {
                format!("Grounded({inner})")
            } else {
                format!("Grounded({inner}, sources: [{}])", sources.join(", "))
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

#[cfg(test)]
mod tests {
    use super::render_value;
    use crate::value::{BoxedValue, Value};
    use std::sync::Arc;

    #[test]
    fn renders_nested_result_option_values() {
        let value = Value::ResultOk(BoxedValue::new(Value::OptionSome(BoxedValue::new(Value::String(
            Arc::from("hi"),
        )))));
        assert_eq!(render_value(&value), "Ok(Some(\"hi\"))");
    }

    #[test]
    fn caps_deep_recursive_rendering() {
        let mut value = Value::Int(0);
        for _ in 0..40 {
            value = Value::OptionSome(BoxedValue::new(value));
        }
        assert!(render_value(&value).contains("<...>"));
    }
}
