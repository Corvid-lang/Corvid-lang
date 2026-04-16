//! Name-resolution error types.

use corvid_ast::Span;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct ResolveError {
    pub kind: ResolveErrorKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolveErrorKind {
    /// An identifier was used but never declared in any enclosing scope.
    UndefinedName(String),

    /// Two top-level declarations share the same name.
    /// `first_span` points to the first declaration; `span` (on the
    /// outer `ResolveError`) points to the duplicate.
    DuplicateDecl { name: String, first_span: Span },

    /// `extend T:` block targets a type that doesn't exist
    /// (or isn't a type — e.g., the user wrote `extend foo:` where
    /// `foo` is an agent name).
    ExtendTargetNotAType(String),

    /// Two methods with the same name declared inside the
    /// same `extend T:` block (or split across multiple `extend T:`
    /// blocks for the same type).
    DuplicateMethod {
        type_name: String,
        method_name: String,
        first_span: Span,
    },

    /// A method's name collides with a field name on the
    /// same type. `.foo` would be ambiguous (field access vs zero-arg
    /// method call), so we reject at declaration time.
    MethodFieldCollision {
        type_name: String,
        method_name: String,
        field_span: Span,
    },
}

impl fmt::Display for ResolveErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UndefinedName(name) => write!(f, "undefined name `{name}`"),
            Self::DuplicateDecl { name, first_span } => write!(
                f,
                "duplicate declaration `{name}` (first declared at [{}..{}])",
                first_span.start, first_span.end
            ),
            Self::ExtendTargetNotAType(name) => write!(
                f,
                "`extend {name}:` — `{name}` is not a declared type"
            ),
            Self::DuplicateMethod {
                type_name,
                method_name,
                first_span,
            } => write!(
                f,
                "duplicate method `{method_name}` on type `{type_name}` (first declared at [{}..{}])",
                first_span.start, first_span.end
            ),
            Self::MethodFieldCollision {
                type_name,
                method_name,
                field_span,
            } => write!(
                f,
                "method `{method_name}` on type `{type_name}` collides with a field of the same name (declared at [{}..{}])",
                field_span.start, field_span.end
            ),
        }
    }
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}..{}] {}", self.span.start, self.span.end, self.kind)
    }
}
