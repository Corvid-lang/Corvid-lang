//! Structural types the checker assigns to expressions.
//!
//! Distinct from `corvid_ast::TypeRef`, which is what the user *wrote*.
//! `Type` is what the compiler *resolved*.

use corvid_ast::{Effect, WeakEffectRow};
use corvid_resolve::DefId;

/// Stable identity for a struct imported from another `.cor` module.
/// The module path is part of the type identity so two modules can
/// both export `Receipt` without becoming accidentally assignable.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImportedStructType {
    pub module_path: String,
    pub def_id: DefId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Type {
    // Primitives
    Int,
    Float,
    String,
    Bool,
    Nothing,

    /// A user-declared `type` (struct-like).
    Struct(DefId),

    /// A public `type` imported through `alias.Name`.
    ImportedStruct(ImportedStructType),

    /// A tool/prompt/agent, considered as a first-class value.
    Function {
        params: Vec<Type>,
        ret: Box<Type>,
        effect: Effect,
    },

    /// A list of homogeneous elements.
    List(Box<Type>),

    /// Compiler-known `Stream<T>`.
    Stream(Box<Type>),

    /// Compiler-known `Result<T, E>`.
    Result(Box<Type>, Box<Type>),

    /// Compiler-known `Option<T>`.
    Option(Box<Type>),

    /// Compiler-known `Weak<T>` / `Weak<T, {effects}>`.
    Weak(Box<Type>, WeakEffectRow),

    /// Compiler-known `Grounded<T>` — a value whose provenance chain
    /// includes at least one `data: grounded` source. The compiler
    /// verifies this statically by tracing data flow from retrieval
    /// tools through prompts to return types.
    Grounded(Box<Type>),

    /// Compiler-known `TraceId` — an opaque handle to a recorded
    /// JSONL trace, used as the subject of a `replay <expr>:`
    /// expression. String literals coerce to `TraceId` inside a
    /// replay context so `replay "run.jsonl": ...` parses
    /// naturally; richer producers (`Trace::load(...)`) can land
    /// later without breaking the surface syntax. Phase 21 slice
    /// 21-inv-E-3.
    TraceId,

    /// Placeholder when the checker can't determine a precise type.
    /// Propagates without cascading errors.
    Unknown,
}

impl Type {
    /// Human-readable name used in diagnostic messages.
    pub fn display_name(&self) -> String {
        match self {
            Type::Int => "Int".into(),
            Type::Float => "Float".into(),
            Type::String => "String".into(),
            Type::Bool => "Bool".into(),
            Type::Nothing => "Nothing".into(),
            Type::Struct(_) => "struct".into(),
            Type::ImportedStruct(imported) => imported.name.clone(),
            Type::Function { .. } => "function".into(),
            Type::List(inner) => format!("List<{}>", inner.display_name()),
            Type::Stream(inner) => format!("Stream<{}>", inner.display_name()),
            Type::Result(ok, err) => {
                format!("Result<{}, {}>", ok.display_name(), err.display_name())
            }
            Type::Option(inner) => format!("Option<{}>", inner.display_name()),
            Type::Weak(inner, effects) => {
                if effects.is_any() {
                    format!("Weak<{}>", inner.display_name())
                } else {
                    let names = effects
                        .effects()
                        .into_iter()
                        .map(|effect| match effect {
                            corvid_ast::WeakEffect::ToolCall => "tool_call",
                            corvid_ast::WeakEffect::Llm => "llm",
                            corvid_ast::WeakEffect::Approve => "approve",
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("Weak<{}, {{{names}}}>", inner.display_name())
                }
            }
            Type::Grounded(inner) => format!("Grounded<{}>", inner.display_name()),
            Type::TraceId => "TraceId".into(),
            Type::Unknown => "<unknown>".into(),
        }
    }

    /// Is this type compatible with `other` in a value-assignment position?
    ///
    /// v0.1 is intentionally lenient: structurally identical types match,
    /// `Unknown` matches anything (to avoid error cascades), and `Int`
    /// implicitly coerces to `Float` in typing-friendly contexts.
    pub fn is_assignable_to(&self, other: &Type) -> bool {
        match (self, other) {
            (Type::Unknown, _) | (_, Type::Unknown) => true,
            (Type::Int, Type::Float) => true, // widening
            (Type::List(a), Type::List(b)) => a.is_assignable_to(b),
            (Type::Stream(a), Type::Stream(b)) => a.is_assignable_to(b),
            (Type::Option(a), Type::Option(b)) => a.is_assignable_to(b),
            (Type::Result(ok_a, err_a), Type::Result(ok_b, err_b)) => {
                ok_a.is_assignable_to(ok_b) && err_a.is_assignable_to(err_b)
            }
            (Type::Weak(inner_a, effects_a), Type::Weak(inner_b, effects_b)) => {
                inner_a.is_assignable_to(inner_b) && effects_a == effects_b
            }
            (Type::Grounded(a), Type::Grounded(b)) => a.is_assignable_to(b),
            // Legacy compatibility: Grounded<T> remains assignable to T.
            // New code should prefer `.unwrap_discarding_sources()` so the
            // provenance drop is visible in source and IR.
            (Type::Grounded(inner), other) => inner.is_assignable_to(other),
            (a, b) => a == b,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Type;

    #[test]
    fn stream_display_name_and_assignability_follow_inner_type() {
        let stream = Type::Stream(Box::new(Type::String));
        assert_eq!(stream.display_name(), "Stream<String>");
        assert!(stream.is_assignable_to(&Type::Stream(Box::new(Type::String))));
        assert!(!stream.is_assignable_to(&Type::Stream(Box::new(Type::Int))));
    }
}
