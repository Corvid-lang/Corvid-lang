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
pub struct TypeWarning {
    pub kind: TypeWarningKind,
    pub span: Span,
}

impl TypeWarning {
    pub fn new(kind: TypeWarningKind, span: Span) -> Self {
        Self { kind, span }
    }

    pub fn message(&self) -> String {
        self.kind.message()
    }

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

    /// `try expr on error retry ...` was used on a value that is neither
    /// `Result` nor `Option`.
    InvalidRetryTarget { got: String },

    /// A compiler-known generic received the wrong number of type arguments.
    GenericArityMismatch {
        name: String,
        expected: usize,
        got: usize,
    },

    /// `Weak<T>` was declared over a non-heap-backed target type.
    InvalidWeakTargetType { got: String },

    /// `Weak::new(value)` only accepts heap-backed strong values.
    InvalidWeakNewTarget { got: String },

    /// `Weak::upgrade(value)` requires a weak reference.
    InvalidWeakUpgradeTarget { got: String },

    /// The checker cannot prove that `upgrade()` happens before an
    /// invalidating effect in the weak's effect row.
    WeakUpgradeAcrossEffects { effects: Vec<String> },

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

    /// A dimensional effect constraint was violated.
    EffectConstraintViolation {
        agent: String,
        dimension: String,
        message: String,
    },

    /// `assert <expr>` inside an eval must typecheck to Bool.
    AssertNotBool { got: String },

    /// `assert called <tool>` or `assert called A before B` references
    /// a name that does not resolve to a known callable.
    EvalUnknownTool { name: String },

    /// `assert approved <label>` references an approval label that does
    /// not match any dangerous tool label in the file.
    EvalUnknownApproval { label: String },

    /// Statistical assertion modifiers must stay in range.
    InvalidConfidence { value: f64 },

    /// An agent returns `Grounded<T>` but the compiler cannot prove a
    /// provenance path from a `data: grounded` source feeds into the
    /// return value.
    UngroundedReturn {
        agent: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeWarningKind {
    /// The cost explorer could not prove a static upper bound.
    UnboundedCostAnalysis {
        agent: String,
        message: String,
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
            Self::InvalidRetryTarget { got } => {
                format!("`try ... on error retry ...` can only be used on `Result` or `Option`, got `{got}`")
            }
            Self::GenericArityMismatch { name, expected, got } => {
                format!(
                    "wrong number of type arguments for `{name}`: expected {expected}, got {got}"
                )
            }
            Self::InvalidWeakTargetType { got } => {
                format!("`Weak<T>` requires a heap-backed target type, got `{got}`")
            }
            Self::InvalidWeakNewTarget { got } => {
                format!("`Weak::new(...)` requires a heap-backed strong value, got `{got}`")
            }
            Self::InvalidWeakUpgradeTarget { got } => {
                format!("`Weak::upgrade(...)` requires a `Weak<T>` value, got `{got}`")
            }
            Self::WeakUpgradeAcrossEffects { effects } => {
                format!(
                    "`Weak::upgrade(...)` is not provably valid here: {} may have invalidated this weak since its last refresh",
                    effects.join(", ")
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
            Self::EffectConstraintViolation { agent, message, .. } => {
                format!("effect constraint violated in agent `{agent}`: {message}")
            }
            Self::AssertNotBool { got } => {
                format!("eval assertions must be `Bool`, got `{got}`")
            }
            Self::EvalUnknownTool { name } => {
                format!("eval trace assertion references unknown callable `{name}`")
            }
            Self::EvalUnknownApproval { label } => {
                format!("eval trace assertion references unknown approval label `{label}`")
            }
            Self::InvalidConfidence { value } => {
                format!("statistical assertion confidence must be in [0.0, 1.0], got `{value}`")
            }
            Self::UngroundedReturn { agent, message } => {
                format!("ungrounded return in agent `{agent}`: {message}")
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
            Self::InvalidRetryTarget { .. } => Some(
                "apply retry only to `Result<T, E>` or `Option<T>` expressions".into(),
            ),
            Self::GenericArityMismatch { name, expected, .. } => Some(format!(
                "`{name}` requires {expected} type argument{}",
                if *expected == 1 { "" } else { "s" }
            )),
            Self::InvalidWeakTargetType { .. } => Some(
                "use `Weak<T>` only with heap-backed types like String, user-declared types, or List<T>".into(),
            ),
            Self::InvalidWeakNewTarget { .. } => Some(
                "pass a String, user-declared type, or List<T> value to `Weak::new(...)`".into(),
            ),
            Self::InvalidWeakUpgradeTarget { .. } => Some(
                "call `Weak::upgrade(...)` only on a value whose type is `Weak<T>`".into(),
            ),
            Self::WeakUpgradeAcrossEffects { effects } => Some(format!(
                "refresh the weak with a new `Weak::new(...)` or an earlier `Weak::upgrade(...)`, and avoid `{}` on every path before this call",
                effects.join(", ")
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
            Self::EffectConstraintViolation { dimension, .. } => Some(format!(
                "relax the `@{dimension}` constraint, or remove the call that violates it"
            )),
            Self::AssertNotBool { .. } => Some(
                "make the asserted expression evaluate to `Bool`, for example by adding a comparison".into(),
            ),
            Self::EvalUnknownTool { .. } => Some(
                "declare the referenced tool, prompt, or agent before using it in `assert called ...`".into(),
            ),
            Self::EvalUnknownApproval { .. } => Some(
                "use the PascalCase approval label for a dangerous tool declared in this file".into(),
            ),
            Self::InvalidConfidence { .. } => Some(
                "use `with confidence P over N runs` with 0.0 <= P <= 1.0 and N > 0".into(),
            ),
            Self::UngroundedReturn { .. } => Some(
                "call a tool declared `uses retrieval` (or any effect with `data: grounded`) \
                 and pass its result to the return value, directly or through a prompt"
                    .into(),
            ),
        }
    }
}

impl TypeWarningKind {
    pub fn message(&self) -> String {
        match self {
            Self::UnboundedCostAnalysis { agent, message } => {
                format!("cost analysis warning in agent `{agent}`: {message}")
            }
        }
    }

    pub fn hint(&self) -> Option<String> {
        match self {
            Self::UnboundedCostAnalysis { .. } => Some(
                "use a statically bounded loop or inspect `:cost <agent>` for the partial tree".into(),
            ),
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

impl fmt::Display for TypeWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}..{}] {}", self.span.start, self.span.end, self.message())?;
        if let Some(hint) = self.hint() {
            write!(f, "\n  help: {hint}")?;
        }
        Ok(())
    }
}
