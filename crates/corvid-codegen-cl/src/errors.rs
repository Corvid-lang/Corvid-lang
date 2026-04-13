//! Errors the Cranelift codegen produces.
//!
//! Distinct from `corvid-vm::InterpError` — those fire at runtime inside
//! the interpreter. These fire at compile time when codegen refuses a
//! construct (too early in the slice plan) or when Cranelift itself
//! raises.

use corvid_ast::Span;
use std::fmt;

#[derive(Debug, Clone)]
pub struct CodegenError {
    pub kind: CodegenErrorKind,
    pub span: Span,
}

impl CodegenError {
    pub fn not_supported(reason: impl Into<String>, span: Span) -> Self {
        Self {
            kind: CodegenErrorKind::NotSupported(reason.into()),
            span,
        }
    }

    pub fn cranelift(message: impl Into<String>, span: Span) -> Self {
        Self {
            kind: CodegenErrorKind::Cranelift(message.into()),
            span,
        }
    }

    pub fn link(message: impl Into<String>) -> Self {
        Self {
            kind: CodegenErrorKind::Link(message.into()),
            span: Span::new(0, 0),
        }
    }

    pub fn io(message: impl Into<String>) -> Self {
        Self {
            kind: CodegenErrorKind::Io(message.into()),
            span: Span::new(0, 0),
        }
    }
}

#[derive(Debug, Clone)]
pub enum CodegenErrorKind {
    /// The IR construct is not supported by this codegen backend at the
    /// current slice. Each arm of 12a's lowering switches carries a
    /// message pointing to the slice that will add support.
    NotSupported(String),

    /// Cranelift itself raised during codegen (invalid IR, verifier
    /// failure, ISA miscompile). Should be rare; report and file a bug.
    Cranelift(String),

    /// System linker invocation failed.
    Link(String),

    /// Filesystem error writing the object file or the final binary.
    Io(String),
}

impl fmt::Display for CodegenErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotSupported(msg) => write!(f, "native codegen does not yet support: {msg}"),
            Self::Cranelift(msg) => write!(f, "cranelift error: {msg}"),
            Self::Link(msg) => write!(f, "linker error: {msg}"),
            Self::Io(msg) => write!(f, "io error: {msg}"),
        }
    }
}

impl fmt::Display for CodegenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}..{}] {}", self.span.start, self.span.end, self.kind)
    }
}

impl std::error::Error for CodegenError {}
