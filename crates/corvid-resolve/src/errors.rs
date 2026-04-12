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
        }
    }
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}..{}] {}", self.span.start, self.span.end, self.kind)
    }
}
