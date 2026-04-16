//! Structural types the checker assigns to expressions.
//!
//! Distinct from `corvid_ast::TypeRef`, which is what the user *wrote*.
//! `Type` is what the compiler *resolved*.

use corvid_ast::{Effect, WeakEffectRow};
use corvid_resolve::DefId;

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

    /// A tool/prompt/agent, considered as a first-class value.
    Function {
        params: Vec<Type>,
        ret: Box<Type>,
        effect: Effect,
    },

    /// A list of homogeneous elements.
    List(Box<Type>),

    /// Compiler-known `Result<T, E>`.
    Result(Box<Type>, Box<Type>),

    /// Compiler-known `Option<T>`.
    Option(Box<Type>),

    /// Compiler-known `Weak<T>` / `Weak<T, {effects}>`.
    Weak(Box<Type>, WeakEffectRow),

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
            Type::Function { .. } => "function".into(),
            Type::List(inner) => format!("List<{}>", inner.display_name()),
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
            (Type::Option(a), Type::Option(b)) => a.is_assignable_to(b),
            (Type::Result(ok_a, err_a), Type::Result(ok_b, err_b)) => {
                ok_a.is_assignable_to(ok_b) && err_a.is_assignable_to(err_b)
            }
            (Type::Weak(inner_a, effects_a), Type::Weak(inner_b, effects_b)) => {
                inner_a.is_assignable_to(inner_b) && effects_a == effects_b
            }
            (a, b) => a == b,
        }
    }
}
