//! Type-checking and effect-checking error types.
//!
//! Every error carries a one-line `message` and, where possible, a `hint`
//! that tells the user exactly how to fix the problem.

use corvid_ast::Span;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct TypeError {
    pub kind: TypeErrorKind,
    pub span: Span,
}

impl TypeError {
    pub fn new(kind: TypeErrorKind, span: Span) -> Self {
        Self { kind, span }
    }

    /// The "what went wrong" message, as a single line.
    pub fn message(&self) -> String {
        self.kind.message()
    }

    /// Optional "here's how to fix it" suggestion.
    pub fn hint(&self) -> Option<String> {
        self.kind.hint()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeErrorKind {
    /// Wrong number of arguments in a call.
    ArityMismatch {
        callee: String,
        expected: usize,
        got: usize,
    },

    /// An argument's type doesn't match the parameter's declared type.
    TypeMismatch {
        expected: String,
        got: String,
        /// Optional context for where the mismatch was detected.
        context: String,
    },

    /// A field that doesn't exist on the given struct.
    UnknownField {
        struct_name: String,
        field: String,
    },

    /// Calling something that isn't callable (e.g. a primitive value).
    NotCallable { got: String },

    /// Field access on a non-struct value.
    NotAStruct { got: String },

    /// A type name used where a value was expected.
    /// E.g. `x = String` or `greet(Int)`.
    TypeAsValue { name: String },

    /// A tool or agent referenced without `()`.
    /// E.g. `x = get_order` instead of `x = get_order(id)`.
    BareFunctionReference { name: String },

    /// `expr?` was used on a value that is neither `Result` nor `Option`.
    InvalidTryPropagate { got: String },

    /// `expr?` was used in a function whose return type cannot absorb the
    /// propagated error/none branch.
    TryPropagateReturnMismatch {
        expected: String,
        got: String,
    },

    /// A compiler-known generic received the wrong number of type arguments.
    GenericArityMismatch {
        name: String,
        expected: usize,
        got: usize,
    },

    /// The return type declared doesn't match what the body returns.
    ReturnTypeMismatch {
        expected: String,
        got: String,
    },

    /// The headline error: a `dangerous` tool was called without a matching
    /// prior `approve` in the same block.
    UnapprovedDangerousCall {
        tool: String,
        /// The `approve` label the user should have written (PascalCase).
        expected_approve_label: String,
        arity: usize,
    },
}

impl TypeErrorKind {
    pub fn message(&self) -> String {
        match self {
            Self::ArityMismatch { callee, expected, got } => {
                format!(
                    "wrong number of arguments to `{callee}`: expected {expected}, got {got}"
                )
            }
            Self::TypeMismatch { expected, got, context } => {
                if context.is_empty() {
                    format!("type mismatch: expected `{expected}`, got `{got}`")
                } else {
                    format!("type mismatch in {context}: expected `{expected}`, got `{got}`")
                }
            }
            Self::UnknownField { struct_name, field } => {
                format!("no field named `{field}` on type `{struct_name}`")
            }
            Self::NotCallable { got } => {
                format!("cannot call a value of type `{got}`")
            }
            Self::NotAStruct { got } => {
                format!("field access requires a struct value, got `{got}`")
            }
            Self::TypeAsValue { name } => {
                format!("`{name}` is a type, not a value")
            }
            Self::BareFunctionReference { name } => {
                format!("`{name}` is a function; call it with `()` to use its result")
            }
            Self::InvalidTryPropagate { got } => {
                format!("`?` can only be used on `Result` or `Option`, got `{got}`")
            }
            Self::TryPropagateReturnMismatch { expected, got } => {
                format!(
                    "`?` return context mismatch: expected enclosing return type `{expected}`, got `{got}`"
                )
            }
            Self::GenericArityMismatch { name, expected, got } => {
                format!(
                    "wrong number of type arguments for `{name}`: expected {expected}, got {got}"
                )
            }
            Self::ReturnTypeMismatch { expected, got } => {
                format!(
                    "return type mismatch: declared `{expected}`, but the body returns `{got}`"
                )
            }
            Self::UnapprovedDangerousCall { tool, .. } => {
                format!("dangerous tool `{tool}` called without a prior `approve`")
            }
        }
    }

    pub fn hint(&self) -> Option<String> {
        match self {
            Self::ArityMismatch { callee, expected, .. } => Some(format!(
                "`{callee}` takes {expected} argument{}",
                if *expected == 1 { "" } else { "s" }
            )),
            Self::TypeMismatch { expected, .. } => Some(format!(
                "change the value to produce a `{expected}`, or update the signature"
            )),
            Self::UnknownField { struct_name, field } => Some(format!(
                "check the declaration of `{struct_name}` for the correct field name (you wrote `{field}`)"
            )),
            Self::NotCallable { .. } => Some(
                "only tools, agents, prompts, and imported functions can be called".into(),
            ),
            Self::NotAStruct { .. } => {
                Some("use `.field` only on values of a user-declared `type`".into())
            }
            Self::TypeAsValue { name } => Some(format!(
                "to create a value of type `{name}`, call a tool or prompt that returns one"
            )),
            Self::BareFunctionReference { name } => {
                Some(format!("did you mean `{name}(...)` ?"))
            }
            Self::InvalidTryPropagate { .. } => Some(
                "apply `?` only to `Result<T, E>` or `Option<T>` values".into(),
            ),
            Self::TryPropagateReturnMismatch { expected, .. } => Some(format!(
                "change the enclosing return type to `{expected}`, or remove `?`"
            )),
            Self::GenericArityMismatch { name, expected, .. } => Some(format!(
                "`{name}` requires {expected} type argument{}",
                if *expected == 1 { "" } else { "s" }
            )),
            Self::ReturnTypeMismatch { expected, .. } => Some(format!(
                "change the final `return` to produce a `{expected}`, or update the declared return type"
            )),
            Self::UnapprovedDangerousCall {
                expected_approve_label,
                arity,
                ..
            } => {
                let args = (0..*arity)
                    .map(|i| format!("arg{}", i + 1))
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(format!(
                    "add `approve {expected_approve_label}({args})` on the line before this call"
                ))
            }
        }
    }
}

impl fmt::Display for TypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}..{}] {}", self.span.start, self.span.end, self.message())?;
        if let Some(hint) = self.hint() {
            write!(f, "\n  help: {hint}")?;
        }
        Ok(())
    }
}
