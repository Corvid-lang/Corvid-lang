//! Expressions — things that produce a value.

use crate::span::{Ident, Span};
use serde::{Deserialize, Serialize};

/// Any expression in Corvid source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    /// A constant: `42`, `"hello"`, `true`, `nothing`.
    Literal { value: Literal, span: Span },

    /// A bare identifier: `order`, `ticket`.
    Ident { name: Ident, span: Span },

    /// A call: `f(x, y)` or `StructName(field1, field2)`.
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },

    /// Field access: `ticket.order_id`.
    FieldAccess {
        target: Box<Expr>,
        field: Ident,
        span: Span,
    },

    /// Index access: `items[0]`, `map["key"]`.
    Index {
        target: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },

    /// Binary operator: `a + b`, `x == y`, `p and q`.
    BinOp {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },

    /// Unary operator: `-x`, `not x`.
    UnOp {
        op: UnaryOp,
        operand: Box<Expr>,
        span: Span,
    },

    /// List literal: `[1, 2, 3]`.
    List { items: Vec<Expr>, span: Span },

    /// Postfix propagation: `expr?`.
    TryPropagate {
        inner: Box<Expr>,
        span: Span,
    },

    /// Retry wrapper: `try expr on error retry N times backoff linear 100`.
    TryRetry {
        body: Box<Expr>,
        attempts: u64,
        backoff: Backoff,
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Literal { span, .. }
            | Expr::Ident { span, .. }
            | Expr::Call { span, .. }
            | Expr::FieldAccess { span, .. }
            | Expr::Index { span, .. }
            | Expr::BinOp { span, .. }
            | Expr::UnOp { span, .. }
            | Expr::List { span, .. }
            | Expr::TryPropagate { span, .. }
            | Expr::TryRetry { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Backoff {
    Linear(u64),
    Exponential(u64),
}

/// A constant value literal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Literal {
    Int(i64),
    Float(f64),
    String(String),
    Bool(bool),
    Nothing,
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinaryOp {
    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,
    Mod,

    // Comparison
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,

    // Logical
    And,
    Or,
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnaryOp {
    Neg,
    Not,
}
