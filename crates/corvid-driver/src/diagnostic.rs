//! Unified `Diagnostic` type for every compiler-stage error.
//!
//! The driver collects errors from lex, parse, resolve, and typecheck
//! stages, each with its own error type. This module normalizes them
//! through `From` impls into one `Diagnostic` shape so CLI, REPL,
//! and programmatic consumers see a single renderable error form.
//!
//! Extracted from `lib.rs` as part of Phase 20i responsibility
//! decomposition (20i-audit-driver-a).

use corvid_ast::Span;
use corvid_resolve::ResolveError;
use corvid_syntax::errors::{LexError, ParseError};
use corvid_types::TypeError;
use std::fmt;
use std::path::Path;

/// A unified diagnostic from any compiler stage, with a span that can be
/// rendered against the original source.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub span: Span,
    pub message: String,
    pub hint: Option<String>,
}

impl Diagnostic {
    pub fn render(&self, source_path: &Path, source: &str) -> String {
        let (line, col) = line_col_of(source, self.span.start);
        let mut out = format!(
            "{}:{}:{}: error: {}",
            source_path.display(),
            line,
            col,
            self.message
        );
        if let Some(h) = &self.hint {
            out.push_str("\n  help: ");
            out.push_str(h);
        }
        out
    }
}

impl From<LexError> for Diagnostic {
    fn from(e: LexError) -> Self {
        Diagnostic {
            span: e.span,
            message: e.kind.to_string(),
            hint: None,
        }
    }
}

impl From<ParseError> for Diagnostic {
    fn from(e: ParseError) -> Self {
        Diagnostic {
            span: e.span,
            message: e.kind.to_string(),
            hint: None,
        }
    }
}

impl From<ResolveError> for Diagnostic {
    fn from(e: ResolveError) -> Self {
        Diagnostic {
            span: e.span,
            message: e.kind.to_string(),
            hint: None,
        }
    }
}

impl From<TypeError> for Diagnostic {
    fn from(e: TypeError) -> Self {
        let hint = e.hint();
        let message = e.message();
        Diagnostic {
            span: e.span,
            message,
            hint,
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}..{}] {}", self.span.start, self.span.end, self.message)?;
        if let Some(h) = &self.hint {
            write!(f, "\n  help: {h}")?;
        }
        Ok(())
    }
}

/// Convert a byte offset into 1-based (line, column) coordinates.
///
/// Columns count Unicode characters, not bytes. Lines split on `\n`.
pub(crate) fn line_col_of(source: &str, offset: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut col = 1usize;
    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Render a list of diagnostics as one concatenated string plus a summary
/// line. Used by the CLI to print every error in one pass.
pub fn summarize_diagnostics(
    diags: &[Diagnostic],
    source_path: &Path,
    source: &str,
) -> String {
    let mut out = String::new();
    for d in diags {
        out.push_str(&d.render(source_path, source));
        out.push('\n');
    }
    out.push_str(&format!("\n{} error(s) found.\n", diags.len()));
    out
}
