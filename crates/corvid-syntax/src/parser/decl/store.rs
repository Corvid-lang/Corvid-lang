//! `session` / `memory` store declaration parsing — record-shaped
//! state with optional `policy` clauses.

use crate::errors::{ParseError, ParseErrorKind};
use crate::parser::Parser;
use crate::token::TokKind;
use corvid_ast::{Ident, StoreDecl, StoreKind, StorePolicy, Visibility};

impl<'a> Parser<'a> {
    pub(super) fn parse_store_decl(
        &mut self,
        kind: StoreKind,
        visibility: Visibility,
    ) -> Result<StoreDecl, ParseError> {
        let start = self.peek_span();
        self.bump(); // session / memory

        let (name, name_span) = self.expect_ident()?;
        self.expect(TokKind::Colon, "`:` after store name")?;
        self.expect_newline()?;

        if !matches!(self.peek(), TokKind::Indent) {
            return Err(ParseError {
                kind: ParseErrorKind::ExpectedBlock,
                span: self.peek_span(),
            });
        }
        self.bump(); // Indent

        let mut fields = Vec::new();
        let mut policies = Vec::new();
        while !matches!(self.peek(), TokKind::Dedent | TokKind::Eof) {
            self.skip_newlines();
            if matches!(self.peek(), TokKind::Dedent | TokKind::Eof) {
                break;
            }
            let parsed = if matches!(self.peek(), TokKind::Ident(word) if word == "policy") {
                self.parse_store_policy().map(|policy| {
                    policies.push(policy);
                })
            } else {
                self.parse_field().map(|field| {
                    fields.push(field);
                })
            };
            if let Err(e) = parsed {
                self.errors.push(e);
                self.sync_to_statement_boundary();
            }
        }
        let end = self.peek_span();
        if matches!(self.peek(), TokKind::Dedent) {
            self.bump();
        }

        Ok(StoreDecl {
            kind,
            name: Ident::new(name, name_span),
            fields,
            policies,
            visibility,
            span: start.merge(end),
        })
    }

    fn parse_store_policy(&mut self) -> Result<StorePolicy, ParseError> {
        let start = self.peek_span();
        self.bump(); // policy
        let (name, name_span) = self.expect_ident()?;
        self.expect(TokKind::Colon, "`:` after store policy name")?;
        let value = self.parse_dimension_value()?;
        let end = self.prev_span();
        self.expect_newline()?;
        Ok(StorePolicy {
            name: Ident::new(name, name_span),
            value,
            span: start.merge(end),
        })
    }
}
