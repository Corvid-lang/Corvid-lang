//! Expression parsing — Pratt-style precedence climbing.
//!
//! Top-down flow: parse_expr -> parse_or -> parse_and -> parse_not
//! -> parse_cmp -> parse_add -> parse_mul -> parse_unary ->
//! parse_postfix -> parse_primary (+ parse_try_retry_expr on `?`
//! and `try` forms). parse_u64_literal is a small shared helper
//! used by both expression parsing and statement-level count
//! literals.
//!
//! Extracted from `parser.rs` as part of Phase 20i responsibility
//! decomposition.

use super::{describe_token, Parser};
use crate::errors::{ParseError, ParseErrorKind};
use crate::token::TokKind;
use corvid_ast::{Backoff, BinaryOp, Expr, Ident, Literal, Span, UnaryOp};

impl<'a> Parser<'a> {
    // ------------------------------------------------------------
    // Expression entry point.
    // ------------------------------------------------------------

    pub(super) fn parse_expr(&mut self) -> Result<Expr, ParseError> {
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
            TokKind::KwReplay => self.parse_replay_expr(),
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

    pub(super) fn parse_u64_literal(&mut self, description: &str) -> Result<u64, ParseError> {
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
}
