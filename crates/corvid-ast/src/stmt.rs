//! Statements — things that execute inside a block but don't produce a value.

use crate::expr::Expr;
use crate::span::{Ident, Span};
use crate::ty::TypeRef;
use serde::{Deserialize, Serialize};

/// A block: a sequence of statements that share a lexical scope.
/// Used for agent bodies, function bodies, and branches of `if`/`for`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

/// Any statement in Corvid source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Stmt {
    /// Variable binding: `order = get_order(id)` or `order: Order = ...`.
    Let {
        name: Ident,
        ty: Option<TypeRef>,
        value: Expr,
        span: Span,
    },

    /// Return from a function/agent: `return decision`.
    Return {
        value: Option<Expr>,
        span: Span,
    },

    /// Conditional: `if cond: ... else: ...`.
    If {
        cond: Expr,
        then_block: Block,
        else_block: Option<Block>,
        span: Span,
    },

    /// Iteration: `for item in items: ...`.
    For {
        var: Ident,
        iter: Expr,
        body: Block,
        span: Span,
    },

    /// The approval gate — the core of Corvid's safety story.
    ///
    /// `approve Action(...)` must precede any `Irreversible` tool call
    /// in the same block whose signature matches `Action`.
    Approve { action: Expr, span: Span },

    /// An expression evaluated for its side effects: `issue_refund(...)`.
    Expr { expr: Expr, span: Span },
}

impl Stmt {
    pub fn span(&self) -> Span {
        match self {
            Stmt::Let { span, .. }
            | Stmt::Return { span, .. }
            | Stmt::If { span, .. }
            | Stmt::For { span, .. }
            | Stmt::Approve { span, .. }
            | Stmt::Expr { span, .. } => *span,
        }
    }
}
