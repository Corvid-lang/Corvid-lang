//! Lexer error types.

use corvid_ast::Span;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct LexError {
    pub kind: LexErrorKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub kind: ParseErrorKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParseErrorKind {
    /// Parser reached EOF but expected more tokens.
    UnexpectedEof,
    /// Found one token; expected something else. `got` is a human-readable
    /// description of what we saw.
    UnexpectedToken {
        got: String,
        expected: String,
    },
    /// `a < b < c` — chained comparison, disallowed in v0.1.
    ChainedComparison,
    /// Something inside parentheses didn't close.
    UnclosedParen,
    /// Something inside brackets didn't close.
    UnclosedBracket,
    /// A block had no statements. Use `pass` for an intentional no-op.
    EmptyBlock,
    /// Expected the start of a block (indent) but found something else.
    ExpectedBlock,
}

impl fmt::Display for ParseErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => write!(f, "unexpected end of input"),
            Self::UnexpectedToken { got, expected } => {
                write!(f, "unexpected token `{got}`, expected {expected}")
            }
            Self::ChainedComparison => write!(
                f,
                "chained comparisons are not allowed (use `and` to combine)"
            ),
            Self::UnclosedParen => write!(f, "unclosed `(`"),
            Self::UnclosedBracket => write!(f, "unclosed `[`"),
            Self::EmptyBlock => {
                write!(f, "block is empty (use `pass` for an intentional no-op)")
            }
            Self::ExpectedBlock => write!(f, "expected an indented block here"),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}..{}] {}", self.span.start, self.span.end, self.kind)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LexErrorKind {
    /// A character we don't recognize (e.g. `!` alone, `$`, non-ASCII symbol).
    UnexpectedChar(char),

    /// A string literal was opened but never closed.
    UnterminatedString,

    /// A `\x` escape where `x` is not a known escape character.
    InvalidEscape(char),

    /// A tab character was used for indentation.
    TabIndentation,

    /// A dedent doesn't land on any previous indent level.
    InconsistentDedent,

    /// A number literal couldn't be parsed.
    InvalidNumber(String),
}

impl fmt::Display for LexErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedChar(c) => write!(f, "unexpected character `{c}`"),
            Self::UnterminatedString => write!(f, "unterminated string literal"),
            Self::InvalidEscape(c) => write!(f, "invalid escape sequence `\\{c}`"),
            Self::TabIndentation => {
                write!(f, "tab character used for indentation (use spaces)")
            }
            Self::InconsistentDedent => {
                write!(f, "dedent does not match any outer indent level")
            }
            Self::InvalidNumber(s) => write!(f, "invalid number literal `{s}`"),
        }
    }
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}..{}] {}", self.span.start, self.span.end, self.kind)
    }
}
