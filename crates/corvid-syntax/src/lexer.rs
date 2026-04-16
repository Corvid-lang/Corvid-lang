//! Hand-rolled lexer for Corvid.
//!
//! Produces a token stream including Python-style `Indent`, `Dedent`,
//! and `Newline` structural tokens. See `ARCHITECTURE.md` §4.

use crate::errors::{LexError, LexErrorKind};
use crate::token::{TokKind, Token};
use corvid_ast::Span;

/// Lex a full source string. Returns tokens on success or a list of errors.
pub fn lex(source: &str) -> Result<Vec<Token>, Vec<LexError>> {
    let mut lx = Lexer::new(source);
    lx.run();
    if lx.errors.is_empty() {
        Ok(lx.tokens)
    } else {
        Err(lx.errors)
    }
}

struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    tokens: Vec<Token>,
    errors: Vec<LexError>,
    /// Nesting depth of `(`, `[`. Newlines inside brackets are ignored.
    bracket_depth: i32,
    /// Stack of current indentation column widths. Starts with `[0]`.
    indent_stack: Vec<usize>,
    /// True once we've emitted any non-structural token on the current
    /// logical line. Controls whether a `\n` produces a `Newline` token.
    had_content_on_line: bool,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            pos: 0,
            tokens: Vec::new(),
            errors: Vec::new(),
            bracket_depth: 0,
            indent_stack: vec![0],
            had_content_on_line: false,
        }
    }

    fn run(&mut self) {
        // Handle indentation of the very first line.
        self.process_line_start();

        while self.pos < self.bytes.len() {
            let c = self.bytes[self.pos];
            match c {
                // Inline whitespace: just skip. `\r` is silently absorbed
                // so CRLF-encoded files (Windows defaults, Git autocrlf)
                // lex identically to LF-only sources.
                b' ' | b'\t' | b'\r' => {
                    self.pos += 1;
                }
                b'\n' => {
                    let nl_start = self.pos;
                    self.pos += 1;
                    if self.bracket_depth == 0 {
                        if self.had_content_on_line {
                            self.emit_structural(
                                TokKind::Newline,
                                Span::new(nl_start, self.pos),
                            );
                            self.had_content_on_line = false;
                        }
                        self.process_line_start();
                    }
                }
                b'#' => {
                    // Line comment: skip to end of line (not consuming \n).
                    while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                }
                b'0'..=b'9' => self.lex_number(),
                b'"' => self.lex_string(),
                c if is_ident_start(c) => self.lex_ident_or_kw(),
                _ => self.lex_punct(),
            }
        }

        // End-of-file: finish any open line and dedent back to column 0.
        if self.had_content_on_line {
            self.emit_structural(TokKind::Newline, Span::new(self.pos, self.pos));
            self.had_content_on_line = false;
        }
        while self.indent_stack.len() > 1 {
            self.emit_structural(TokKind::Dedent, Span::new(self.pos, self.pos));
            self.indent_stack.pop();
        }
        self.emit_structural(TokKind::Eof, Span::new(self.pos, self.pos));
    }

    /// Called at the start of each physical line (after `\n`) and at file
    /// start. Measures indentation and emits `Indent`/`Dedent` tokens.
    /// Blank or comment-only lines are skipped.
    fn process_line_start(&mut self) {
        // Tolerate a leading `\r` from a CRLF blank line.
        while self.pos < self.bytes.len() && self.bytes[self.pos] == b'\r' {
            self.pos += 1;
        }

        let start = self.pos;
        let mut indent = 0usize;
        let mut had_tab = false;

        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b' ' => {
                    indent += 1;
                    self.pos += 1;
                }
                b'\t' => {
                    had_tab = true;
                    self.pos += 1;
                }
                _ => break,
            }
        }

        // Blank or comment-only line — don't affect indentation.
        if self.pos >= self.bytes.len() {
            return;
        }
        match self.bytes[self.pos] {
            b'\n' | b'\r' | b'#' => return,
            _ => {}
        }

        if had_tab {
            self.errors.push(LexError {
                kind: LexErrorKind::TabIndentation,
                span: Span::new(start, self.pos),
            });
        }

        let current = *self.indent_stack.last().expect("indent stack never empty");
        if indent > current {
            self.indent_stack.push(indent);
            self.emit_structural(TokKind::Indent, Span::new(start, self.pos));
        } else if indent < current {
            while *self.indent_stack.last().unwrap() > indent {
                self.indent_stack.pop();
                self.emit_structural(TokKind::Dedent, Span::new(self.pos, self.pos));
            }
            if *self.indent_stack.last().unwrap() != indent {
                self.errors.push(LexError {
                    kind: LexErrorKind::InconsistentDedent,
                    span: Span::new(start, self.pos),
                });
            }
        }
    }

    fn emit(&mut self, kind: TokKind, span: Span) {
        self.had_content_on_line = true;
        self.tokens.push(Token::new(kind, span));
    }

    fn emit_structural(&mut self, kind: TokKind, span: Span) {
        self.tokens.push(Token::new(kind, span));
    }

    fn lex_number(&mut self) {
        let start = self.pos;
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        let is_float = self.pos + 1 < self.bytes.len()
            && self.bytes[self.pos] == b'.'
            && self.bytes[self.pos + 1].is_ascii_digit();
        if is_float {
            self.pos += 1; // consume dot
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
            let text = &self.src[start..self.pos];
            match text.parse::<f64>() {
                Ok(v) => self.emit(TokKind::Float(v), Span::new(start, self.pos)),
                Err(_) => self.errors.push(LexError {
                    kind: LexErrorKind::InvalidNumber(text.to_string()),
                    span: Span::new(start, self.pos),
                }),
            }
        } else {
            let text = &self.src[start..self.pos];
            match text.parse::<i64>() {
                Ok(v) => self.emit(TokKind::Int(v), Span::new(start, self.pos)),
                Err(_) => self.errors.push(LexError {
                    kind: LexErrorKind::InvalidNumber(text.to_string()),
                    span: Span::new(start, self.pos),
                }),
            }
        }
    }

    fn lex_string(&mut self) {
        // Triple-quoted multi-line string: `"""..."""`.
        if self.pos + 2 < self.bytes.len()
            && self.bytes[self.pos + 1] == b'"'
            && self.bytes[self.pos + 2] == b'"'
        {
            self.lex_triple_string();
        } else {
            self.lex_single_string();
        }
    }

    fn lex_single_string(&mut self) {
        let start = self.pos;
        self.pos += 1; // consume opening "
        let mut contents = String::new();
        loop {
            if self.pos >= self.bytes.len() {
                self.errors.push(LexError {
                    kind: LexErrorKind::UnterminatedString,
                    span: Span::new(start, self.pos),
                });
                return;
            }
            let c = self.bytes[self.pos];
            match c {
                b'"' => {
                    self.pos += 1;
                    self.emit(TokKind::StringLit(contents), Span::new(start, self.pos));
                    return;
                }
                b'\n' => {
                    // Single-line strings may not span lines.
                    self.errors.push(LexError {
                        kind: LexErrorKind::UnterminatedString,
                        span: Span::new(start, self.pos),
                    });
                    return;
                }
                b'\\' => {
                    if let Some(ch) = self.consume_escape(start) {
                        contents.push(ch);
                    } else {
                        return;
                    }
                }
                _ => {
                    let ch = self.src[self.pos..].chars().next().unwrap();
                    contents.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
    }

    fn lex_triple_string(&mut self) {
        let start = self.pos;
        self.pos += 3; // consume opening """
        let mut contents = String::new();
        loop {
            if self.pos >= self.bytes.len() {
                self.errors.push(LexError {
                    kind: LexErrorKind::UnterminatedString,
                    span: Span::new(start, self.pos),
                });
                return;
            }
            // Closing """
            if self.pos + 2 < self.bytes.len()
                && self.bytes[self.pos] == b'"'
                && self.bytes[self.pos + 1] == b'"'
                && self.bytes[self.pos + 2] == b'"'
            {
                self.pos += 3;
                self.emit(TokKind::StringLit(contents), Span::new(start, self.pos));
                return;
            }
            // Special case: closing """ at exact EOF.
            if self.pos + 3 == self.bytes.len()
                && self.bytes[self.pos] == b'"'
                && self.bytes[self.pos + 1] == b'"'
                && self.bytes[self.pos + 2] == b'"'
            {
                self.pos += 3;
                self.emit(TokKind::StringLit(contents), Span::new(start, self.pos));
                return;
            }

            let c = self.bytes[self.pos];
            if c == b'\\' {
                if let Some(ch) = self.consume_escape(start) {
                    contents.push(ch);
                } else {
                    return;
                }
            } else {
                let ch = self.src[self.pos..].chars().next().unwrap();
                contents.push(ch);
                self.pos += ch.len_utf8();
            }
        }
    }

    /// Consume a `\x` escape, returning the decoded character. On an invalid
    /// escape, record an error but still return the raw character so lexing
    /// can continue. Returns `None` only on EOF after the backslash.
    fn consume_escape(&mut self, _string_start: usize) -> Option<char> {
        let esc_start = self.pos;
        self.pos += 1; // consume backslash
        if self.pos >= self.bytes.len() {
            self.errors.push(LexError {
                kind: LexErrorKind::UnterminatedString,
                span: Span::new(esc_start, self.pos),
            });
            return None;
        }
        let esc = self.bytes[self.pos];
        let ch = match esc {
            b'n' => '\n',
            b't' => '\t',
            b'r' => '\r',
            b'\\' => '\\',
            b'"' => '"',
            b'0' => '\0',
            other => {
                self.errors.push(LexError {
                    kind: LexErrorKind::InvalidEscape(other as char),
                    span: Span::new(esc_start, self.pos + 1),
                });
                other as char
            }
        };
        self.pos += 1;
        Some(ch)
    }

    fn lex_ident_or_kw(&mut self) {
        let start = self.pos;
        while self.pos < self.bytes.len() && is_ident_continue(self.bytes[self.pos]) {
            self.pos += 1;
        }
        let text = &self.src[start..self.pos];
        let kind = TokKind::keyword_from(text)
            .unwrap_or_else(|| TokKind::Ident(text.to_string()));
        self.emit(kind, Span::new(start, self.pos));
    }

    fn lex_punct(&mut self) {
        let start = self.pos;
        let c = self.bytes[self.pos];
        let (kind, len): (TokKind, usize) = match c {
            b'(' => {
                self.bracket_depth += 1;
                (TokKind::LParen, 1)
            }
            b')' => {
                self.bracket_depth -= 1;
                (TokKind::RParen, 1)
            }
            b'[' => {
                self.bracket_depth += 1;
                (TokKind::LBracket, 1)
            }
            b']' => {
                self.bracket_depth -= 1;
                (TokKind::RBracket, 1)
            }
            b'{' => (TokKind::LBrace, 1),
            b'}' => (TokKind::RBrace, 1),
            b':' => (TokKind::Colon, 1),
            b',' => (TokKind::Comma, 1),
            b'.' => (TokKind::Dot, 1),
            b'?' => (TokKind::Question, 1),
            b'+' => (TokKind::Plus, 1),
            b'*' => (TokKind::Star, 1),
            b'/' => (TokKind::Slash, 1),
            b'%' => (TokKind::Percent, 1),
            b'-' => {
                if self.peek(1) == Some(b'>') {
                    (TokKind::Arrow, 2)
                } else {
                    (TokKind::Minus, 1)
                }
            }
            b'=' => {
                if self.peek(1) == Some(b'=') {
                    (TokKind::Eq, 2)
                } else {
                    (TokKind::Assign, 1)
                }
            }
            b'!' => {
                if self.peek(1) == Some(b'=') {
                    (TokKind::NotEq, 2)
                } else {
                    self.errors.push(LexError {
                        kind: LexErrorKind::UnexpectedChar('!'),
                        span: Span::new(start, start + 1),
                    });
                    self.pos += 1;
                    return;
                }
            }
            b'<' => {
                if self.peek(1) == Some(b'=') {
                    (TokKind::LtEq, 2)
                } else {
                    (TokKind::Lt, 1)
                }
            }
            b'>' => {
                if self.peek(1) == Some(b'=') {
                    (TokKind::GtEq, 2)
                } else {
                    (TokKind::Gt, 1)
                }
            }
            _ => {
                let ch = self.src[self.pos..].chars().next().unwrap_or('?');
                self.errors.push(LexError {
                    kind: LexErrorKind::UnexpectedChar(ch),
                    span: Span::new(start, start + ch.len_utf8()),
                });
                self.pos += ch.len_utf8();
                return;
            }
        };
        self.pos += len;
        self.emit(kind, Span::new(start, start + len));
    }

    fn peek(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }
}

fn is_ident_start(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphabetic()
}

fn is_ident_continue(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphanumeric()
}
