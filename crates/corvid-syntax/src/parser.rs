//! Parser for Corvid expressions.
//!
//! Parser implementation for expressions, statements, and declarations.
//!
//! Technique: recursive descent for structural rules + Pratt-style
//! precedence climbing for binary operators.

use crate::errors::{ParseError, ParseErrorKind};
use crate::token::{TokKind, Token};
use corvid_ast::{
    AdversarialSpec, AgentDecl, Backoff, BackpressurePolicy, BinaryOp, Block, Decl,
    DimensionDecl, DimensionValue, Effect, EffectConstraint, EffectDecl, EffectRef, EffectRow,
    EnsembleSpec, EvalAssert, EvalDecl, Expr, ExtendDecl, ExtendMethod, ExtendMethodKind, Field,
    File, Ident, ImportDecl, ImportSource, Literal, ModelDecl, ModelField, Param,
    ProgressiveChain, ProgressiveStage, PromptDecl, PromptStreamSettings, RolloutSpec, RouteArm,
    RoutePattern, RouteTable, Span, Stmt, ToolDecl, TypeDecl, TypeRef, UnaryOp, Visibility,
    VoteStrategy, WeakEffect, WeakEffectRow,
};

#[derive(Debug, Clone, PartialEq)]
pub enum ReplItem {
    Decl(Decl),
    Stmt(Stmt),
    Expr(Expr),
}

/// Parse a full expression from a token stream.
///
/// The token stream is typically produced by [`crate::lex`]. Structural
/// tokens (`Newline`, `Indent`, `Dedent`, `Eof`) terminate the expression
/// — the parser stops before them and leaves them for the caller.
pub fn parse_expr(tokens: &[Token]) -> Result<Expr, ParseError> {
    let mut p = Parser::new(tokens);
    let expr = p.parse_expr()?;
    Ok(expr)
}

/// Parse a full `.cor` file into a `File` AST.
///
/// Errors are collected — a broken declaration reports an error and the
/// parser recovers to the next top-level keyword before continuing.
pub fn parse_file(tokens: &[Token]) -> (File, Vec<ParseError>) {
    let mut p = Parser::new(tokens);
    let file = p.parse_file_inner();
    (file, p.errors)
}

/// Parse a single REPL turn as either a declaration, a statement, or
/// an expression. Classification uses first-token lookahead so errors
/// stay local and predictable.
pub fn parse_repl_input(tokens: &[Token]) -> Result<ReplItem, Vec<ParseError>> {
    let mut p = Parser::new(tokens);
    p.skip_newlines();
    let item = match p.parse_repl_item() {
        Ok(item) => item,
        Err(err) => return Err(vec![err]),
    };
    p.skip_newlines();
    if !matches!(p.peek(), TokKind::Eof) {
        p.errors.push(ParseError {
            kind: ParseErrorKind::UnexpectedToken {
                got: describe_token(p.peek()),
                expected: "end of input".into(),
            },
            span: p.peek_span(),
        });
    }
    if p.errors.is_empty() {
        Ok(item)
    } else {
        Err(p.errors)
    }
}

