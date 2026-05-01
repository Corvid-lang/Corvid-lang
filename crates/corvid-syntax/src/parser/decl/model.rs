//! `model` declaration parsing — Phase 20h primitive that pins
//! provider/temperature/etc. configuration as a named, reusable
//! block of dimension-style fields.

use crate::errors::{ParseError, ParseErrorKind};
use crate::parser::Parser;
use crate::token::TokKind;
use corvid_ast::{Ident, ModelDecl, ModelField};

impl<'a> Parser<'a> {
    pub(super) fn parse_model_decl(&mut self) -> Result<ModelDecl, ParseError> {
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
}
