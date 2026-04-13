//! Runtime errors raised by `corvid-runtime`.
//!
//! Distinct from `corvid-vm::InterpError` — those are interpreter-level
//! (type mismatch, division by zero). These cover the runtime boundary:
//! tool dispatch, approval flow, LLM adapters, tracing.
//!
//! The interpreter wraps `RuntimeError` into `InterpError::Runtime(...)`
//! when it bubbles up to user code; downstream renderers can pattern-match
//! either form.

use std::fmt;

#[derive(Debug, Clone)]
pub enum RuntimeError {
    /// A tool name was called that has no registered handler.
    UnknownTool(String),

    /// A tool handler returned an error.
    ToolFailed { tool: String, message: String },

    /// A prompt name was called that has no registered template / handler.
    UnknownPrompt(String),

    /// No LLM adapter is registered that can handle the requested model.
    NoAdapter(String),

    /// An LLM adapter returned an error (HTTP, parse, etc.).
    AdapterFailed { adapter: String, message: String },

    /// Approval was denied (user said no, programmatic approver returned deny).
    ApprovalDenied { action: String },

    /// Approval flow failed for a non-deny reason (timeout, IO).
    ApprovalFailed(String),

    /// Wire-format conversion failed (Value <-> JSON marshalling).
    Marshal(String),

    /// No model is configured for an LLM call. Hint to set `CORVID_MODEL`
    /// or pass `model=` per call.
    NoModelConfigured,

    /// Catch-all. Prefer adding a dedicated variant.
    Other(String),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTool(name) => write!(f, "no handler registered for tool `{name}`"),
            Self::ToolFailed { tool, message } => {
                write!(f, "tool `{tool}` failed: {message}")
            }
            Self::UnknownPrompt(name) => write!(f, "no prompt named `{name}`"),
            Self::NoAdapter(model) => {
                write!(f, "no LLM adapter registered for model `{model}`")
            }
            Self::AdapterFailed { adapter, message } => {
                write!(f, "LLM adapter `{adapter}` failed: {message}")
            }
            Self::ApprovalDenied { action } => {
                write!(f, "approval denied for `{action}`")
            }
            Self::ApprovalFailed(msg) => write!(f, "approval flow failed: {msg}"),
            Self::Marshal(msg) => write!(f, "value marshalling failed: {msg}"),
            Self::NoModelConfigured => write!(
                f,
                "no LLM model configured. Set CORVID_MODEL, add `default_model = \"...\"` to corvid.toml, or pass `model=` per call."
            ),
            Self::Other(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for RuntimeError {}