/// Parse a top-level block (expects `Indent`, statements, `Dedent`).
///
/// Returns a `Block` plus any errors encountered. Errors are collected:
/// parsing does not stop at the first problem.
pub fn parse_block(tokens: &[Token]) -> (Block, Vec<ParseError>) {
    let mut p = Parser::new(tokens);
    let mut errors = Vec::new();
    match p.parse_indented_block() {
        Ok(block) => {
            errors.extend(p.errors);
            (block, errors)
        }
        Err(e) => {
            errors.push(e);
            errors.extend(p.errors);
            (
                Block {
                    stmts: Vec::new(),
                    span: Span::new(0, 0),
                },
                errors,
            )
        }
    }
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    /// Errors collected during statement/block parsing. Expression-level
    /// errors are fatal and returned via `Result`.
    errors: Vec<ParseError>,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self {
            tokens,
            pos: 0,
            errors: Vec::new(),
        }
    }

    /// Peek at the next token without consuming.
    fn peek(&self) -> &TokKind {
        self.tokens
            .get(self.pos)
            .map(|t| &t.kind)
            .unwrap_or(&TokKind::Eof)
    }

    /// Current token's span (or a zero-width span at EOF).
    fn peek_span(&self) -> Span {
        self.tokens
            .get(self.pos)
            .map(|t| t.span)
            .unwrap_or(Span::new(0, 0))
    }

    fn prev_span(&self) -> Span {
        self.pos
            .checked_sub(1)
            .and_then(|idx| self.tokens.get(idx))
            .map(|t| t.span)
            .unwrap_or(Span::new(0, 0))
    }

    /// Consume the next token.
    fn bump(&mut self) -> &'a Token {
        let tok = &self.tokens[self.pos];
        self.pos += 1;
        tok
    }

    fn at_end(&self) -> bool {
        matches!(
            self.peek(),
            TokKind::Eof | TokKind::Newline | TokKind::Indent | TokKind::Dedent
        )
    }


    fn expect_ident(&mut self) -> Result<(String, Span), ParseError> {
        let span = self.peek_span();
        match self.peek().clone() {
            TokKind::Ident(name) => {
                self.bump();
                Ok((name, span))
            }
            other => Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(&other),
                    expected: "an identifier".into(),
                },
                span,
            }),
        }
    }

    fn expect(&mut self, kind: TokKind, description: &str) -> Result<Span, ParseError> {
        let span = self.peek_span();
        if std::mem::discriminant(self.peek()) == std::mem::discriminant(&kind) {
            self.bump();
            Ok(span)
        } else {
            Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(self.peek()),
                    expected: description.into(),
                },
                span,
            })
        }
    }

    /// Skip tokens until we hit the next `Newline`, `Dedent`, or `Eof`.
    /// Used for error recovery — consume the broken statement so parsing
    /// can continue with the next line.
    fn sync_to_statement_boundary(&mut self) {
        while !matches!(
            self.peek(),
            TokKind::Newline | TokKind::Dedent | TokKind::Eof
        ) {
            self.bump();
        }
        // Consume the newline itself if present, so the next statement starts clean.
        if matches!(self.peek(), TokKind::Newline) {
            self.bump();
        }
    }


    fn expect_newline(&mut self) -> Result<(), ParseError> {
        match self.peek() {
            TokKind::Newline => {
                self.bump();
                Ok(())
            }
            TokKind::Eof | TokKind::Dedent => Ok(()),
            other => Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(other),
                    expected: "end of line".into(),
                },
                span: self.peek_span(),
            }),
        }
    }

    // ------------------------------------------------------------
    // File and declaration parsing.
    // ------------------------------------------------------------

    fn parse_file_inner(&mut self) -> File {
        let start_span = self.peek_span();
        let mut decls = Vec::new();

        // Skip any leading newlines (files may start with a blank line).
        self.skip_newlines();

        while !matches!(self.peek(), TokKind::Eof) {
            match self.parse_decl() {
                Ok(d) => decls.push(d),
                Err(e) => {
                    self.errors.push(e);
                    self.sync_to_next_decl();
                }
            }
            self.skip_newlines();
        }

        let end_span = self.peek_span();
        File {
            decls,
            span: start_span.merge(end_span),
        }
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek(), TokKind::Newline) {
            self.bump();
        }
    }

    fn parse_repl_item(&mut self) -> Result<ReplItem, ParseError> {
        if matches!(
            self.peek(),
            TokKind::KwImport
                | TokKind::KwType
                | TokKind::KwTool
                | TokKind::KwPrompt
                | TokKind::KwEval
                | TokKind::KwAgent
                | TokKind::KwExtend
                | TokKind::KwEffect
                | TokKind::KwModel
                | TokKind::At
        ) {
            return self.parse_decl().map(ReplItem::Decl);
        }

        if self.starts_stmt() {
            return self.parse_stmt().map(ReplItem::Stmt);
        }

        let expr = self.parse_expr()?;
        self.expect_newline()?;
        Ok(ReplItem::Expr(expr))
    }

    fn starts_stmt(&self) -> bool {
        match self.peek() {
            TokKind::KwReturn
            | TokKind::KwYield
            | TokKind::KwIf
            | TokKind::KwFor
            | TokKind::KwApprove
            | TokKind::KwBreak
            | TokKind::KwContinue
            | TokKind::KwPass => true,
            TokKind::Ident(_) => matches!(
                self.tokens.get(self.pos + 1).map(|t| &t.kind),
                Some(TokKind::Assign)
            ),
            _ => false,
        }
    }

    /// Skip tokens until we reach a token that could start a declaration
    /// (or EOF). Used after a parse error at the top level.
    fn sync_to_next_decl(&mut self) {
        loop {
            match self.peek() {
                TokKind::KwImport
                | TokKind::KwType
                | TokKind::KwTool
                | TokKind::KwPrompt
                | TokKind::KwEval
                | TokKind::KwAgent
                | TokKind::KwEffect
                | TokKind::KwModel
                | TokKind::At
                | TokKind::KwExtend
                | TokKind::Eof => return,
                _ => {
                    self.bump();
                }
            }
        }
    }


    fn peek_ident_is(&self, expected: &str) -> bool {
        matches!(self.peek(), TokKind::Ident(name) if name == expected)
    }

    fn expect_contextual_ident(&mut self, expected: &str) -> Result<Span, ParseError> {
        let span = self.peek_span();
        match self.peek() {
            TokKind::Ident(name) if name == expected => {
                self.bump();
                Ok(span)
            }
            other => Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(other),
                    expected: format!("`{expected}`"),
                },
                span,
            }),
        }
    }



}

fn describe_token(t: &TokKind) -> String {
    match t {
        TokKind::Ident(s) => format!("identifier `{s}`"),
        TokKind::Int(n) => format!("integer `{n}`"),
        TokKind::Float(f) => format!("float `{f}`"),
        TokKind::StringLit(s) => format!("string `\"{s}\"`"),
        TokKind::Eof => "end of input".into(),
        other => format!("{other:?}"),
    }
}


mod decl;
mod effect_row;
mod expr;
mod literals;
mod prompt;
mod replay_expr;
mod stmt;
mod types;

#[cfg(test)]
mod tests;
