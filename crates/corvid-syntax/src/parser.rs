//! Parser for Corvid expressions.
//!
//! Phase 3a: expression grammar only. Statements and declarations come next.
//!
//! Technique: recursive descent for structural rules + Pratt-style
//! precedence climbing for binary operators.

use crate::errors::{ParseError, ParseErrorKind};
use crate::token::{TokKind, Token};
use corvid_ast::{
    AgentDecl, Backoff, BinaryOp, Block, Decl, Effect, Expr, ExtendDecl, ExtendMethod,
    ExtendMethodKind, Field, File, Ident, ImportDecl, ImportSource, Literal, Param, PromptDecl,
    Span, Stmt, ToolDecl, TypeDecl, TypeRef, UnaryOp, Visibility,
};

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

    // ------------------------------------------------------------
    // Expression entry point.
    // ------------------------------------------------------------

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_or()
    }

    // or_expr := and_expr ('or' and_expr)*
    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), TokKind::KwOr) {
            self.bump();
            let right = self.parse_and()?;
            let span = left.span().merge(right.span());
            left = Expr::BinOp {
                op: BinaryOp::Or,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    // and_expr := not_expr ('and' not_expr)*
    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_not()?;
        while matches!(self.peek(), TokKind::KwAnd) {
            self.bump();
            let right = self.parse_not()?;
            let span = left.span().merge(right.span());
            left = Expr::BinOp {
                op: BinaryOp::And,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    // not_expr := 'not' not_expr | cmp_expr
    fn parse_not(&mut self) -> Result<Expr, ParseError> {
        if matches!(self.peek(), TokKind::KwNot) {
            let start = self.peek_span().start;
            self.bump();
            let operand = self.parse_not()?;
            let span = Span::new(start, operand.span().end);
            Ok(Expr::UnOp {
                op: UnaryOp::Not,
                operand: Box::new(operand),
                span,
            })
        } else {
            self.parse_cmp()
        }
    }

    // cmp_expr := add_expr (cmp_op add_expr)?
    // chained comparisons (a < b < c) are explicitly rejected.
    fn parse_cmp(&mut self) -> Result<Expr, ParseError> {
        let left = self.parse_add()?;
        let op = match self.peek() {
            TokKind::Eq => Some(BinaryOp::Eq),
            TokKind::NotEq => Some(BinaryOp::NotEq),
            TokKind::Lt => Some(BinaryOp::Lt),
            TokKind::LtEq => Some(BinaryOp::LtEq),
            TokKind::Gt => Some(BinaryOp::Gt),
            TokKind::GtEq => Some(BinaryOp::GtEq),
            _ => None,
        };
        let Some(op) = op else { return Ok(left) };
        self.bump();
        let right = self.parse_add()?;

        // Reject a second comparison operator.
        if matches!(
            self.peek(),
            TokKind::Eq
                | TokKind::NotEq
                | TokKind::Lt
                | TokKind::LtEq
                | TokKind::Gt
                | TokKind::GtEq
        ) {
            return Err(ParseError {
                kind: ParseErrorKind::ChainedComparison,
                span: self.peek_span(),
            });
        }

        let span = left.span().merge(right.span());
        Ok(Expr::BinOp {
            op,
            left: Box::new(left),
            right: Box::new(right),
            span,
        })
    }

    // add_expr := mul_expr (('+' | '-') mul_expr)*
    fn parse_add(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_mul()?;
        loop {
            let op = match self.peek() {
                TokKind::Plus => BinaryOp::Add,
                TokKind::Minus => BinaryOp::Sub,
                _ => return Ok(left),
            };
            self.bump();
            let right = self.parse_mul()?;
            let span = left.span().merge(right.span());
            left = Expr::BinOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
    }

    // mul_expr := unary_expr (('*' | '/' | '%') unary_expr)*
    fn parse_mul(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                TokKind::Star => BinaryOp::Mul,
                TokKind::Slash => BinaryOp::Div,
                TokKind::Percent => BinaryOp::Mod,
                _ => return Ok(left),
            };
            self.bump();
            let right = self.parse_unary()?;
            let span = left.span().merge(right.span());
            left = Expr::BinOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
    }

    // unary_expr := '-' unary_expr | postfix_expr
    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        if matches!(self.peek(), TokKind::Minus) {
            let start = self.peek_span().start;
            self.bump();
            let operand = self.parse_unary()?;
            let span = Span::new(start, operand.span().end);
            Ok(Expr::UnOp {
                op: UnaryOp::Neg,
                operand: Box::new(operand),
                span,
            })
        } else {
            self.parse_postfix()
        }
    }

    // postfix_expr := primary (postfix_op)*
    // postfix_op   := '.' IDENT | '[' expr ']' | '(' args? ')'
    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut target = self.parse_primary()?;
        loop {
            match self.peek() {
                TokKind::Dot => {
                    self.bump();
                    let (field_name, field_span) = self.expect_ident()?;
                    let span = target.span().merge(field_span);
                    target = Expr::FieldAccess {
                        target: Box::new(target),
                        field: Ident::new(field_name, field_span),
                        span,
                    };
                }
                TokKind::LBracket => {
                    self.bump();
                    let idx = self.parse_expr()?;
                    let end_span = self.peek_span();
                    if !matches!(self.peek(), TokKind::RBracket) {
                        return Err(ParseError {
                            kind: ParseErrorKind::UnclosedBracket,
                            span: end_span,
                        });
                    }
                    self.bump();
                    let span = target.span().merge(end_span);
                    target = Expr::Index {
                        target: Box::new(target),
                        index: Box::new(idx),
                        span,
                    };
                }
                TokKind::LParen => {
                    self.bump();
                    let mut args = Vec::new();
                    if !matches!(self.peek(), TokKind::RParen) {
                        args.push(self.parse_expr()?);
                        while matches!(self.peek(), TokKind::Comma) {
                            self.bump();
                            // Allow trailing comma.
                            if matches!(self.peek(), TokKind::RParen) {
                                break;
                            }
                            args.push(self.parse_expr()?);
                        }
                    }
                    let end_span = self.peek_span();
                    if !matches!(self.peek(), TokKind::RParen) {
                        return Err(ParseError {
                            kind: ParseErrorKind::UnclosedParen,
                            span: end_span,
                        });
                    }
                    self.bump();
                    let span = target.span().merge(end_span);
                    target = Expr::Call {
                        callee: Box::new(target),
                        args,
                        span,
                    };
                }
                TokKind::Question => {
                    let question_span = self.peek_span();
                    let target_span = target.span();
                    self.bump();
                    target = Expr::TryPropagate {
                        inner: Box::new(target),
                        span: target_span.merge(question_span),
                    };
                }
                _ => return Ok(target),
            }
        }
    }

    // primary := literal | IDENT | '(' expr ')' | '[' items? ']'
    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        let start_span = self.peek_span();
        match self.peek().clone() {
            TokKind::Int(n) => {
                self.bump();
                Ok(Expr::Literal {
                    value: Literal::Int(n),
                    span: start_span,
                })
            }
            TokKind::Float(f) => {
                self.bump();
                Ok(Expr::Literal {
                    value: Literal::Float(f),
                    span: start_span,
                })
            }
            TokKind::StringLit(s) => {
                self.bump();
                Ok(Expr::Literal {
                    value: Literal::String(s),
                    span: start_span,
                })
            }
            TokKind::KwTrue => {
                self.bump();
                Ok(Expr::Literal {
                    value: Literal::Bool(true),
                    span: start_span,
                })
            }
            TokKind::KwFalse => {
                self.bump();
                Ok(Expr::Literal {
                    value: Literal::Bool(false),
                    span: start_span,
                })
            }
            TokKind::KwNothing => {
                self.bump();
                Ok(Expr::Literal {
                    value: Literal::Nothing,
                    span: start_span,
                })
            }
            TokKind::KwTry => self.parse_try_retry_expr(),
            TokKind::Ident(name) => {
                self.bump();
                Ok(Expr::Ident {
                    name: Ident::new(name, start_span),
                    span: start_span,
                })
            }
            TokKind::LParen => {
                self.bump();
                let inner = self.parse_expr()?;
                let end_span = self.peek_span();
                if !matches!(self.peek(), TokKind::RParen) {
                    return Err(ParseError {
                        kind: ParseErrorKind::UnclosedParen,
                        span: end_span,
                    });
                }
                self.bump();
                Ok(inner)
            }
            TokKind::LBracket => {
                self.bump();
                let mut items = Vec::new();
                if !matches!(self.peek(), TokKind::RBracket) {
                    items.push(self.parse_expr()?);
                    while matches!(self.peek(), TokKind::Comma) {
                        self.bump();
                        if matches!(self.peek(), TokKind::RBracket) {
                            break;
                        }
                        items.push(self.parse_expr()?);
                    }
                }
                let end_span = self.peek_span();
                if !matches!(self.peek(), TokKind::RBracket) {
                    return Err(ParseError {
                        kind: ParseErrorKind::UnclosedBracket,
                        span: end_span,
                    });
                }
                self.bump();
                let span = start_span.merge(end_span);
                Ok(Expr::List { items, span })
            }
            TokKind::Eof | TokKind::Newline | TokKind::Indent | TokKind::Dedent => {
                Err(ParseError {
                    kind: ParseErrorKind::UnexpectedEof,
                    span: start_span,
                })
            }
            other => Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(&other),
                    expected: "an expression".into(),
                },
                span: start_span,
            }),
        }
    }

    fn parse_try_retry_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.peek_span();
        self.bump(); // try
        let body = self.parse_expr()?;
        self.expect(TokKind::KwOn, "`on` after `try` body")?;
        self.expect(TokKind::KwError, "`error` after `on` in retry expression")?;
        self.expect(TokKind::KwRetry, "`retry` in retry expression")?;
        let attempts = self.parse_u64_literal("retry attempt count")?;
        self.expect(TokKind::KwTimes, "`times` after retry count")?;
        self.expect(TokKind::KwBackoff, "`backoff` after retry count")?;

        let backoff = match self.peek() {
            TokKind::KwLinear => {
                self.bump();
                Backoff::Linear(self.parse_u64_literal("linear backoff delay in ms")?)
            }
            TokKind::KwExponential => {
                self.bump();
                Backoff::Exponential(
                    self.parse_u64_literal("exponential backoff base delay in ms")?,
                )
            }
            other => {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: describe_token(other),
                        expected: "`linear <ms>` or `exponential <ms>`".into(),
                    },
                    span: self.peek_span(),
                });
            }
        };

        Ok(Expr::TryRetry {
            body: Box::new(body),
            attempts,
            backoff,
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_u64_literal(&mut self, description: &str) -> Result<u64, ParseError> {
        let span = self.peek_span();
        match self.peek().clone() {
            TokKind::Int(value) if value >= 0 => {
                self.bump();
                Ok(value as u64)
            }
            other => Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(&other),
                    expected: description.into(),
                },
                span,
            }),
        }
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

    // ------------------------------------------------------------
    // Block parsing.
    // ------------------------------------------------------------

    /// Expect `Indent`, then 1+ statements, then `Dedent`.
    fn parse_indented_block(&mut self) -> Result<Block, ParseError> {
        let start_span = self.peek_span();
        if !matches!(self.peek(), TokKind::Indent) {
            return Err(ParseError {
                kind: ParseErrorKind::ExpectedBlock,
                span: start_span,
            });
        }
        self.bump(); // consume Indent

        let mut stmts = Vec::new();
        while !matches!(self.peek(), TokKind::Dedent | TokKind::Eof) {
            match self.parse_stmt() {
                Ok(s) => stmts.push(s),
                Err(e) => {
                    self.errors.push(e);
                    self.sync_to_statement_boundary();
                }
            }
        }
        let end_span = self.peek_span();
        if matches!(self.peek(), TokKind::Dedent) {
            self.bump();
        }

        if stmts.is_empty() {
            self.errors.push(ParseError {
                kind: ParseErrorKind::EmptyBlock,
                span: start_span,
            });
        }

        Ok(Block {
            stmts,
            span: start_span.merge(end_span),
        })
    }

    // ------------------------------------------------------------
    // Statement parsing.
    // ------------------------------------------------------------

    fn parse_stmt(&mut self) -> Result<Stmt, ParseError> {
        // Skip stray newlines (blank lines inside blocks).
        while matches!(self.peek(), TokKind::Newline) {
            self.bump();
        }

        match self.peek() {
            TokKind::KwReturn => self.parse_return_stmt(),
            TokKind::KwIf => self.parse_if_stmt(),
            TokKind::KwFor => self.parse_for_stmt(),
            TokKind::KwApprove => self.parse_approve_stmt(),
            TokKind::KwBreak => self.parse_simple_kw_stmt(TokKind::KwBreak, |_| {
                // We don't have a dedicated Break variant yet — represent it as
                // an expression statement referencing the keyword would be wrong.
                // Use a placeholder: treat Break/Continue/Pass as expression-less
                // marker statements via Stmt::Expr with a specific Ident.
                // For cleanness we'll add variants when the AST needs them.
                unreachable!("handled specially")
            }),
            TokKind::KwContinue => self.parse_simple_kw_stmt(TokKind::KwContinue, |_| {
                unreachable!("handled specially")
            }),
            TokKind::KwPass => self.parse_simple_kw_stmt(TokKind::KwPass, |_| {
                unreachable!("handled specially")
            }),
            TokKind::Ident(_) => self.parse_assign_or_expr_stmt(),
            _ => self.parse_expr_stmt(),
        }
    }

    fn parse_return_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.peek_span();
        self.bump(); // return
        let value = if matches!(self.peek(), TokKind::Newline | TokKind::Eof) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        let end = self.peek_span();
        self.expect_newline()?;
        Ok(Stmt::Return {
            value,
            span: start.merge(end),
        })
    }

    fn parse_if_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.peek_span();
        self.bump(); // if
        let cond = self.parse_expr()?;
        self.expect(TokKind::Colon, "`:` after `if` condition")?;
        self.expect_newline()?;
        let then_block = self.parse_indented_block()?;
        let else_block = if matches!(self.peek(), TokKind::KwElse) {
            self.bump();
            self.expect(TokKind::Colon, "`:` after `else`")?;
            self.expect_newline()?;
            Some(self.parse_indented_block()?)
        } else {
            None
        };
        let end = else_block
            .as_ref()
            .map(|b| b.span)
            .unwrap_or(then_block.span);
        Ok(Stmt::If {
            cond,
            then_block,
            else_block,
            span: start.merge(end),
        })
    }

    fn parse_for_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.peek_span();
        self.bump(); // for
        let (var_name, var_span) = self.expect_ident()?;
        self.expect(TokKind::KwIn, "`in` in `for` loop")?;
        let iter = self.parse_expr()?;
        self.expect(TokKind::Colon, "`:` after `for` clause")?;
        self.expect_newline()?;
        let body = self.parse_indented_block()?;
        let end = body.span;
        Ok(Stmt::For {
            var: Ident::new(var_name, var_span),
            iter,
            body,
            span: start.merge(end),
        })
    }

    fn parse_approve_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.peek_span();
        self.bump(); // approve
        let action = self.parse_expr()?;
        let end = action.span();
        self.expect_newline()?;
        Ok(Stmt::Approve {
            action,
            span: start.merge(end),
        })
    }

    /// `break`, `continue`, and `pass` are each a single keyword + newline.
    /// Since the AST doesn't yet have dedicated variants, they are encoded
    /// as expression statements containing a sentinel `Ident` — the name
    /// resolver will recognize them. (A future cleanup: add real variants.)
    fn parse_simple_kw_stmt(
        &mut self,
        _expected: TokKind,
        _: fn(Span) -> Stmt,
    ) -> Result<Stmt, ParseError> {
        let span = self.peek_span();
        let kw = self.peek().clone();
        self.bump();
        self.expect_newline()?;
        let name = match kw {
            TokKind::KwBreak => "break",
            TokKind::KwContinue => "continue",
            TokKind::KwPass => "pass",
            _ => unreachable!(),
        };
        Ok(Stmt::Expr {
            expr: Expr::Ident {
                name: Ident::new(name, span),
                span,
            },
            span,
        })
    }

    /// `IDENT '=' expr NEWLINE` is an assignment; anything else is an
    /// expression statement.
    fn parse_assign_or_expr_stmt(&mut self) -> Result<Stmt, ParseError> {
        // Peek two ahead: IDENT then `=` ? → assignment.
        if matches!(self.peek(), TokKind::Ident(_))
            && matches!(
                self.tokens.get(self.pos + 1).map(|t| &t.kind),
                Some(TokKind::Assign)
            )
        {
            let start = self.peek_span();
            let (name, name_span) = self.expect_ident()?;
            self.bump(); // =
            let value = self.parse_expr()?;
            let end = value.span();
            self.expect_newline()?;
            return Ok(Stmt::Let {
                name: Ident::new(name, name_span),
                ty: None,
                value,
                span: start.merge(end),
            });
        }
        self.parse_expr_stmt()
    }

    fn parse_expr_stmt(&mut self) -> Result<Stmt, ParseError> {
        let expr = self.parse_expr()?;
        let span = expr.span();
        self.expect_newline()?;
        Ok(Stmt::Expr { expr, span })
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

    /// Skip tokens until we reach a token that could start a declaration
    /// (or EOF). Used after a parse error at the top level.
    fn sync_to_next_decl(&mut self) {
        loop {
            match self.peek() {
                TokKind::KwImport
                | TokKind::KwType
                | TokKind::KwTool
                | TokKind::KwPrompt
                | TokKind::KwAgent
                | TokKind::KwExtend
                | TokKind::Eof => return,
                _ => {
                    self.bump();
                }
            }
        }
    }

    fn parse_decl(&mut self) -> Result<Decl, ParseError> {
        match self.peek() {
            TokKind::KwImport => self.parse_import_decl().map(Decl::Import),
            TokKind::KwType => self.parse_type_decl().map(Decl::Type),
            TokKind::KwTool => self.parse_tool_decl().map(Decl::Tool),
            TokKind::KwPrompt => self.parse_prompt_decl().map(Decl::Prompt),
            TokKind::KwAgent => self.parse_agent_decl().map(Decl::Agent),
            TokKind::KwExtend => self.parse_extend_decl().map(Decl::Extend),
            other => Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(other),
                    expected: "a top-level declaration (agent, tool, prompt, type, import, extend)".into(),
                },
                span: self.peek_span(),
            }),
        }
    }

    // -- import --------------------------------------------------

    fn parse_import_decl(&mut self) -> Result<ImportDecl, ParseError> {
        let start = self.peek_span();
        self.bump(); // import

        // Source language: currently only `python` is accepted.
        let (source_name, source_span) = self.expect_ident()?;
        let source = match source_name.as_str() {
            "python" => ImportSource::Python,
            _ => {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: format!("identifier `{source_name}`"),
                        expected: "an import source (currently: `python`)".into(),
                    },
                    span: source_span,
                });
            }
        };

        // Module string.
        let module_span = self.peek_span();
        let module = match self.peek().clone() {
            TokKind::StringLit(s) => {
                self.bump();
                s
            }
            other => {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: describe_token(&other),
                        expected: "a module name string".into(),
                    },
                    span: module_span,
                });
            }
        };

        // Optional `as IDENT`.
        let alias = if matches!(self.peek(), TokKind::KwAs) {
            self.bump();
            let (name, span) = self.expect_ident()?;
            Some(Ident::new(name, span))
        } else {
            None
        };

        let end = self.peek_span();
        self.expect_newline()?;
        Ok(ImportDecl {
            source,
            module,
            alias,
            span: start.merge(end),
        })
    }

    // -- type ----------------------------------------------------

    fn parse_type_decl(&mut self) -> Result<TypeDecl, ParseError> {
        let start = self.peek_span();
        self.bump(); // type

        let (name, name_span) = self.expect_ident()?;
        self.expect(TokKind::Colon, "`:` after type name")?;
        self.expect_newline()?;

        if !matches!(self.peek(), TokKind::Indent) {
            return Err(ParseError {
                kind: ParseErrorKind::ExpectedBlock,
                span: self.peek_span(),
            });
        }
        self.bump(); // Indent

        let mut fields = Vec::new();
        while !matches!(self.peek(), TokKind::Dedent | TokKind::Eof) {
            self.skip_newlines();
            if matches!(self.peek(), TokKind::Dedent | TokKind::Eof) {
                break;
            }
            match self.parse_field() {
                Ok(f) => fields.push(f),
                Err(e) => {
                    self.errors.push(e);
                    self.sync_to_statement_boundary();
                }
            }
        }
        let end = self.peek_span();
        if matches!(self.peek(), TokKind::Dedent) {
            self.bump();
        }

        Ok(TypeDecl {
            name: Ident::new(name, name_span),
            fields,
            span: start.merge(end),
        })
    }

    fn parse_field(&mut self) -> Result<Field, ParseError> {
        let start = self.peek_span();
        let (name, name_span) = self.expect_ident()?;
        self.expect(TokKind::Colon, "`:` between field name and type")?;
        let ty = self.parse_type_ref()?;
        let end = ty.span();
        self.expect_newline()?;
        Ok(Field {
            name: Ident::new(name, name_span),
            ty,
            span: start.merge(end),
        })
    }

    // -- tool ----------------------------------------------------

    fn parse_tool_decl(&mut self) -> Result<ToolDecl, ParseError> {
        let start = self.peek_span();
        self.bump(); // tool

        let (name, name_span) = self.expect_ident()?;
        let params = self.parse_params()?;
        self.expect(TokKind::Arrow, "`->` before return type")?;
        let return_ty = self.parse_type_ref()?;

        let effect = if matches!(self.peek(), TokKind::KwDangerous) {
            self.bump();
            Effect::Dangerous
        } else {
            Effect::Safe
        };

        let end = self.peek_span();
        self.expect_newline()?;
        Ok(ToolDecl {
            name: Ident::new(name, name_span),
            params,
            return_ty,
            effect,
            span: start.merge(end),
        })
    }

    // -- prompt --------------------------------------------------

    fn parse_prompt_decl(&mut self) -> Result<PromptDecl, ParseError> {
        let start = self.peek_span();
        self.bump(); // prompt

        let (name, name_span) = self.expect_ident()?;
        let params = self.parse_params()?;
        self.expect(TokKind::Arrow, "`->` before return type")?;
        let return_ty = self.parse_type_ref()?;
        self.expect(TokKind::Colon, "`:` after prompt signature")?;
        self.expect_newline()?;

        if !matches!(self.peek(), TokKind::Indent) {
            return Err(ParseError {
                kind: ParseErrorKind::ExpectedBlock,
                span: self.peek_span(),
            });
        }
        self.bump(); // Indent

        // Expect a single string literal as the template.
        let template = match self.peek().clone() {
            TokKind::StringLit(s) => {
                self.bump();
                s
            }
            other => {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: describe_token(&other),
                        expected: "a prompt template string".into(),
                    },
                    span: self.peek_span(),
                });
            }
        };
        self.expect_newline()?;

        let end = self.peek_span();
        if matches!(self.peek(), TokKind::Dedent) {
            self.bump();
        }

        Ok(PromptDecl {
            name: Ident::new(name, name_span),
            params,
            return_ty,
            template,
            span: start.merge(end),
        })
    }

    // -- agent ---------------------------------------------------

    fn parse_agent_decl(&mut self) -> Result<AgentDecl, ParseError> {
        let start = self.peek_span();
        self.bump(); // agent

        let (name, name_span) = self.expect_ident()?;
        let params = self.parse_params()?;
        self.expect(TokKind::Arrow, "`->` before return type")?;
        let return_ty = self.parse_type_ref()?;
        self.expect(TokKind::Colon, "`:` after agent signature")?;
        self.expect_newline()?;

        let body = self.parse_indented_block()?;
        let end = body.span;

        Ok(AgentDecl {
            name: Ident::new(name, name_span),
            params,
            return_ty,
            body,
            span: start.merge(end),
        })
    }

    // -- extend (Phase 16 methods) ------------------------------

    /// Parse an `extend TypeName:` block. The body is an indented
    /// list of tool / prompt / agent declarations, each optionally
    /// prefixed with `public` or `public(package)`.
    ///
    /// ```text
    /// extend Order:
    ///     public agent total(o: Order) -> Int:
    ///         return o.amount + o.tax
    ///     public prompt summarize(o: Order) -> String:
    ///         "..."
    ///     public tool fetch_status(o: Order) -> Status dangerous
    ///     agent compute_tax(o: Order) -> Int:   # private
    ///         return o.amount / 10
    /// ```
    fn parse_extend_decl(&mut self) -> Result<ExtendDecl, ParseError> {
        let start = self.peek_span();
        self.bump(); // extend

        let (name, name_span) = self.expect_ident()?;
        self.expect(TokKind::Colon, "`:` after extend target")?;
        self.expect_newline()?;
        self.expect(TokKind::Indent, "indented block of methods")?;

        let mut methods: Vec<ExtendMethod> = Vec::new();
        while !matches!(self.peek(), TokKind::Dedent | TokKind::Eof) {
            let visibility = self.parse_optional_visibility()?;
            let method_kind = match self.peek() {
                TokKind::KwAgent => {
                    let d = self.parse_agent_decl()?;
                    ExtendMethodKind::Agent(d)
                }
                TokKind::KwPrompt => {
                    let d = self.parse_prompt_decl()?;
                    ExtendMethodKind::Prompt(d)
                }
                TokKind::KwTool => {
                    let d = self.parse_tool_decl()?;
                    ExtendMethodKind::Tool(d)
                }
                other => {
                    return Err(ParseError {
                        kind: ParseErrorKind::UnexpectedToken {
                            got: describe_token(other),
                            expected: "agent / prompt / tool declaration inside `extend` block".into(),
                        },
                        span: self.peek_span(),
                    });
                }
            };
            methods.push(ExtendMethod {
                visibility,
                kind: method_kind,
            });
        }

        let end_span = self.peek_span();
        self.expect(
            TokKind::Dedent,
            "end of indented `extend` block (dedent)",
        )?;

        Ok(ExtendDecl {
            type_name: Ident::new(name, name_span),
            methods,
            span: start.merge(end_span),
        })
    }

    /// Parse an optional visibility prefix: `public`, `public(package)`,
    /// or nothing (returning `Visibility::Private`). Consumes the
    /// tokens on success; leaves them alone if no `public` keyword.
    fn parse_optional_visibility(&mut self) -> Result<Visibility, ParseError> {
        if !matches!(self.peek(), TokKind::KwPublic) {
            return Ok(Visibility::Private);
        }
        self.bump(); // public
        if matches!(self.peek(), TokKind::LParen) {
            self.bump(); // (
            // Only `package` is accepted inside public(...) at Phase
            // 16. Phase 20 extends with effect-scoped variants.
            match self.peek() {
                TokKind::KwPackage => {
                    self.bump();
                }
                other => {
                    return Err(ParseError {
                        kind: ParseErrorKind::UnexpectedToken {
                            got: describe_token(other),
                            expected: "`package` inside `public(...)` (the only variant Phase 16 supports)".into(),
                        },
                        span: self.peek_span(),
                    });
                }
            }
            self.expect(TokKind::RParen, "`)` after `public(package)`")?;
            Ok(Visibility::PublicPackage)
        } else {
            Ok(Visibility::Public)
        }
    }

    // -- shared helpers -----------------------------------------

    /// Parse `( )` or `( param (, param)* )`.
    fn parse_params(&mut self) -> Result<Vec<Param>, ParseError> {
        self.expect(TokKind::LParen, "`(` to open parameter list")?;
        let mut params = Vec::new();
        if !matches!(self.peek(), TokKind::RParen) {
            params.push(self.parse_param()?);
            while matches!(self.peek(), TokKind::Comma) {
                self.bump();
                if matches!(self.peek(), TokKind::RParen) {
                    break; // allow trailing comma
                }
                params.push(self.parse_param()?);
            }
        }
        let close_span = self.peek_span();
        if !matches!(self.peek(), TokKind::RParen) {
            return Err(ParseError {
                kind: ParseErrorKind::UnclosedParen,
                span: close_span,
            });
        }
        self.bump();
        Ok(params)
    }

    fn parse_param(&mut self) -> Result<Param, ParseError> {
        let start = self.peek_span();
        let (name, name_span) = self.expect_ident()?;
        self.expect(TokKind::Colon, "`:` between parameter name and type")?;
        let ty = self.parse_type_ref()?;
        let end = ty.span();
        Ok(Param {
            name: Ident::new(name, name_span),
            ty,
            span: start.merge(end),
        })
    }

    fn parse_type_ref(&mut self) -> Result<TypeRef, ParseError> {
        let (name, name_span) = self.expect_ident()?;
        let name_ident = Ident::new(name, name_span);
        if !matches!(self.peek(), TokKind::Lt) {
            return Ok(TypeRef::Named {
                name: name_ident,
                span: name_span,
            });
        }

        self.bump(); // <
        let mut args = Vec::new();
        if !matches!(self.peek(), TokKind::Gt) {
            args.push(self.parse_type_ref()?);
            while matches!(self.peek(), TokKind::Comma) {
                self.bump();
                args.push(self.parse_type_ref()?);
            }
        }
        let end_span = self.peek_span();
        self.expect(TokKind::Gt, "`>` to close generic type arguments")?;
        Ok(TypeRef::Generic {
            name: name_ident,
            args,
            span: name_span.merge(end_span),
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lex;
    use corvid_ast::{BinaryOp, Expr, Literal, UnaryOp};

    fn parse(src: &str) -> Expr {
        let tokens = lex(src).expect("lex failed");
        parse_expr(&tokens).expect("parse failed")
    }

    fn try_parse(src: &str) -> Result<Expr, ParseError> {
        let tokens = lex(src).expect("lex failed");
        parse_expr(&tokens)
    }

    // -------------------- literals --------------------

    #[test]
    fn int_literal() {
        assert!(matches!(
            parse("42"),
            Expr::Literal { value: Literal::Int(42), .. }
        ));
    }

    #[test]
    fn float_literal() {
        match parse("3.14") {
            Expr::Literal { value: Literal::Float(f), .. } => assert!((f - 3.14).abs() < 1e-9),
            other => panic!("expected float, got {other:?}"),
        }
    }

    #[test]
    fn string_literal() {
        assert!(matches!(
            parse(r#""hello""#),
            Expr::Literal { value: Literal::String(ref s), .. } if s == "hello"
        ));
    }

    #[test]
    fn bool_literals() {
        assert!(matches!(
            parse("true"),
            Expr::Literal { value: Literal::Bool(true), .. }
        ));
        assert!(matches!(
            parse("false"),
            Expr::Literal { value: Literal::Bool(false), .. }
        ));
    }

    #[test]
    fn nothing_literal() {
        assert!(matches!(
            parse("nothing"),
            Expr::Literal { value: Literal::Nothing, .. }
        ));
    }

    #[test]
    fn identifier() {
        assert!(matches!(
            parse("order"),
            Expr::Ident { ref name, .. } if name.name == "order"
        ));
    }

    // -------------------- parentheses --------------------

    #[test]
    fn parenthesized_expression() {
        // `(42)` should produce the same AST as `42`.
        assert!(matches!(
            parse("(42)"),
            Expr::Literal { value: Literal::Int(42), .. }
        ));
    }

    // -------------------- operator precedence --------------------

    #[test]
    fn multiplication_binds_tighter_than_addition() {
        // `1 + 2 * 3` parses as `1 + (2 * 3)`.
        let e = parse("1 + 2 * 3");
        match e {
            Expr::BinOp { op: BinaryOp::Add, ref left, ref right, .. } => {
                assert!(matches!(**left, Expr::Literal { value: Literal::Int(1), .. }));
                match &**right {
                    Expr::BinOp { op: BinaryOp::Mul, left: l2, right: r2, .. } => {
                        assert!(matches!(**l2, Expr::Literal { value: Literal::Int(2), .. }));
                        assert!(matches!(**r2, Expr::Literal { value: Literal::Int(3), .. }));
                    }
                    other => panic!("expected inner Mul, got {other:?}"),
                }
            }
            other => panic!("expected Add at top, got {other:?}"),
        }
    }

    #[test]
    fn parens_override_precedence() {
        // `(1 + 2) * 3` parses as `(Add(1, 2)) * 3`.
        let e = parse("(1 + 2) * 3");
        match e {
            Expr::BinOp { op: BinaryOp::Mul, ref left, ref right, .. } => {
                assert!(matches!(**left, Expr::BinOp { op: BinaryOp::Add, .. }));
                assert!(matches!(**right, Expr::Literal { value: Literal::Int(3), .. }));
            }
            other => panic!("expected Mul at top, got {other:?}"),
        }
    }

    #[test]
    fn logical_precedence_or_below_and() {
        // `a or b and c` parses as `a or (b and c)`.
        let e = parse("a or b and c");
        match e {
            Expr::BinOp { op: BinaryOp::Or, ref right, .. } => {
                assert!(matches!(**right, Expr::BinOp { op: BinaryOp::And, .. }));
            }
            other => panic!("expected Or at top, got {other:?}"),
        }
    }

    #[test]
    fn not_binds_after_and_or() {
        // `not a and b` parses as `(not a) and b`.
        let e = parse("not a and b");
        match e {
            Expr::BinOp { op: BinaryOp::And, ref left, .. } => {
                assert!(matches!(**left, Expr::UnOp { op: UnaryOp::Not, .. }));
            }
            other => panic!("expected And at top, got {other:?}"),
        }
    }

    #[test]
    fn unary_minus_stacks() {
        // `--x` parses as `Neg(Neg(x))`.
        let e = parse("--x");
        match e {
            Expr::UnOp { op: UnaryOp::Neg, ref operand, .. } => {
                assert!(matches!(**operand, Expr::UnOp { op: UnaryOp::Neg, .. }));
            }
            other => panic!("expected outer Neg, got {other:?}"),
        }
    }

    #[test]
    fn unary_minus_binds_tighter_than_binary_minus() {
        // `-x - y` parses as `(Neg(x)) - y`.
        let e = parse("-x - y");
        match e {
            Expr::BinOp { op: BinaryOp::Sub, ref left, .. } => {
                assert!(matches!(**left, Expr::UnOp { op: UnaryOp::Neg, .. }));
            }
            other => panic!("expected Sub at top, got {other:?}"),
        }
    }

    // -------------------- postfix operators --------------------

    #[test]
    fn field_access_chains() {
        // `a.b.c` parses as `FieldAccess(FieldAccess(a, b), c)`.
        let e = parse("a.b.c");
        match e {
            Expr::FieldAccess { ref target, ref field, .. } => {
                assert_eq!(field.name, "c");
                assert!(matches!(**target, Expr::FieldAccess { .. }));
            }
            other => panic!("expected outer FieldAccess, got {other:?}"),
        }
    }

    #[test]
    fn call_with_args() {
        let e = parse("f(1, 2, 3)");
        match e {
            Expr::Call { ref callee, ref args, .. } => {
                assert!(matches!(**callee, Expr::Ident { .. }));
                assert_eq!(args.len(), 3);
            }
            other => panic!("expected Call, got {other:?}"),
        }
    }

    #[test]
    fn call_with_trailing_comma() {
        let e = parse("f(1, 2,)");
        match e {
            Expr::Call { args, .. } => assert_eq!(args.len(), 2),
            other => panic!("expected Call, got {other:?}"),
        }
    }

    #[test]
    fn indexing() {
        let e = parse("xs[0]");
        assert!(matches!(e, Expr::Index { .. }));
    }

    #[test]
    fn mixed_postfix_chain() {
        // `f(x).y[z]` — call, field, index in order.
        let e = parse("f(x).y[z]");
        match e {
            Expr::Index { target, .. } => match *target {
                Expr::FieldAccess { target, .. } => {
                    assert!(matches!(*target, Expr::Call { .. }));
                }
                other => panic!("expected FieldAccess, got {other:?}"),
            },
            other => panic!("expected outer Index, got {other:?}"),
        }
    }

    // -------------------- list literals --------------------

    #[test]
    fn empty_list() {
        let e = parse("[]");
        match e {
            Expr::List { items, .. } => assert_eq!(items.len(), 0),
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn list_with_items() {
        let e = parse("[1, 2, 3]");
        match e {
            Expr::List { items, .. } => assert_eq!(items.len(), 3),
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn parses_postfix_try_propagate() {
        let e = parse("load_order()?");
        match e {
            Expr::TryPropagate { inner, .. } => {
                assert!(matches!(*inner, Expr::Call { .. }));
            }
            other => panic!("expected TryPropagate, got {other:?}"),
        }
    }

    #[test]
    fn parses_try_retry_with_linear_backoff() {
        let e = parse("try fetch_order(id) on error retry 3 times backoff linear 50");
        match e {
            Expr::TryRetry {
                body,
                attempts,
                backoff,
                ..
            } => {
                assert_eq!(attempts, 3);
                assert_eq!(backoff, Backoff::Linear(50));
                assert!(matches!(*body, Expr::Call { .. }));
            }
            other => panic!("expected TryRetry, got {other:?}"),
        }
    }

    #[test]
    fn parses_try_retry_with_exponential_backoff() {
        let e = parse("try maybe_send() on error retry 5 times backoff exponential 125");
        match e {
            Expr::TryRetry {
                attempts,
                backoff,
                ..
            } => {
                assert_eq!(attempts, 5);
                assert_eq!(backoff, Backoff::Exponential(125));
            }
            other => panic!("expected TryRetry, got {other:?}"),
        }
    }

    // -------------------- errors --------------------

    #[test]
    fn rejects_chained_comparison() {
        let err = try_parse("a < b < c").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::ChainedComparison));
    }

    #[test]
    fn rejects_unclosed_paren() {
        let err = try_parse("(1 + 2").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::UnclosedParen));
    }

    #[test]
    fn rejects_unclosed_bracket() {
        let err = try_parse("[1, 2").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::UnclosedBracket));
    }

    #[test]
    fn rejects_empty_input() {
        let err = try_parse("").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::UnexpectedEof));
    }

    #[test]
    fn rejects_retry_without_backoff_policy_kind() {
        let err = try_parse("try fetch() on error retry 2 times backoff 100").unwrap_err();
        assert!(matches!(err.kind, ParseErrorKind::UnexpectedToken { .. }));
    }

    // -------------------- realistic agent snippets --------------------

    #[test]
    fn parses_field_on_call() {
        // Real Corvid pattern: tool call, then field access.
        let e = parse("get_order(ticket.order_id).amount");
        assert!(matches!(e, Expr::FieldAccess { .. }));
    }

    #[test]
    fn parses_struct_literal_via_call_syntax() {
        // `IssueRefund(order.id, order.amount)` — just a call at parse time.
        let e = parse("IssueRefund(order.id, order.amount)");
        match e {
            Expr::Call { callee, args, .. } => {
                assert!(matches!(*callee, Expr::Ident { .. }));
                assert_eq!(args.len(), 2);
            }
            other => panic!("expected Call, got {other:?}"),
        }
    }

    // =================================================================
    // Phase 3b — Statement and block parser tests
    // =================================================================

    use corvid_ast::{Block, Stmt};

    /// Lex a source snippet and strip the leading Newline (if any) so the
    /// token stream begins at the first meaningful token. Tests below use
    /// raw strings with the first line blank for readability.
    fn lex_block_src(src: &str) -> Vec<Token> {
        let mut toks = lex(src).expect("lex failed");
        // Drop an initial Newline introduced by a leading blank line.
        while matches!(toks.first().map(|t| &t.kind), Some(TokKind::Newline)) {
            toks.remove(0);
        }
        toks
    }

    fn parse_blk(src: &str) -> Block {
        let tokens = lex_block_src(src);
        let (block, errors) = parse_block(&tokens);
        assert!(
            errors.is_empty(),
            "parse errors: {:?}\nsource:\n{src}",
            errors
        );
        block
    }

    fn parse_blk_errs(src: &str) -> (Block, Vec<ParseError>) {
        let tokens = lex_block_src(src);
        parse_block(&tokens)
    }

    // -------------------- assignment --------------------

    #[test]
    fn parses_simple_assignment() {
        let b = parse_blk("\n    x = 42\n");
        assert_eq!(b.stmts.len(), 1);
        match &b.stmts[0] {
            Stmt::Let { name, value, .. } => {
                assert_eq!(name.name, "x");
                assert!(matches!(value, Expr::Literal { value: Literal::Int(42), .. }));
            }
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn parses_assignment_to_call_result() {
        let b = parse_blk("\n    order = get_order(ticket.order_id)\n");
        assert!(matches!(&b.stmts[0], Stmt::Let { .. }));
    }

    // -------------------- expression statement --------------------

    #[test]
    fn parses_expression_statement() {
        let b = parse_blk("\n    issue_refund(id, amount)\n");
        assert!(matches!(&b.stmts[0], Stmt::Expr { .. }));
    }

    // -------------------- return --------------------

    #[test]
    fn parses_return_with_value() {
        let b = parse_blk("\n    return decision\n");
        match &b.stmts[0] {
            Stmt::Return { value: Some(_), .. } => {}
            other => panic!("expected Return Some, got {other:?}"),
        }
    }

    #[test]
    fn parses_bare_return() {
        let b = parse_blk("\n    return\n");
        match &b.stmts[0] {
            Stmt::Return { value: None, .. } => {}
            other => panic!("expected Return None, got {other:?}"),
        }
    }

    // -------------------- if / else --------------------

    #[test]
    fn parses_if_without_else() {
        let src = "\n    if x:\n        y = 1\n";
        let b = parse_blk(src);
        match &b.stmts[0] {
            Stmt::If { then_block, else_block: None, .. } => {
                assert_eq!(then_block.stmts.len(), 1);
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn parses_if_with_else() {
        let src = "\n    if x:\n        y = 1\n    else:\n        y = 2\n";
        let b = parse_blk(src);
        match &b.stmts[0] {
            Stmt::If { then_block, else_block: Some(el), .. } => {
                assert_eq!(then_block.stmts.len(), 1);
                assert_eq!(el.stmts.len(), 1);
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    // -------------------- for --------------------

    #[test]
    fn parses_for_loop() {
        let src = "\n    for item in items:\n        process(item)\n";
        let b = parse_blk(src);
        match &b.stmts[0] {
            Stmt::For { var, body, .. } => {
                assert_eq!(var.name, "item");
                assert_eq!(body.stmts.len(), 1);
            }
            other => panic!("expected For, got {other:?}"),
        }
    }

    // -------------------- approve --------------------

    #[test]
    fn parses_approve_stmt() {
        let b = parse_blk("\n    approve IssueRefund(order.id, order.amount)\n");
        match &b.stmts[0] {
            Stmt::Approve { action, .. } => {
                assert!(matches!(action, Expr::Call { .. }));
            }
            other => panic!("expected Approve, got {other:?}"),
        }
    }

    // -------------------- break / continue / pass --------------------

    #[test]
    fn parses_break_continue_pass() {
        let src = "\n    for x in xs:\n        if x:\n            break\n        if x:\n            continue\n        pass\n";
        let b = parse_blk(src);
        // Just ensure parsing succeeds. (break/continue/pass currently encoded
        // as Expr::Ident statements — will get dedicated AST variants later.)
        assert_eq!(b.stmts.len(), 1);
    }

    // -------------------- canonical refund_bot body --------------------

    #[test]
    fn parses_refund_bot_body() {
        let src = "
    order = get_order(ticket.order_id)
    decision = decide_refund(ticket, order)

    if decision.should_refund:
        approve IssueRefund(order.id, order.amount)
        issue_refund(order.id, order.amount)

    return decision
";
        let b = parse_blk(src);
        assert_eq!(b.stmts.len(), 4);
        assert!(matches!(b.stmts[0], Stmt::Let { .. }));
        assert!(matches!(b.stmts[1], Stmt::Let { .. }));
        assert!(matches!(b.stmts[2], Stmt::If { .. }));
        assert!(matches!(b.stmts[3], Stmt::Return { .. }));

        // Inner: the if body should contain approve then call.
        if let Stmt::If { then_block, .. } = &b.stmts[2] {
            assert_eq!(then_block.stmts.len(), 2);
            assert!(matches!(then_block.stmts[0], Stmt::Approve { .. }));
            assert!(matches!(then_block.stmts[1], Stmt::Expr { .. }));
        }
    }

    // -------------------- errors --------------------

    #[test]
    fn missing_colon_after_if_reports_error() {
        let src = "\n    if x\n        y = 1\n";
        let (_block, errs) = parse_blk_errs(src);
        assert!(!errs.is_empty(), "expected error for missing colon");
        assert!(
            errs.iter().any(|e| matches!(
                e.kind,
                ParseErrorKind::UnexpectedToken { .. }
            )),
            "expected UnexpectedToken, got {errs:?}"
        );
    }

    #[test]
    fn empty_block_reports_error() {
        // Block with only a blank line inside — no statements. Since the
        // lexer collapses blank lines away entirely, we simulate this with
        // a raw token sequence: Indent Dedent.
        let tokens = vec![
            Token::new(TokKind::Indent, Span::new(0, 0)),
            Token::new(TokKind::Dedent, Span::new(0, 0)),
            Token::new(TokKind::Eof, Span::new(0, 0)),
        ];
        let (_block, errs) = parse_block(&tokens);
        assert!(errs.iter().any(|e| matches!(e.kind, ParseErrorKind::EmptyBlock)));
    }

    #[test]
    fn parser_recovers_and_continues_after_bad_stmt() {
        // First statement is broken (missing `:` after `if`). Second is fine.
        // The parser should report the error but still parse the second.
        let src = "\n    if x\n    y = 42\n";
        let (block, errs) = parse_blk_errs(src);
        assert!(!errs.is_empty());
        // After recovery we should have parsed at least one good statement.
        assert!(
            !block.stmts.is_empty(),
            "expected recovery to yield statements"
        );
    }

    // =================================================================
    // Phase 3c — File / declaration parser tests
    // =================================================================

    use corvid_ast::{AgentDecl, Decl, Effect, File, ImportSource, TypeRef};

    fn parse_file_src(src: &str) -> File {
        let tokens = lex(src).expect("lex failed");
        let (file, errors) = parse_file(&tokens);
        assert!(
            errors.is_empty(),
            "parse errors: {:?}\nsource:\n{src}",
            errors
        );
        file
    }

    fn parse_file_errs(src: &str) -> (File, Vec<ParseError>) {
        let tokens = lex(src).expect("lex failed");
        parse_file(&tokens)
    }

    // -------------------- imports --------------------

    #[test]
    fn parses_import_python() {
        let file = parse_file_src(r#"import python "anthropic" as anthropic"#);
        assert_eq!(file.decls.len(), 1);
        match &file.decls[0] {
            Decl::Import(i) => {
                assert!(matches!(i.source, ImportSource::Python));
                assert_eq!(i.module, "anthropic");
                assert_eq!(i.alias.as_ref().unwrap().name, "anthropic");
            }
            other => panic!("expected Import, got {other:?}"),
        }
    }

    #[test]
    fn parses_import_without_alias() {
        let file = parse_file_src(r#"import python "anthropic""#);
        match &file.decls[0] {
            Decl::Import(i) => {
                assert_eq!(i.module, "anthropic");
                assert!(i.alias.is_none());
            }
            other => panic!("expected Import, got {other:?}"),
        }
    }

    // -------------------- types --------------------

    #[test]
    fn parses_type_decl() {
        let src = "\
type Ticket:
    order_id: String
    user_id: String
    message: String
";
        let file = parse_file_src(src);
        match &file.decls[0] {
            Decl::Type(t) => {
                assert_eq!(t.name.name, "Ticket");
                assert_eq!(t.fields.len(), 3);
                assert_eq!(t.fields[0].name.name, "order_id");
            }
            other => panic!("expected Type, got {other:?}"),
        }
    }

    #[test]
    fn parses_result_and_option_type_refs() {
        let src = "\
agent load(id: String) -> Result<Option<Order>, String>:
    return fetch(id)
";
        let file = parse_file_src(src);
        let agent = match &file.decls[0] {
            Decl::Agent(a) => a,
            other => panic!("expected Agent, got {other:?}"),
        };
        match &agent.return_ty {
            TypeRef::Generic { name, args, .. } => {
                assert_eq!(name.name, "Result");
                assert_eq!(args.len(), 2);
                assert!(matches!(
                    &args[0],
                    TypeRef::Generic { name, args, .. }
                    if name.name == "Option" && args.len() == 1
                ));
                assert!(matches!(
                    &args[1],
                    TypeRef::Named { name, .. } if name.name == "String"
                ));
            }
            other => panic!("expected generic Result return type, got {other:?}"),
        }
    }

    // -------------------- tools --------------------

    #[test]
    fn parses_safe_tool() {
        let src = "tool get_order(id: String) -> Order";
        let file = parse_file_src(src);
        match &file.decls[0] {
            Decl::Tool(t) => {
                assert_eq!(t.name.name, "get_order");
                assert_eq!(t.params.len(), 1);
                assert_eq!(t.params[0].name.name, "id");
                assert!(matches!(t.effect, Effect::Safe));
                assert!(matches!(
                    t.return_ty,
                    TypeRef::Named { ref name, .. } if name.name == "Order"
                ));
            }
            other => panic!("expected Tool, got {other:?}"),
        }
    }

    #[test]
    fn parses_dangerous_tool() {
        let src = "tool issue_refund(id: String, amount: Float) -> Receipt dangerous";
        let file = parse_file_src(src);
        match &file.decls[0] {
            Decl::Tool(t) => {
                assert_eq!(t.params.len(), 2);
                assert!(matches!(t.effect, Effect::Dangerous));
            }
            other => panic!("expected Tool, got {other:?}"),
        }
    }

    #[test]
    fn parses_tool_with_no_params() {
        let file = parse_file_src("tool now() -> String");
        match &file.decls[0] {
            Decl::Tool(t) => assert_eq!(t.params.len(), 0),
            other => panic!("expected Tool, got {other:?}"),
        }
    }

    // -------------------- prompts --------------------

    #[test]
    fn parses_single_line_prompt() {
        let src = "\
prompt greet(name: String) -> String:
    \"Write a short, warm greeting to {name}.\"
";
        let file = parse_file_src(src);
        match &file.decls[0] {
            Decl::Prompt(p) => {
                assert_eq!(p.name.name, "greet");
                assert_eq!(p.params.len(), 1);
                assert!(p.template.contains("greeting"));
            }
            other => panic!("expected Prompt, got {other:?}"),
        }
    }

    #[test]
    fn parses_triple_quoted_prompt() {
        let src = "\
prompt decide(ticket: Ticket) -> Decision:
    \"\"\"
    Decide whether this ticket deserves a refund.
    Consider the order amount and the user's complaint.
    \"\"\"
";
        let file = parse_file_src(src);
        match &file.decls[0] {
            Decl::Prompt(p) => {
                assert!(p.template.contains("refund"));
                assert!(p.template.contains("complaint"));
            }
            other => panic!("expected Prompt, got {other:?}"),
        }
    }

    // -------------------- agents --------------------

    #[test]
    fn parses_agent_with_body() {
        let src = "\
agent hello(name: String) -> String:
    message = greet(name)
    return message
";
        let file = parse_file_src(src);
        match &file.decls[0] {
            Decl::Agent(a) => {
                assert_eq!(a.name.name, "hello");
                assert_eq!(a.params.len(), 1);
                assert_eq!(a.body.stmts.len(), 2);
            }
            other => panic!("expected Agent, got {other:?}"),
        }
    }

    // -------------------- full refund_bot file --------------------

    #[test]
    fn parses_full_refund_bot_file() {
        let src = r#"
import python "anthropic" as anthropic

type Ticket:
    order_id: String
    user_id: String
    message: String

type Order:
    id: String
    amount: Float
    user_id: String

type Decision:
    should_refund: Bool
    reason: String

type Receipt:
    refund_id: String
    amount: Float

tool get_order(id: String) -> Order
tool issue_refund(id: String, amount: Float) -> Receipt dangerous

prompt decide_refund(ticket: Ticket, order: Order) -> Decision:
    """
    Decide whether this ticket deserves a refund.
    """

agent refund_bot(ticket: Ticket) -> Decision:
    order = get_order(ticket.order_id)
    decision = decide_refund(ticket, order)

    if decision.should_refund:
        approve IssueRefund(order.id, order.amount)
        issue_refund(order.id, order.amount)

    return decision
"#;
        let (file, errors) = parse_file_errs(src);
        assert!(errors.is_empty(), "parse errors: {errors:?}");

        // Expected structure:
        //   1 import
        //   4 types
        //   2 tools
        //   1 prompt
        //   1 agent
        assert_eq!(file.decls.len(), 9);

        let import_count = file.decls.iter().filter(|d| matches!(d, Decl::Import(_))).count();
        let type_count = file.decls.iter().filter(|d| matches!(d, Decl::Type(_))).count();
        let tool_count = file.decls.iter().filter(|d| matches!(d, Decl::Tool(_))).count();
        let prompt_count = file.decls.iter().filter(|d| matches!(d, Decl::Prompt(_))).count();
        let agent_count = file.decls.iter().filter(|d| matches!(d, Decl::Agent(_))).count();
        assert_eq!(import_count, 1);
        assert_eq!(type_count, 4);
        assert_eq!(tool_count, 2);
        assert_eq!(prompt_count, 1);
        assert_eq!(agent_count, 1);

        // Verify dangerous tool is marked, safe tool isn't.
        let tools: Vec<&ToolDecl> = file
            .decls
            .iter()
            .filter_map(|d| if let Decl::Tool(t) = d { Some(t) } else { None })
            .collect();
        assert!(tools.iter().any(|t| matches!(t.effect, Effect::Safe)));
        assert!(tools.iter().any(|t| matches!(t.effect, Effect::Dangerous)));

        // Verify the agent's body parses down to the expected shape.
        let agent: &AgentDecl = file
            .decls
            .iter()
            .find_map(|d| if let Decl::Agent(a) = d { Some(a) } else { None })
            .unwrap();
        assert_eq!(agent.name.name, "refund_bot");
        assert_eq!(agent.body.stmts.len(), 4);
        assert!(matches!(agent.body.stmts[0], Stmt::Let { .. }));
        assert!(matches!(agent.body.stmts[1], Stmt::Let { .. }));
        assert!(matches!(agent.body.stmts[2], Stmt::If { .. }));
        assert!(matches!(agent.body.stmts[3], Stmt::Return { .. }));
    }

    // -------------------- error recovery --------------------

    #[test]
    fn recovers_from_bad_tool_to_following_agent() {
        // Tool is missing `->`. Agent after should still parse.
        let src = "\
tool broken(x: String) Order
agent good(x: String) -> String:
    return x
";
        let (file, errs) = parse_file_errs(src);
        assert!(!errs.is_empty());
        // We should still see the agent declaration in the recovered file.
        assert!(
            file.decls.iter().any(|d| matches!(d, Decl::Agent(_))),
            "expected agent after recovery"
        );
    }

    #[test]
    fn reports_error_on_unknown_top_level_token() {
        let (_file, errs) = parse_file_errs("xyz");
        assert!(!errs.is_empty());
        assert!(
            errs.iter()
                .any(|e| matches!(e.kind, ParseErrorKind::UnexpectedToken { .. }))
        );
    }

    #[test]
    fn reports_error_on_unknown_import_source() {
        let (_file, errs) = parse_file_errs(r#"import ruby "foo""#);
        assert!(!errs.is_empty());
    }

    // -----------------------------------------------------------------
    // Phase 16 — `extend T:` block + visibility parsing
    // -----------------------------------------------------------------

    use corvid_ast::{ExtendDecl, ExtendMethodKind, Visibility};

    fn first_extend(file: &File) -> &ExtendDecl {
        file.decls
            .iter()
            .find_map(|d| match d {
                Decl::Extend(e) => Some(e),
                _ => None,
            })
            .expect("expected an `extend` decl in the file")
    }

    #[test]
    fn parses_extend_with_one_agent_method() {
        let file = parse_file_src(
            "type Order:\n    amount: Int\n\nextend Order:\n    public agent total(o: Order) -> Int:\n        return o.amount\n",
        );
        let ext = first_extend(&file);
        assert_eq!(ext.type_name.name.as_str(), "Order");
        assert_eq!(ext.methods.len(), 1);
        let m = &ext.methods[0];
        assert_eq!(m.visibility, Visibility::Public);
        assert!(matches!(m.kind, ExtendMethodKind::Agent(_)));
        assert_eq!(m.name().name.as_str(), "total");
    }

    #[test]
    fn parses_extend_default_visibility_is_private() {
        let file = parse_file_src(
            "type Order:\n    amount: Int\n\nextend Order:\n    agent total(o: Order) -> Int:\n        return o.amount\n",
        );
        let ext = first_extend(&file);
        assert_eq!(ext.methods[0].visibility, Visibility::Private);
    }

    #[test]
    fn parses_extend_public_package_visibility() {
        let file = parse_file_src(
            "type Order:\n    amount: Int\n\nextend Order:\n    public(package) agent total(o: Order) -> Int:\n        return o.amount\n",
        );
        let ext = first_extend(&file);
        assert_eq!(ext.methods[0].visibility, Visibility::PublicPackage);
    }

    #[test]
    fn parses_extend_with_mixed_decl_kinds() {
        // The whole point of Phase 16's "methods can be any decl kind"
        // — verify the parser accepts a mix of agent / prompt / tool
        // inside one `extend` block.
        let file = parse_file_src(
            "type Order:\n    amount: Int\n\nextend Order:\n    public agent total(o: Order) -> Int:\n        return o.amount\n    public prompt summarize(o: Order) -> String:\n        \"Summarize this order\"\n    public tool fetch_status(o: Order) -> Status dangerous\n",
        );
        let ext = first_extend(&file);
        assert_eq!(ext.methods.len(), 3);
        assert!(matches!(ext.methods[0].kind, ExtendMethodKind::Agent(_)));
        assert!(matches!(ext.methods[1].kind, ExtendMethodKind::Prompt(_)));
        assert!(matches!(ext.methods[2].kind, ExtendMethodKind::Tool(_)));
    }

    #[test]
    fn rejects_public_with_unknown_inner_keyword() {
        let (_file, errs) = parse_file_errs(
            "type Order:\n    amount: Int\n\nextend Order:\n    public(secret) agent total(o: Order) -> Int:\n        return o.amount\n",
        );
        assert!(
            !errs.is_empty(),
            "expected parse error for `public(secret)` — only `public(package)` is valid in Phase 16"
        );
    }
}
