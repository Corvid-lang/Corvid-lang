//! `eval`, `test`, `fixture`, and `mock` declaration parsing —
//! plus the shared assertion-block parser and the
//! `assert ...` / `assert_snapshot ...` line grammar that lives
//! at the foot of an eval/test body.

use crate::errors::{ParseError, ParseErrorKind};
use crate::parser::{describe_token, Parser};
use crate::token::TokKind;
use corvid_ast::{
    BinaryOp, Block, EvalAssert, EvalDecl, FixtureDecl, Ident, MockDecl, Span, TestDecl,
};

impl<'a> Parser<'a> {
    pub(super) fn parse_eval_decl(&mut self) -> Result<EvalDecl, ParseError> {
        let start = self.peek_span();
        self.bump(); // eval

        let (name, name_span) = self.expect_ident()?;
        let (body, assertions, end) = self.parse_assertion_block("eval")?;

        Ok(EvalDecl {
            name: Ident::new(name, name_span),
            body,
            assertions,
            span: start.merge(end),
        })
    }

    pub(super) fn parse_test_decl(&mut self) -> Result<TestDecl, ParseError> {
        let start = self.peek_span();
        self.bump(); // test

        let (name, name_span) = self.expect_ident()?;
        let trace_fixture = if self.peek_ident_is("from_trace") {
            self.bump();
            match self.peek().clone() {
                TokKind::StringLit(path) => {
                    self.bump();
                    Some(path)
                }
                other => {
                    return Err(ParseError {
                        kind: ParseErrorKind::UnexpectedToken {
                            got: describe_token(&other),
                            expected: "a string trace fixture path after `from_trace`".into(),
                        },
                        span: self.peek_span(),
                    });
                }
            }
        } else {
            None
        };
        let (body, assertions, end) = self.parse_assertion_block("test")?;

        Ok(TestDecl {
            name: Ident::new(name, name_span),
            trace_fixture,
            body,
            assertions,
            span: start.merge(end),
        })
    }

    pub(super) fn parse_fixture_decl(&mut self) -> Result<FixtureDecl, ParseError> {
        let start = self.peek_span();
        self.bump(); // fixture

        let (name, name_span) = self.expect_ident()?;
        let params = self.parse_params()?;
        self.expect(TokKind::Arrow, "`->` before fixture return type")?;
        let return_ty = self.parse_type_ref()?;
        self.expect(TokKind::Colon, "`:` after fixture signature")?;
        self.expect_newline()?;

        let body = self.parse_indented_block()?;
        let end = body.span;
        Ok(FixtureDecl {
            name: Ident::new(name, name_span),
            params,
            return_ty,
            body,
            span: start.merge(end),
        })
    }

    pub(super) fn parse_mock_decl(&mut self) -> Result<MockDecl, ParseError> {
        let start = self.peek_span();
        self.bump(); // mock

        let (target, target_span) = self.expect_ident()?;
        let params = self.parse_params()?;
        self.expect(TokKind::Arrow, "`->` before mock return type")?;
        let return_ty = self.parse_type_ref()?;
        let effect_row = self.parse_uses_clause()?;
        self.expect(TokKind::Colon, "`:` after mock signature")?;
        self.expect_newline()?;

        let body = self.parse_indented_block()?;
        let end = body.span;
        Ok(MockDecl {
            target: Ident::new(target, target_span),
            params,
            return_ty,
            body,
            effect_row,
            span: start.merge(end),
        })
    }

    fn parse_assertion_block(
        &mut self,
        decl_kind: &'static str,
    ) -> Result<(Block, Vec<EvalAssert>, Span), ParseError> {
        let start = self.peek_span();
        self.expect(TokKind::Colon, &format!("`:` after {decl_kind} name"))?;
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
            if matches!(self.peek(), TokKind::KwAssert | TokKind::KwAssertSnapshot) {
                saw_assert = true;
                assertions.push(self.parse_eval_assertion_line()?);
                continue;
            }
            if saw_assert {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: describe_token(self.peek()),
                        expected: format!(
                            "only `assert ...` lines after the first {decl_kind} assertion"
                        ),
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

        Ok((
            Block {
                stmts,
                span: start.merge(end),
            },
            assertions,
            end,
        ))
    }

    fn parse_eval_assertion_line(&mut self) -> Result<EvalAssert, ParseError> {
        if matches!(self.peek(), TokKind::KwAssertSnapshot) {
            return self.parse_snapshot_assert();
        }
        self.parse_eval_assert()
    }

    fn parse_snapshot_assert(&mut self) -> Result<EvalAssert, ParseError> {
        let start = self.peek_span();
        self.bump(); // assert_snapshot
        let expr = self.parse_expr()?;
        let end = expr.span();
        self.expect_newline()?;
        Ok(EvalAssert::Snapshot {
            expr,
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
}
