//! Type references as written in source.
//!
//! The AST stores `TypeRef` — the syntactic form of a type annotation.
//! The type checker later resolves these into fully-known structural types.

use crate::span::{Ident, Span};
use serde::{Deserialize, Serialize};

/// Effect rows attached to `Weak<T, {effects}>`.
///
/// These are not the same as tool declaration effects (`safe` /
/// `dangerous`). They describe which runtime actions invalidate the
/// compiler's proof that a weak reference is still refresh-valid at a
/// given program point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum WeakEffect {
    ToolCall,
    Llm,
    Approve,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WeakEffectRow {
    pub tool_call: bool,
    pub llm: bool,
    pub approve: bool,
}

impl WeakEffectRow {
    pub const fn any() -> Self {
        Self {
            tool_call: true,
            llm: true,
            approve: true,
        }
    }

    pub const fn empty() -> Self {
        Self {
            tool_call: false,
            llm: false,
            approve: false,
        }
    }

    pub fn from_effects(effects: &[WeakEffect]) -> Self {
        let mut row = Self::empty();
        for effect in effects {
            match effect {
                WeakEffect::ToolCall => row.tool_call = true,
                WeakEffect::Llm => row.llm = true,
                WeakEffect::Approve => row.approve = true,
            }
        }
        row
    }

    pub fn effects(&self) -> Vec<WeakEffect> {
        let mut effects = Vec::new();
        if self.tool_call {
            effects.push(WeakEffect::ToolCall);
        }
        if self.llm {
            effects.push(WeakEffect::Llm);
        }
        if self.approve {
            effects.push(WeakEffect::Approve);
        }
        effects
    }

    pub fn is_any(&self) -> bool {
        self.tool_call && self.llm && self.approve
    }
}

/// A type as the user wrote it in source code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TypeRef {
    /// A named type: `String`, `Ticket`, `Decision`.
    Named { name: Ident, span: Span },

    /// A qualified type resolved through an import alias:
    /// `policy.Receipt`, `types.Verdict`. The `alias` identifier
    /// must bind to a Corvid `.cor` file import; the resolver
    /// looks up `name` inside that imported module's exported
    /// symbol table. Introduced by `lang-cor-imports-basic-parse`;
    /// full resolution lands in `lang-cor-imports-basic-resolve`.
    Qualified {
        alias: Ident,
        name: Ident,
        span: Span,
    },

    /// A generic application: `List[Order]`, `Map[String, Int]`.
    Generic {
        name: Ident,
        args: Vec<TypeRef>,
        span: Span,
    },

    /// `Weak<T>` or `Weak<T, {tool_call, llm}>`.
    Weak {
        inner: Box<TypeRef>,
        effects: Option<WeakEffectRow>,
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
            | TypeRef::Qualified { span, .. }
            | TypeRef::Generic { span, .. }
            | TypeRef::Weak { span, .. }
            | TypeRef::Function { span, .. } => *span,
        }
    }
}

/// A parameter to a function, tool, agent, or prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Param {
    pub name: Ident,
    pub ty: TypeRef,
    #[serde(default)]
    pub ownership: Option<OwnershipAnnotation>,
    pub span: Span,
}

/// Ownership contract on an FFI-visible boundary type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnershipAnnotation {
    pub mode: OwnershipMode,
    #[serde(default)]
    pub lifetime: Option<String>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OwnershipMode {
    Owned,
    Borrowed,
    Shared,
    Static,
}

/// A field in a struct-like type declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    pub name: Ident,
    pub ty: TypeRef,
    pub span: Span,
}
