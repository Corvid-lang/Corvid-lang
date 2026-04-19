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
use crate::span::{Ident, Span};
use serde::{Deserialize, Serialize};

/// One arm of a replay block: `when <pattern> [as <ident>] -> <body>`.
/// The `else` fallback is represented separately on
/// [`crate::expr::Expr::Replay`] so the parser + checker can treat
/// it as required rather than one-of-many.
///
/// `capture`, when `Some`, binds the matched recorded event's
/// payload as a local visible in `body`. For `llm(...)` arms the
/// payload is the recorded `LlmResult` value; for `tool(...)` arms
/// the payload is the recorded `ToolResult`; for `approve(...)`
/// arms the payload is the recorded approval verdict (Bool).
/// Scope-opening + resolution land in the resolver slice
/// (21-inv-E-2b).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayArm {
    pub pattern: ReplayPattern,
    #[serde(default)]
    pub capture: Option<Ident>,
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

/// A tool-arg pattern. Three shapes:
/// - `_` matches any argument without binding it.
/// - `"..."` matches a specific string literal.
/// - a bare identifier `ticket_id` matches any value **and** binds
///   it as a local visible in the arm body (same treatment the
///   `as <ident>` tail gives for whole-event captures, applied
///   per-arg for tools).
///
/// Broader structural patterns can land later without breaking
/// this surface form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolArgPattern {
    /// `_` — wildcard; matches any argument value without binding.
    Wildcard { span: Span },
    /// `"..."` — string-equality match.
    StringLit { value: String, span: Span },
    /// `<ident>` — capture; matches any argument value and binds
    /// it as a local in the arm body. Scope wiring lands in E-2b.
    Capture { name: Ident, span: Span },
}

impl ToolArgPattern {
    pub fn span(&self) -> Span {
        match self {
            Self::Wildcard { span }
            | Self::StringLit { span, .. }
            | Self::Capture { span, .. } => *span,
        }
    }
}
