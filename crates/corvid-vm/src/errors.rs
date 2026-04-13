//! Runtime errors raised by the interpreter.
//!
//! These are distinct from the compile-time errors in `corvid-types`. A
//! program that passes the type checker can still raise these at runtime
//! (division by zero, unapproved action at a bypassed boundary, etc.).

use corvid_ast::Span;
use corvid_resolve::LocalId;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct InterpError {
    pub kind: InterpErrorKind,
    pub span: Span,
}

impl InterpError {
    pub fn new(kind: InterpErrorKind, span: Span) -> Self {
        Self { kind, span }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum InterpErrorKind {
    /// A local was referenced that has no binding in the current env.
    /// Reaching this typically means the resolver / IR lowering are out
    /// of sync with the interpreter.
    UndefinedLocal(LocalId),

    /// An operation received a value whose type it can't handle.
    /// `got` is the dynamic type name; `expected` is a short description.
    TypeMismatch { expected: String, got: String },

    /// Field access targeted a struct, but the field doesn't exist on it.
    UnknownField { struct_name: String, field: String },

    /// Arithmetic failure (overflow, division by zero, etc.).
    Arithmetic(String),

    /// Indexing a list with an out-of-range index.
    IndexOutOfBounds { len: usize, index: i64 },

    /// The interpreter encountered a construct it doesn't implement yet.
    /// Expected only during phased rollout — should never fire in shipped code.
    NotImplemented(String),

    /// An agent or tool returned without producing a value, but a value was expected.
    MissingReturn,

    /// An approval action was denied or failed at runtime.
    ApprovalDenied(String),

    /// A tool or prompt couldn't be dispatched. Arrives in later Phase-11 slices.
    DispatchFailed(String),

    /// Catch-all with message. Prefer adding a dedicated variant over this.
    Other(String),
}

impl fmt::Display for InterpErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UndefinedLocal(id) => write!(f, "local binding #{} is unbound", id.0),
            Self::TypeMismatch { expected, got } => {
                write!(f, "type mismatch: expected `{expected}`, got `{got}`")
            }
            Self::UnknownField { struct_name, field } => {
                write!(f, "no field `{field}` on type `{struct_name}`")
            }
            Self::Arithmetic(msg) => write!(f, "arithmetic error: {msg}"),
            Self::IndexOutOfBounds { len, index } => {
                write!(f, "index {index} out of bounds for list of length {len}")
            }
            Self::NotImplemented(what) => {
                write!(f, "interpreter does not yet support: {what}")
            }
            Self::MissingReturn => write!(f, "function ended without returning a value"),
            Self::ApprovalDenied(action) => {
                write!(f, "approval denied for action `{action}`")
            }
            Self::DispatchFailed(msg) => write!(f, "call dispatch failed: {msg}"),
            Self::Other(m) => f.write_str(m),
        }
    }
}

impl fmt::Display for InterpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}..{}] {}", self.span.start, self.span.end, self.kind)
    }
}
