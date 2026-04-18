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
                let name = self.parse_namespaced_ident_from(name)?;
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
            TokKind::KwYield => self.parse_yield_stmt(),
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

    fn parse_yield_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.peek_span();
        self.bump(); // yield
        let value = self.parse_expr()?;
        let end = value.span();
        self.expect_newline()?;
        Ok(Stmt::Yield {
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

    fn parse_decl(&mut self) -> Result<Decl, ParseError> {
        match self.peek() {
            TokKind::KwImport => self.parse_import_decl().map(Decl::Import),
            TokKind::KwType => self.parse_type_decl().map(Decl::Type),
            TokKind::KwTool => self.parse_tool_decl().map(Decl::Tool),
            TokKind::KwPrompt => self.parse_prompt_decl().map(Decl::Prompt),
            TokKind::KwEval => self.parse_eval_decl().map(Decl::Eval),
            TokKind::KwAgent => self.parse_agent_decl().map(Decl::Agent),
            TokKind::KwExtend => self.parse_extend_decl().map(Decl::Extend),
            TokKind::KwEffect => self.parse_effect_decl().map(Decl::Effect),
            TokKind::KwModel => self.parse_model_decl().map(Decl::Model),
            TokKind::At => {
                let constraints = self.parse_constraints()?;
                if !matches!(self.peek(), TokKind::KwAgent) {
                    return Err(ParseError {
                        kind: ParseErrorKind::UnexpectedToken {
                            got: describe_token(self.peek()),
                            expected: "`agent` after constraint annotations".into(),
                        },
                        span: self.peek_span(),
                    });
                }
                let mut agent = self.parse_agent_decl()?;
                agent.constraints = constraints;
                Ok(Decl::Agent(agent))
            }
            other => Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(other),
                    expected: "a top-level declaration (agent, tool, prompt, eval, type, import, extend, effect, @annotation)".into(),
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

        let effect_row = self.parse_uses_clause()?;

        let end = self.peek_span();
        self.expect_newline()?;
        Ok(ToolDecl {
            name: Ident::new(name, name_span),
            params,
            return_ty,
            effect,
            effect_row,
            span: start.merge(end),
        })
    }

    // -- effect --------------------------------------------------

    fn parse_effect_decl(&mut self) -> Result<EffectDecl, ParseError> {
        let start = self.peek_span();
        self.bump(); // effect

        let (name, name_span) = self.expect_ident()?;
        self.expect(TokKind::Colon, "`:` after effect name")?;
        self.expect_newline()?;

        if !matches!(self.peek(), TokKind::Indent) {
            return Err(ParseError {
                kind: ParseErrorKind::ExpectedBlock,
                span: self.peek_span(),
            });
        }
        self.bump(); // Indent

        let mut dimensions = Vec::new();
        while !matches!(self.peek(), TokKind::Dedent | TokKind::Eof) {
            self.skip_newlines();
            if matches!(self.peek(), TokKind::Dedent | TokKind::Eof) {
                break;
            }
            dimensions.push(self.parse_dimension_decl()?);
        }

        let end = self.peek_span();
        if matches!(self.peek(), TokKind::Dedent) {
            self.bump();
        }

        Ok(EffectDecl {
            name: Ident::new(name, name_span),
            dimensions,
            span: start.merge(end),
        })
    }

    fn parse_dimension_decl(&mut self) -> Result<DimensionDecl, ParseError> {
        let start = self.peek_span();
        let (name, name_span) = self.expect_ident()?;
        self.expect(TokKind::Colon, "`:` after dimension name")?;
        let value = self.parse_dimension_value()?;
        let end = self.prev_span();
        self.expect_newline()?;
        Ok(DimensionDecl {
            name: Ident::new(name, name_span),
            value,
            span: start.merge(end),
        })
    }

    // -- model (Phase 20h) --------------------------------------

    fn parse_model_decl(&mut self) -> Result<ModelDecl, ParseError> {
        let start = self.peek_span();
        self.bump(); // model

        let (name, name_span) = self.expect_ident()?;
        self.expect(TokKind::Colon, "`:` after model name")?;
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
            fields.push(self.parse_model_field()?);
        }

        let end = self.peek_span();
        if matches!(self.peek(), TokKind::Dedent) {
            self.bump();
        }

        Ok(ModelDecl {
            name: Ident::new(name, name_span),
            fields,
            span: start.merge(end),
        })
    }

    fn parse_model_field(&mut self) -> Result<ModelField, ParseError> {
        let start = self.peek_span();
        let (name, name_span) = self.expect_ident()?;
        self.expect(TokKind::Colon, "`:` after model field name")?;
        let value = self.parse_dimension_value()?;
        let end = self.prev_span();
        self.expect_newline()?;
        Ok(ModelField {
            name: Ident::new(name, name_span),
            value,
            span: start.merge(end),
        })
    }

    fn parse_dimension_value(&mut self) -> Result<DimensionValue, ParseError> {
        let span = self.peek_span();
        match self.peek().clone() {
            TokKind::KwTrue => {
                self.bump();
                Ok(DimensionValue::Bool(true))
            }
            TokKind::KwFalse => {
                self.bump();
                Ok(DimensionValue::Bool(false))
            }
            TokKind::Dollar => {
                self.bump();
                match self.peek().clone() {
                    TokKind::Int(n) => {
                        self.bump();
                        Ok(DimensionValue::Cost(n as f64))
                    }
                    TokKind::Float(n) => {
                        self.bump();
                        Ok(DimensionValue::Cost(n))
                    }
                    other => Err(ParseError {
                        kind: ParseErrorKind::UnexpectedToken {
                            got: describe_token(&other),
                            expected: "a numeric cost literal after `$`".into(),
                        },
                        span: self.peek_span(),
                    }),
                }
            }
            TokKind::Int(n) => {
                self.bump();
                Ok(DimensionValue::Number(
                    self.consume_optional_duration_suffix(n as f64),
                ))
            }
            TokKind::Float(n) => {
                self.bump();
                Ok(DimensionValue::Number(self.consume_optional_duration_suffix(n)))
            }
            TokKind::StringLit(s) => {
                self.bump();
                Ok(DimensionValue::Name(s))
            }
            TokKind::Ident(name) => {
                self.bump();
                if name == "streaming" && matches!(self.peek(), TokKind::LParen) {
                    self.bump(); // (
                    self.expect_contextual_ident("backpressure")?;
                    self.expect(TokKind::Colon, "`:` after `backpressure`")?;
                    let backpressure = self.parse_backpressure_policy()?;
                    self.expect(TokKind::RParen, "`)` after streaming latency config")?;
                    return Ok(DimensionValue::Streaming { backpressure });
                }
                // Check for confidence-gated trust: `autonomous_if_confident(0.95)`
                if name.ends_with("_if_confident") && matches!(self.peek(), TokKind::LParen) {
                    self.bump(); // (
                    let threshold = match self.peek().clone() {
                        TokKind::Float(f) => { self.bump(); f }
                        TokKind::Int(n) => { self.bump(); n as f64 }
                        other => {
                            return Err(ParseError {
                                kind: ParseErrorKind::UnexpectedToken {
                                    got: describe_token(&other),
                                    expected: "a confidence threshold (0.0–1.0)".into(),
                                },
                                span: self.peek_span(),
                            });
                        }
                    };
                    self.expect(TokKind::RParen, "`)` after confidence threshold")?;
                    let above = name.strip_suffix("_if_confident").unwrap_or(&name).to_string();
                    Ok(DimensionValue::ConfidenceGated {
                        threshold,
                        above,
                        below: "human_required".to_string(),
                    })
                } else {
                    Ok(DimensionValue::Name(name))
                }
            }
            other => Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(&other),
                    expected: "a dimension value".into(),
                },
                span,
            }),
        }
    }

    // -- prompt --------------------------------------------------

    fn parse_prompt_decl(&mut self) -> Result<PromptDecl, ParseError> {
        let start = self.peek_span();
        self.bump(); // prompt

        let (name, name_span) = self.expect_ident()?;
        let params = self.parse_params()?;
        self.expect(TokKind::Arrow, "`->` before return type")?;
        let return_ty = self.parse_type_ref()?;
        let effect_row = self.parse_uses_clause()?;
        self.expect(TokKind::Colon, "`:` after prompt signature")?;
        self.expect_newline()?;

        if !matches!(self.peek(), TokKind::Indent) {
            return Err(ParseError {
                kind: ParseErrorKind::ExpectedBlock,
                span: self.peek_span(),
            });
        }
        self.bump(); // Indent

        // Phase 20h: optional `requires: <capability>` clause.
        // Must appear before `with ...` stream settings and the
        // template, so the body order is: requires → route → with → template.
        let capability_required = if matches!(self.peek(), TokKind::KwRequires) {
            self.bump(); // requires
            self.expect(TokKind::Colon, "`:` after `requires`")?;
            let (ident, ident_span) = self.expect_ident()?;
            self.expect_newline()?;
            Some(Ident::new(ident, ident_span))
        } else {
            None
        };

        // Phase 20h slice C: optional `route:` block. Each arm is
        // `<guard-expr> -> <model-ident>` or `_ -> <model-ident>`.
        let route = if matches!(self.peek(), TokKind::KwRoute) {
            Some(self.parse_prompt_route_block()?)
        } else {
            None
        };

        // Phase 20h slice E: optional `progressive:` block. Mutually
        // exclusive with `route:`; the parser reports a dedicated
        // error if both appear on the same prompt.
        let progressive = if matches!(self.peek(), TokKind::KwProgressive) {
            if route.is_some() {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: "`progressive:` after `route:`".into(),
                        expected: "a prompt template string (a prompt uses either `route:` or `progressive:`, not both)".into(),
                    },
                    span: self.peek_span(),
                });
            }
            Some(self.parse_prompt_progressive_block()?)
        } else {
            None
        };

        // Phase 20h slice I: optional `rollout ...` one-liner.
        // Mutually exclusive with both `route:` and `progressive:`.
        let rollout = if matches!(self.peek(), TokKind::KwRollout) {
            if route.is_some() || progressive.is_some() {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: "`rollout` after another dispatch clause".into(),
                        expected: "a prompt template string (a prompt uses exactly one of `route:`, `progressive:`, `rollout`, or `ensemble`)".into(),
                    },
                    span: self.peek_span(),
                });
            }
            Some(self.parse_prompt_rollout_clause()?)
        } else {
            None
        };

        // Phase 20h slice F: optional `ensemble [...] vote <strategy>`.
        // Mutually exclusive with route / progressive / rollout.
        let ensemble = if matches!(self.peek(), TokKind::KwEnsemble) {
            if route.is_some() || progressive.is_some() || rollout.is_some() {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: "`ensemble` after another dispatch clause".into(),
                        expected: "a prompt template string (a prompt uses exactly one of `route:`, `progressive:`, `rollout`, `ensemble`, or `adversarial:`)".into(),
                    },
                    span: self.peek_span(),
                });
            }
            Some(self.parse_prompt_ensemble_clause()?)
        } else {
            None
        };

        // Phase 20h slice G: optional `adversarial:` block.
        // Mutually exclusive with every other dispatch clause.
        let adversarial = if matches!(self.peek(), TokKind::KwAdversarial) {
            if route.is_some()
                || progressive.is_some()
                || rollout.is_some()
                || ensemble.is_some()
            {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: "`adversarial:` after another dispatch clause".into(),
                        expected: "a prompt template string (a prompt uses exactly one of `route:`, `progressive:`, `rollout`, `ensemble`, or `adversarial:`)".into(),
                    },
                    span: self.peek_span(),
                });
            }
            Some(self.parse_prompt_adversarial_clause()?)
        } else {
            None
        };

        let stream = self.parse_prompt_stream_settings()?;

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
            effect_row,
            cites_strictly: None,
            stream,
            capability_required,
            route,
            progressive,
            rollout,
            ensemble,
            adversarial,
            span: start.merge(end),
        })
    }

    /// Parse:
    ///
    ///     adversarial:
    ///         propose: <model>
    ///         challenge: <model>
    ///         adjudicate: <model>
    ///
    /// Every stage is required. Caller has already positioned at
    /// the `adversarial` keyword.
    fn parse_prompt_adversarial_clause(
        &mut self,
    ) -> Result<AdversarialSpec, ParseError> {
        let start = self.peek_span();
        self.bump(); // adversarial
        self.expect(TokKind::Colon, "`:` after `adversarial`")?;
        self.expect_newline()?;

        if !matches!(self.peek(), TokKind::Indent) {
            return Err(ParseError {
                kind: ParseErrorKind::ExpectedBlock,
                span: self.peek_span(),
            });
        }
        self.bump(); // Indent

        self.skip_newlines();
        let proposer = self.parse_adversarial_stage(TokKind::KwPropose, "propose")?;
        self.skip_newlines();
        let challenger =
            self.parse_adversarial_stage(TokKind::KwChallenge, "challenge")?;
        self.skip_newlines();
        let adjudicator =
            self.parse_adversarial_stage(TokKind::KwAdjudicate, "adjudicate")?;
        self.skip_newlines();

        let end = self.peek_span();
        if matches!(self.peek(), TokKind::Dedent) {
            self.bump();
        }

        Ok(AdversarialSpec {
            proposer,
            challenger,
            adjudicator,
            span: start.merge(end),
        })
    }

    fn parse_adversarial_stage(
        &mut self,
        expected_kw: TokKind,
        label: &str,
    ) -> Result<Ident, ParseError> {
        if !matches!(self.peek(), k if k == &expected_kw) {
            return Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(self.peek()),
                    expected: format!("`{label}:` stage in adversarial block"),
                },
                span: self.peek_span(),
            });
        }
        self.bump(); // propose / challenge / adjudicate
        self.expect(TokKind::Colon, "`:` after adversarial stage")?;
        let (name, span) = self.expect_ident()?;
        self.expect_newline()?;
        Ok(Ident::new(name, span))
    }

    /// Parse `ensemble [<m1>, <m2>, <m3>] vote <strategy>`. Caller has
    /// already positioned at the `ensemble` keyword.
    fn parse_prompt_ensemble_clause(&mut self) -> Result<EnsembleSpec, ParseError> {
        let start = self.peek_span();
        self.bump(); // ensemble

        self.expect(TokKind::LBracket, "`[` after `ensemble`")?;

        let mut models = Vec::new();
        loop {
            if matches!(self.peek(), TokKind::RBracket) {
                break;
            }
            let (name, span) = self.expect_ident()?;
            models.push(Ident::new(name, span));
            match self.peek() {
                TokKind::Comma => {
                    self.bump();
                }
                TokKind::RBracket => break,
                other => {
                    return Err(ParseError {
                        kind: ParseErrorKind::UnexpectedToken {
                            got: describe_token(other),
                            expected: "`,` or `]` after ensemble model".into(),
                        },
                        span: self.peek_span(),
                    });
                }
            }
        }
        self.expect(TokKind::RBracket, "`]` after ensemble models")?;

        if models.len() < 2 {
            return Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: format!("{} ensemble model(s)", models.len()),
                    expected: "at least two models in the ensemble — voting is undefined with fewer".into(),
                },
                span: start,
            });
        }

        self.expect(TokKind::KwVote, "`vote` after ensemble model list")?;

        // Strategy ident. Currently only `majority` is supported.
        let vote = match self.peek().clone() {
            TokKind::Ident(name) if name == "majority" => {
                self.bump();
                VoteStrategy::Majority
            }
            other => {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: describe_token(&other),
                        expected: "a vote strategy (`majority`)".into(),
                    },
                    span: self.peek_span(),
                });
            }
        };

        let end = self.prev_span();
        self.expect_newline()?;

        Ok(EnsembleSpec {
            models,
            vote,
            span: start.merge(end),
        })
    }

    /// Parse `rollout N% <variant>, else <baseline>`. Caller has
    /// already positioned at the `rollout` keyword.
    fn parse_prompt_rollout_clause(&mut self) -> Result<RolloutSpec, ParseError> {
        let start = self.peek_span();
        self.bump(); // rollout

        // Percentage — accept Int or Float, mandatory `%`.
        let variant_percent = match self.peek().clone() {
            TokKind::Float(n) => {
                self.bump();
                n
            }
            TokKind::Int(n) => {
                self.bump();
                n as f64
            }
            other => {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: describe_token(&other),
                        expected: "a percentage after `rollout`".into(),
                    },
                    span: self.peek_span(),
                });
            }
        };
        self.expect(TokKind::Percent, "`%` after rollout percentage")?;

        let (variant_name, variant_span) = self.expect_ident()?;
        self.expect(TokKind::Comma, "`,` after rollout variant")?;
        self.expect(TokKind::KwElse, "`else` before rollout baseline")?;
        let (baseline_name, baseline_span) = self.expect_ident()?;
        let end = self.prev_span();
        self.expect_newline()?;

        Ok(RolloutSpec {
            variant_percent,
            variant: Ident::new(variant_name, variant_span),
            baseline: Ident::new(baseline_name, baseline_span),
            span: start.merge(end),
        })
    }

    /// Parse a `route:` block inside a prompt body. Caller has
    /// already positioned at the `route` keyword.
    fn parse_prompt_route_block(&mut self) -> Result<RouteTable, ParseError> {
        let start = self.peek_span();
        self.bump(); // route
        self.expect(TokKind::Colon, "`:` after `route`")?;
        self.expect_newline()?;

        if !matches!(self.peek(), TokKind::Indent) {
            return Err(ParseError {
                kind: ParseErrorKind::ExpectedBlock,
                span: self.peek_span(),
            });
        }
        self.bump(); // Indent

        let mut arms = Vec::new();
        while !matches!(self.peek(), TokKind::Dedent | TokKind::Eof) {
            self.skip_newlines();
            if matches!(self.peek(), TokKind::Dedent | TokKind::Eof) {
                break;
            }
            arms.push(self.parse_route_arm()?);
        }

        let end = self.peek_span();
        if matches!(self.peek(), TokKind::Dedent) {
            self.bump();
        }

        if arms.is_empty() {
            return Err(ParseError {
                kind: ParseErrorKind::ExpectedBlock,
                span: start,
            });
        }

        Ok(RouteTable {
            arms,
            span: start.merge(end),
        })
    }

    fn parse_route_arm(&mut self) -> Result<RouteArm, ParseError> {
        let arm_start = self.peek_span();

        // A bare `_` on its own is the wildcard pattern. Anything
        // else is a guard expression, which we parse with the full
        // expression grammar (boolean ops, comparisons, calls).
        let pattern = if self.is_wildcard_token() {
            let span = self.peek_span();
            self.bump();
            RoutePattern::Wildcard { span }
        } else {
            RoutePattern::Guard(self.parse_expr()?)
        };

        self.expect(TokKind::Arrow, "`->` after route pattern")?;
        let (model_name, model_span) = self.expect_ident()?;
        let end = self.prev_span();
        self.expect_newline()?;

        Ok(RouteArm {
            pattern,
            model: Ident::new(model_name, model_span),
            span: arm_start.merge(end),
        })
    }

    /// Is the next token a lone `_` identifier?
    fn is_wildcard_token(&self) -> bool {
        matches!(self.peek(), TokKind::Ident(name) if name == "_")
    }

    /// Parse a `progressive:` block inside a prompt body. Caller has
    /// already positioned at the `progressive` keyword.
    fn parse_prompt_progressive_block(
        &mut self,
    ) -> Result<ProgressiveChain, ParseError> {
        let start = self.peek_span();
        self.bump(); // progressive
        self.expect(TokKind::Colon, "`:` after `progressive`")?;
        self.expect_newline()?;

        if !matches!(self.peek(), TokKind::Indent) {
            return Err(ParseError {
                kind: ParseErrorKind::ExpectedBlock,
                span: self.peek_span(),
            });
        }
        self.bump(); // Indent

        let mut stages = Vec::new();
        while !matches!(self.peek(), TokKind::Dedent | TokKind::Eof) {
            self.skip_newlines();
            if matches!(self.peek(), TokKind::Dedent | TokKind::Eof) {
                break;
            }
            stages.push(self.parse_progressive_stage()?);
        }

        let end = self.peek_span();
        if matches!(self.peek(), TokKind::Dedent) {
            self.bump();
        }

        if stages.len() < 2 {
            return Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: format!("{} stage(s)", stages.len()),
                    expected: "at least two `progressive:` stages (primary + terminal fallback)".into(),
                },
                span: start,
            });
        }

        // Every stage except the last must declare a threshold.
        // The last stage must NOT declare one — it's the terminal
        // fallback that always runs.
        for (idx, stage) in stages.iter().enumerate() {
            let is_last = idx == stages.len() - 1;
            if is_last && stage.threshold.is_some() {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: "`below <threshold>` on the terminal stage".into(),
                        expected: "a bare model name on the last stage (terminal fallback always runs)".into(),
                    },
                    span: stage.span,
                });
            }
            if !is_last && stage.threshold.is_none() {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: "a non-terminal stage without `below <threshold>`".into(),
                        expected: "`<model> below <threshold>` on every stage except the last".into(),
                    },
                    span: stage.span,
                });
            }
        }

        Ok(ProgressiveChain {
            stages,
            span: start.merge(end),
        })
    }

    fn parse_progressive_stage(&mut self) -> Result<ProgressiveStage, ParseError> {
        let start = self.peek_span();
        let (model_name, model_span) = self.expect_ident()?;

        let threshold = if matches!(self.peek(), TokKind::KwBelow) {
            self.bump(); // below
            match self.peek().clone() {
                TokKind::Float(n) => {
                    self.bump();
                    Some(n)
                }
                TokKind::Int(n) => {
                    self.bump();
                    Some(n as f64)
                }
                other => {
                    return Err(ParseError {
                        kind: ParseErrorKind::UnexpectedToken {
                            got: describe_token(&other),
                            expected: "a numeric confidence threshold after `below`".into(),
                        },
                        span: self.peek_span(),
                    });
                }
            }
        } else {
            None
        };

        let end = self.prev_span();
        self.expect_newline()?;
        Ok(ProgressiveStage {
            model: Ident::new(model_name, model_span),
            threshold,
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
        let effect_row = self.parse_uses_clause()?;
        self.expect(TokKind::Colon, "`:` after agent signature")?;
        self.expect_newline()?;

        let body = self.parse_indented_block()?;
        let end = body.span;

        Ok(AgentDecl {
            name: Ident::new(name, name_span),
            params,
            return_ty,
            body,
            effect_row,
            constraints: Vec::new(),
            span: start.merge(end),
        })
    }

    // -- eval ----------------------------------------------------

    fn parse_eval_decl(&mut self) -> Result<EvalDecl, ParseError> {
        let start = self.peek_span();
        self.bump(); // eval

        let (name, name_span) = self.expect_ident()?;
        self.expect(TokKind::Colon, "`:` after eval name")?;
        self.expect_newline()?;

        if !matches!(self.peek(), TokKind::Indent) {
            return Err(ParseError {
                kind: ParseErrorKind::ExpectedBlock,
                span: self.peek_span(),
            });
        }
        self.bump(); // Indent

        let mut stmts = Vec::new();
        let mut assertions = Vec::new();
        let mut saw_assert = false;

        while !matches!(self.peek(), TokKind::Dedent | TokKind::Eof) {
            self.skip_newlines();
            if matches!(self.peek(), TokKind::Dedent | TokKind::Eof) {
                break;
            }
            if matches!(self.peek(), TokKind::KwAssert) {
                saw_assert = true;
                assertions.push(self.parse_eval_assert()?);
                continue;
            }
            if saw_assert {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: describe_token(self.peek()),
                        expected: "only `assert ...` lines after the first eval assertion".into(),
                    },
                    span: self.peek_span(),
                });
            }
            stmts.push(self.parse_stmt()?);
        }

        let end = self.peek_span();
        if matches!(self.peek(), TokKind::Dedent) {
            self.bump();
        }

        Ok(EvalDecl {
            name: Ident::new(name, name_span),
            body: Block {
                stmts,
                span: start.merge(end),
            },
            assertions,
            span: start.merge(end),
        })
    }

    fn parse_eval_assert(&mut self) -> Result<EvalAssert, ParseError> {
        let start = self.peek_span();
        self.bump(); // assert

        let assert_node = match self.peek().clone() {
            TokKind::Ident(keyword) if keyword == "called" => {
                self.bump();
                let (first_name, first_span) = self.expect_ident()?;
                let first = Ident::new(first_name, first_span);
                if self.peek_ident_is("before") {
                    self.bump();
                    let (second_name, second_span) = self.expect_ident()?;
                    EvalAssert::Ordering {
                        before: first,
                        after: Ident::new(second_name, second_span),
                        span: start.merge(second_span),
                    }
                } else {
                    EvalAssert::Called {
                        tool: first,
                        span: start.merge(first_span),
                    }
                }
            }
            TokKind::Ident(keyword) if keyword == "approved" => {
                self.bump();
                let (label, span) = self.expect_ident()?;
                EvalAssert::Approved {
                    label: Ident::new(label, span),
                    span: start.merge(span),
                }
            }
            TokKind::Ident(keyword) if keyword == "cost" => {
                self.bump();
                let op = match self.peek() {
                    TokKind::Eq => BinaryOp::Eq,
                    TokKind::NotEq => BinaryOp::NotEq,
                    TokKind::Lt => BinaryOp::Lt,
                    TokKind::LtEq => BinaryOp::LtEq,
                    TokKind::Gt => BinaryOp::Gt,
                    TokKind::GtEq => BinaryOp::GtEq,
                    other => {
                        return Err(ParseError {
                            kind: ParseErrorKind::UnexpectedToken {
                                got: describe_token(other),
                                expected: "a comparison operator after `assert cost`".into(),
                            },
                            span: self.peek_span(),
                        })
                    }
                };
                self.bump();
                let bound_span = self.peek_span();
                let bound = self.parse_cost_literal()?;
                EvalAssert::Cost {
                    op,
                    bound,
                    span: start.merge(bound_span),
                }
            }
            _ => {
                let expr = self.parse_expr()?;
                let mut confidence = None;
                let mut runs = None;
                let mut end = expr.span();
                if self.peek_ident_is("with") {
                    self.bump();
                    self.expect_contextual_ident("confidence")?;
                    confidence = Some(self.parse_confidence_literal()?);
                    self.expect_contextual_ident("over")?;
                    let (run_count, run_span) = self.expect_positive_int_literal("a positive run count")?;
                    self.expect_contextual_ident("runs")?;
                    runs = Some(run_count);
                    end = end.merge(run_span);
                }
                EvalAssert::Value {
                    expr,
                    confidence,
                    runs,
                    span: start.merge(end),
                }
            }
        };

        self.expect_newline()?;
        Ok(assert_node)
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

    fn parse_confidence_literal(&mut self) -> Result<f64, ParseError> {
        let span = self.peek_span();
        match self.peek().clone() {
            TokKind::Float(value) => {
                self.bump();
                Ok(value)
            }
            TokKind::Int(value) => {
                self.bump();
                Ok(value as f64)
            }
            other => Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(&other),
                    expected: "a numeric confidence literal".into(),
                },
                span,
            }),
        }
    }

    fn expect_positive_int_literal(
        &mut self,
        expected: &str,
    ) -> Result<(u64, Span), ParseError> {
        let span = self.peek_span();
        match self.peek().clone() {
            TokKind::Int(value) if value > 0 => {
                self.bump();
                Ok((value as u64, span))
            }
            other => Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(&other),
                    expected: expected.into(),
                },
                span,
            }),
        }
    }

    fn parse_cost_literal(&mut self) -> Result<f64, ParseError> {
        self.expect(TokKind::Dollar, "`$` before cost literal")?;
        let span = self.peek_span();
        match self.peek().clone() {
            TokKind::Float(value) => {
                self.bump();
                Ok(value)
            }
            TokKind::Int(value) => {
                self.bump();
                Ok(value as f64)
            }
            other => Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(&other),
                    expected: "a numeric cost literal after `$`".into(),
                },
                span,
            }),
        }
    }

    fn parse_uses_clause(&mut self) -> Result<EffectRow, ParseError> {
        if !matches!(self.peek(), TokKind::KwUses) {
            return Ok(EffectRow::default());
        }
        let start = self.peek_span();
        self.bump(); // uses
        let mut effects = Vec::new();
        let (first_name, first_span) = self.expect_ident()?;
        effects.push(EffectRef {
            name: Ident::new(first_name, first_span),
            span: first_span,
        });
        while matches!(self.peek(), TokKind::Comma) {
            self.bump();
            let (name, span) = self.expect_ident()?;
            effects.push(EffectRef {
                name: Ident::new(name, span),
                span,
            });
        }
        let end = effects.last().map(|e| e.span).unwrap_or(start);
        Ok(EffectRow {
            effects,
            span: start.merge(end),
        })
    }

    fn parse_prompt_stream_settings(&mut self) -> Result<PromptStreamSettings, ParseError> {
        let mut settings = PromptStreamSettings::default();
        while self.peek_ident_is("with") {
            self.bump(); // with
            let (name, span) = self.expect_ident()?;
            match name.as_str() {
                "min_confidence" => {
                    settings.min_confidence = Some(self.parse_confidence_literal()?);
                }
                "max_tokens" => {
                    let (max_tokens, _) =
                        self.expect_positive_int_literal("a positive max token count")?;
                    settings.max_tokens = Some(max_tokens);
                }
                "backpressure" => {
                    settings.backpressure = Some(self.parse_backpressure_policy()?);
                }
                _ => {
                    return Err(ParseError {
                        kind: ParseErrorKind::UnexpectedToken {
                            got: format!("identifier `{name}`"),
                            expected: "`min_confidence`, `max_tokens`, or `backpressure`".into(),
                        },
                        span,
                    });
                }
            }
            self.expect_newline()?;
        }
        Ok(settings)
    }

    fn parse_backpressure_policy(&mut self) -> Result<BackpressurePolicy, ParseError> {
        let span = self.peek_span();
        match self.peek().clone() {
            TokKind::Ident(name) if name == "unbounded" => {
                self.bump();
                Ok(BackpressurePolicy::Unbounded)
            }
            TokKind::Ident(name) if name == "bounded" => {
                self.bump();
                self.expect(TokKind::LParen, "`(` after `bounded`")?;
                let (capacity, _) =
                    self.expect_positive_int_literal("a positive bounded backpressure size")?;
                self.expect(TokKind::RParen, "`)` after bounded backpressure size")?;
                Ok(BackpressurePolicy::Bounded(capacity))
            }
            other => Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(&other),
                    expected: "`bounded(N)` or `unbounded`".into(),
                },
                span,
            }),
        }
    }

    fn parse_constraints(&mut self) -> Result<Vec<EffectConstraint>, ParseError> {
        let mut constraints = Vec::new();
        while matches!(self.peek(), TokKind::At) {
            let start = self.peek_span();
            self.bump(); // @
            let (name, name_span) = self.expect_ident()?;
            if name == "budget" && matches!(self.peek(), TokKind::LParen) {
                constraints.extend(self.parse_budget_constraints(start, name_span)?);
                self.expect_newline()?;
                continue;
            }
            let value = if matches!(self.peek(), TokKind::LParen) {
                self.bump();
                if matches!(self.peek(), TokKind::RParen) {
                    self.bump();
                    None
                } else {
                    let value = self.parse_dimension_value()?;
                    self.expect(TokKind::RParen, "`)` after constraint value")?;
                    Some(value)
                }
            } else {
                None
            };
            let end = self.prev_span();
            self.expect_newline()?;
            constraints.push(EffectConstraint {
                dimension: Ident::new(name, name_span),
                value,
                span: start.merge(end),
            });
        }
        Ok(constraints)
    }

    fn parse_budget_constraints(
        &mut self,
        start: Span,
        name_span: Span,
    ) -> Result<Vec<EffectConstraint>, ParseError> {
        self.expect(TokKind::LParen, "`(` after `@budget`")?;
        let mut constraints = Vec::new();

        if !matches!(self.peek(), TokKind::RParen) {
            loop {
                if matches!(self.peek(), TokKind::Dollar) {
                    let value = self.parse_dimension_value()?;
                    constraints.push(EffectConstraint {
                        dimension: Ident::new("cost", name_span),
                        value: Some(value),
                        span: start.merge(self.prev_span()),
                    });
                } else {
                    let (dim_name, dim_span) = self.expect_ident()?;
                    self.expect(TokKind::Colon, "`:` after budget dimension name")?;
                    let value = self.parse_dimension_value()?;
                    let canonical_name = match dim_name.as_str() {
                        "latency" => "latency_ms".to_string(),
                        other => other.to_string(),
                    };
                    constraints.push(EffectConstraint {
                        dimension: Ident::new(canonical_name, dim_span),
                        value: Some(value),
                        span: start.merge(self.prev_span()),
                    });
                }

                if !matches!(self.peek(), TokKind::Comma) {
                    break;
                }
                self.bump();
            }
        }

        self.expect(TokKind::RParen, "`)` after budget constraints")?;
        Ok(constraints)
    }

    fn consume_optional_duration_suffix(&mut self, value: f64) -> f64 {
        match self.peek() {
            TokKind::Ident(unit) if unit == "ms" => {
                self.bump();
                value
            }
            TokKind::Ident(unit) if unit == "s" => {
                self.bump();
                value * 1000.0
            }
            _ => value,
        }
    }

    // -- extend methods ------------------------------

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
            // Only `package` is accepted inside public(...) today.
            // Future work may add effect-scoped variants.
            match self.peek() {
                TokKind::KwPackage => {
                    self.bump();
                }
                other => {
                    return Err(ParseError {
                        kind: ParseErrorKind::UnexpectedToken {
                            got: describe_token(other),
                            expected: "`package` inside `public(...)` (the only supported variant today)".into(),
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


mod types;

#[cfg(test)]
mod tests;
