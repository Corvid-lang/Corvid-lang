//! Numeric literal helpers — confidence / positive-int / cost.
//!
//! Cross-cutting helpers used by prompt stream settings, route
//! arms, progressive thresholds, eval assertions, model fields,
//! and `@cost` / `@budget` constraints. Each parses a single
//! numeric token (optionally with a leading `$`) and produces a
//! typed value.
//!
//! Extracted from `parser.rs` as part of Phase 20i responsibility
//! decomposition.

use super::{describe_token, Parser};
use crate::errors::{ParseError, ParseErrorKind};
use crate::token::TokKind;
use corvid_ast::Span;

impl<'a> Parser<'a> {
    pub(super) fn parse_confidence_literal(&mut self) -> Result<f64, ParseError> {
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

    pub(super) fn expect_positive_int_literal(
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

    pub(super) fn parse_cost_literal(&mut self) -> Result<f64, ParseError> {
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
}
