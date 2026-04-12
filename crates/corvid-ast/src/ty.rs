//! Type references as written in source.
//!
//! The AST stores `TypeRef` — the syntactic form of a type annotation.
//! The type checker later resolves these into fully-known structural types.

use crate::span::{Ident, Span};
use serde::{Deserialize, Serialize};

/// A type as the user wrote it in source code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TypeRef {
    /// A named type: `String`, `Ticket`, `Decision`.
    Named { name: Ident, span: Span },

    /// A generic application: `List[Order]`, `Map[String, Int]`.
    Generic {
        name: Ident,
        args: Vec<TypeRef>,
        span: Span,
    },

    /// A function type: `(Int, Int) -> Int`.
    Function {
        params: Vec<TypeRef>,
        ret: Box<TypeRef>,
        span: Span,
    },
}

impl TypeRef {
    pub fn span(&self) -> Span {
        match self {
            TypeRef::Named { span, .. }
            | TypeRef::Generic { span, .. }
            | TypeRef::Function { span, .. } => *span,
        }
    }
}

/// A parameter to a function, tool, agent, or prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Param {
    pub name: Ident,
    pub ty: TypeRef,
    pub span: Span,
}

/// A field in a struct-like type declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    pub name: Ident,
    pub ty: TypeRef,
    pub span: Span,
}
