//! Type-checking and effect-checking error types.
//!
//! Every error carries a one-line `message` and, where possible, a `hint`
//! that tells the user exactly how to fix the problem.
//!
//! Diagnostics that enforce a public Corvid guarantee additionally carry
//! a `guarantee_id` resolving to a row in
//! [`corvid_guarantees::GUARANTEE_REGISTRY`]. This makes each contract
//! enforcement non-anonymous: `corvid claim --explain` reports which
//! guarantees a binary was checked against, and Slice 35-E will reject
//! any registered Static guarantee with no corresponding tagged
//! diagnostic. Non-contract diagnostics (arity mismatches, type
//! mismatches, unknown fields) carry `guarantee_id == None` —
//! they are diagnostics about the program's well-formedness, not about
//! a public Corvid promise.

use corvid_ast::Span;

mod error_kind;
pub use error_kind::TypeErrorKind;
mod warning_kind;
pub use warning_kind::TypeWarningKind;
mod display;

#[derive(Debug, Clone, PartialEq)]
pub struct TypeError {
    pub kind: TypeErrorKind,
    pub span: Span,
    /// `Some(id)` when this diagnostic enforces the
    /// [`corvid_guarantees::Guarantee`] with the given stable id;
    /// `None` for general well-formedness diagnostics that do not
    /// back a public Corvid promise.
    pub guarantee_id: Option<&'static str>,
}

impl TypeError {
    /// Construct a non-contract diagnostic (arity mismatch, type
    /// mismatch, unknown field, etc.). Use [`with_guarantee`](Self::with_guarantee)
    /// for diagnostics that enforce a registered Corvid guarantee.
    pub fn new(kind: TypeErrorKind, span: Span) -> Self {
        Self {
            kind,
            span,
            guarantee_id: None,
        }
    }

    /// Construct a contract-enforcing diagnostic. The `guarantee_id`
    /// must resolve to a row in
    /// [`corvid_guarantees::GUARANTEE_REGISTRY`]; in debug builds this
    /// is asserted at construction so an unregistered or misspelled
    /// id fails fast in tests rather than silently shipping. The
    /// release build trusts the assertion already passed in test.
    pub fn with_guarantee(kind: TypeErrorKind, span: Span, guarantee_id: &'static str) -> Self {
        debug_assert!(
            corvid_guarantees::lookup(guarantee_id).is_some(),
            "TypeError::with_guarantee called with unregistered id `{guarantee_id}` \
             — every contract-enforcing diagnostic must reference a row in \
             corvid_guarantees::GUARANTEE_REGISTRY (Phase 35-A)"
        );
        Self {
            kind,
            span,
            guarantee_id: Some(guarantee_id),
        }
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
