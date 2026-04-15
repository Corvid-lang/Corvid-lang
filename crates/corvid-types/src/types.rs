//! Structural types the checker assigns to expressions.
//!
//! Distinct from `corvid_ast::TypeRef`, which is what the user *wrote*.
//! `Type` is what the compiler *resolved*.

use corvid_ast::Effect;
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
            (a, b) => a == b,
        }
    }
}
