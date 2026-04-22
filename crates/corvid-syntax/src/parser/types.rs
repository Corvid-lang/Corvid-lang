//! Type-reference parsing: `Name`, `Generic<T>`, `Weak<T, {effects}>`.
//!
//! Extracted from `parser.rs` as part of Phase 20i responsibility
//! decomposition. All four functions operate on the `Parser` state
//! machine via `impl<'a> Parser<'a>`.

use super::{describe_token, Parser};
use crate::errors::{ParseError, ParseErrorKind};
use crate::token::TokKind;
use corvid_ast::{Ident, TypeRef, WeakEffect, WeakEffectRow};

impl<'a> Parser<'a> {
    pub(super) fn parse_type_ref(&mut self) -> Result<TypeRef, ParseError> {
        let (name, name_span) = self.expect_ident()?;
        let name_ident = Ident::new(name, name_span);

        // Qualified type: `alias.TypeName` — resolved through a
        // Corvid-file import alias. Recognized at parse time;
        // resolver does the module lookup in
        // `lang-cor-imports-basic-resolve`.
        if matches!(self.peek(), TokKind::Dot) {
            self.bump(); // .
            let (member, member_span) = self.expect_ident()?;
            return Ok(TypeRef::Qualified {
                alias: name_ident,
                name: Ident::new(member, member_span),
                span: name_span.merge(member_span),
            });
        }

        if !matches!(self.peek(), TokKind::Lt) {
            return Ok(TypeRef::Named {
                name: name_ident,
                span: name_span,
            });
        }

        self.bump(); // <
        if name_ident.name == "Weak" {
            let inner = self.parse_type_ref()?;
            let effects = if matches!(self.peek(), TokKind::Comma) {
                self.bump();
                Some(self.parse_weak_effect_row()?)
            } else {
                None
            };
            let end_span = self.peek_span();
            self.expect(TokKind::Gt, "`>` to close Weak type arguments")?;
            return Ok(TypeRef::Weak {
                inner: Box::new(inner),
                effects,
                span: name_span.merge(end_span),
            });
        }

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

    pub(super) fn parse_weak_effect_row(&mut self) -> Result<WeakEffectRow, ParseError> {
        let start = self.peek_span();
        self.expect(TokKind::LBrace, "`{` to start Weak effect row")?;
        let mut effects = Vec::new();
        if !matches!(self.peek(), TokKind::RBrace) {
            effects.push(self.parse_weak_effect_name()?);
            while matches!(self.peek(), TokKind::Comma) {
                self.bump();
                effects.push(self.parse_weak_effect_name()?);
            }
        }
        let end = self.peek_span();
        self.expect(TokKind::RBrace, "`}` to close Weak effect row")?;
        let row = WeakEffectRow::from_effects(&effects);
        if row == WeakEffectRow::empty() {
            return Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: "empty effect row".into(),
                    expected: "one or more Weak effects".into(),
                },
                span: start.merge(end),
            });
        }
        Ok(row)
    }

    pub(super) fn parse_weak_effect_name(&mut self) -> Result<WeakEffect, ParseError> {
        let span = self.peek_span();
        match self.peek().clone() {
            TokKind::Ident(name) => {
                self.bump();
                match name.as_str() {
                    "tool_call" => Ok(WeakEffect::ToolCall),
                    "llm" => Ok(WeakEffect::Llm),
                    "approve" => Ok(WeakEffect::Approve),
                    _ => Err(ParseError {
                        kind: ParseErrorKind::UnexpectedToken {
                            got: format!("identifier `{name}`"),
                            expected: "`tool_call`, `llm`, or `approve`".into(),
                        },
                        span,
                    }),
                }
            }
            other => Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(&other),
                    expected: "a Weak effect name".into(),
                },
                span,
            }),
        }
    }

    pub(super) fn parse_namespaced_ident_from(
        &mut self,
        mut name: String,
    ) -> Result<String, ParseError> {
        if !matches!(self.peek(), TokKind::Colon) {
            return Ok(name);
        }
        let saved_pos = self.pos;
        self.bump();
        if !matches!(self.peek(), TokKind::Colon) {
            self.pos = saved_pos;
            return Ok(name);
        }
        self.bump();
        let (suffix, _) = self.expect_ident()?;
        name.push_str("::");
        name.push_str(&suffix);
        Ok(name)
    }
}
