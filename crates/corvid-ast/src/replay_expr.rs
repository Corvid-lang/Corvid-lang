//! AST for the `replay` expression — Corvid's language-level
//! primitive for ingesting a recorded JSONL trace and dispatching
//! on its event stream.
//!
//! Grammar (v0, Phase 21 slice 21-inv-E-1):
//!
//! ```text
//! replay_expr    := 'replay' expr ':' INDENT arm+ else_arm DEDENT
//! arm            := 'when' event_pattern '->' expr NEWLINE
//! else_arm       := 'else' expr NEWLINE
//! event_pattern  := 'llm'     '(' STRING ')'
//!                 | 'tool'    '(' STRING ',' arg_pattern ')'
//!                 | 'approve' '(' STRING ')'
//! arg_pattern    := '_' | STRING
//! ```
//!
//! Resolver-level work (trace-id locals, name-resolving the event
//! pattern into a structured `TraceEventPattern`) lands in slice
//! 21-inv-E-2. This module only defines the shape.

use crate::expr::Expr;
use crate::span::Span;
use serde::{Deserialize, Serialize};

/// One arm of a replay block: `when <pattern> -> <body>`. The
/// `else` fallback is represented separately on [`crate::expr::Expr::Replay`]
/// so the parser + checker can treat it as required rather than
/// one-of-many.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayArm {
    pub pattern: ReplayPattern,
    pub body: Expr,
    pub span: Span,
}

/// A pattern that matches one kind of recorded trace event.
/// Parsed as a surface form; the checker (slice 21-inv-E-3) will
/// refine this into a typed `TraceEventPattern` after resolving
/// the string literals against tool/prompt/approval symbols.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReplayPattern {
    /// `llm("<prompt-name>")` — matches a `TraceEvent::LlmCall` /
    /// `LlmResult` pair whose prompt name equals `prompt`.
    Llm { prompt: String, span: Span },
    /// `tool("<tool-name>", <arg_pattern>)` — matches a
    /// `ToolCall` / `ToolResult` pair for `tool` whose first
    /// argument matches `arg`.
    Tool {
        tool: String,
        arg: ToolArgPattern,
        span: Span,
    },
    /// `approve("<label>")` — matches an `ApprovalRequest` /
    /// `ApprovalResponse` pair for the named approval site.
    Approve { label: String, span: Span },
}

impl ReplayPattern {
    pub fn span(&self) -> Span {
        match self {
            Self::Llm { span, .. }
            | Self::Tool { span, .. }
            | Self::Approve { span, .. } => *span,
        }
    }
}

/// A tool-arg pattern. Only `_` (match-anything) and a string
/// literal (match-this-exact-value) are supported in v0. Broader
/// structural patterns can land later without breaking this
/// surface form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolArgPattern {
    /// `_` — wildcard; matches any argument value.
    Wildcard { span: Span },
    /// `"..."` — string-equality match.
    StringLit { value: String, span: Span },
}

impl ToolArgPattern {
    pub fn span(&self) -> Span {
        match self {
            Self::Wildcard { span } | Self::StringLit { span, .. } => *span,
        }
    }
}
