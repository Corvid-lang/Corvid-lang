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

    /// `replay <trace>: when llm("<name>") -> ...` — `<name>`
    /// doesn't match any `prompt` declaration in scope. Either the
    /// prompt doesn't exist, is declared under a different name,
    /// or exists under a different declaration kind (tool, agent).
    UnknownReplayPrompt { name: String },

    /// `replay <trace>: when tool("<name>", ...) -> ...` — `<name>`
    /// doesn't match any `tool` declaration in scope.
    UnknownReplayTool { name: String },

    /// `replay <trace>: when approve("<label>") -> ...` — `<label>`
    /// doesn't appear at any `approve Label(...)` site in the file.
    /// Labels aren't declarations (they're free-form identifiers at
    /// approval sites), so resolution is a presence check against
    /// the set of labels used elsewhere in the program.
    UnknownReplayApproval { label: String },

    /// A replay arm names the right kind of declaration (e.g. the
    /// name *does* resolve), but the kind is wrong for the event
    /// family. For example `when llm("get_order") -> ...` where
    /// `get_order` is a `tool`, not a `prompt`. The wrong-kind
    /// case is distinguished from UnknownReplayPrompt so the
    /// diagnostic can suggest the right event family.
    ReplayPatternKindMismatch {
        name: String,
        expected_kind: &'static str,
        actual_kind: &'static str,
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
            Self::UnknownReplayPrompt { name } => write!(
                f,
                "replay pattern `llm(\"{name}\")` does not match any `prompt` declaration"
            ),
            Self::UnknownReplayTool { name } => write!(
                f,
                "replay pattern `tool(\"{name}\", ...)` does not match any `tool` declaration"
            ),
            Self::UnknownReplayApproval { label } => write!(
                f,
                "replay pattern `approve(\"{label}\")` does not match any approval label used in this file"
            ),
            Self::ReplayPatternKindMismatch {
                name,
                expected_kind,
                actual_kind,
            } => write!(
                f,
                "replay pattern expects `{name}` to be a {expected_kind}, but `{name}` is a {actual_kind}"
            ),
        }
    }
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}..{}] {}", self.span.start, self.span.end, self.kind)
    }
}
