//! Parser for the `replay` expression (Phase 21 slice 21-inv-E-1).
//!
//! Grammar:
//!
//! ```text
//! replay_expr    := 'replay' expr ':' NEWLINE INDENT arm+ else_arm DEDENT
//! arm            := 'when' event_pattern '->' expr NEWLINE
//! else_arm       := 'else' expr NEWLINE
//! event_pattern  := 'llm'     '(' STRING ')'
//!                 | 'tool'    '(' STRING ',' arg_pattern ')'
//!                 | 'approve' '(' STRING ')'
//! arg_pattern    := '_' | STRING
//! ```
//!
//! Shape rules enforced here:
//! - Exactly one `else` arm, and it must be the last arm in the
//!   block. `when` arms are optional — `replay <trace>: else <expr>`
//!   is the minimal valid form.
//! - `llm` / `tool` / `approve` are contextual identifiers (not
//!   keywords) so they don't claim those names at file scope.
//! - `_` is parsed from the lexed `Ident("_")` token — the lexer
//!   treats underscore as an identifier start/continue.
//!
//! Resolver-level work (trace-id locals, pattern-type resolution)
//! lands in 21-inv-E-2.

use super::{describe_token, Parser};
use crate::errors::{ParseError, ParseErrorKind};
use crate::token::TokKind;
use corvid_ast::{Expr, Ident, ReplayArm, ReplayPattern, ToolArgPattern};

/// Internal classifier for the three event-kind words. `tool` and
/// `approve` are lexed as keywords; `llm` is lexed as an
/// identifier. Normalizing both into this enum keeps the pattern
/// dispatch flat.
enum EventKind {
    Llm,
    Tool,
    Approve,
}

impl<'a> Parser<'a> {
    /// Parse a `replay <expr>: INDENT arms DEDENT` block. Caller
    /// guarantees the current token is `KwReplay`.
    pub(super) fn parse_replay_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.peek_span();
        self.bump(); // `replay`

        let trace = self.parse_expr()?;

        self.expect(TokKind::Colon, "`:` after `replay <trace>`")?;
        self.expect_newline()?;

        if !matches!(self.peek(), TokKind::Indent) {
            return Err(ParseError {
                kind: ParseErrorKind::ExpectedBlock,
                span: self.peek_span(),
            });
        }
        self.bump(); // `Indent`

        let mut arms: Vec<ReplayArm> = Vec::new();
        let mut else_body: Option<Expr> = None;

        loop {
            self.skip_newlines();
            if matches!(self.peek(), TokKind::Dedent | TokKind::Eof) {
                break;
            }

            match self.peek() {
                TokKind::KwWhen => {
                    if else_body.is_some() {
                        return Err(ParseError {
                            kind: ParseErrorKind::UnexpectedToken {
                                got: "`when` arm after `else`".into(),
                                expected: "`else` must be the final arm of a replay block".into(),
                            },
                            span: self.peek_span(),
                        });
                    }
                    let arm = self.parse_replay_when_arm()?;
                    arms.push(arm);
                }
                TokKind::KwElse => {
                    if else_body.is_some() {
                        return Err(ParseError {
                            kind: ParseErrorKind::UnexpectedToken {
                                got: "second `else` arm".into(),
                                expected: "a replay block has exactly one `else` arm".into(),
                            },
                            span: self.peek_span(),
                        });
                    }
                    self.bump(); // `else`
                    let body = self.parse_expr()?;
                    else_body = Some(body);
                }
                other => {
                    return Err(ParseError {
                        kind: ParseErrorKind::UnexpectedToken {
                            got: describe_token(other),
                            expected: "`when <pattern> -> <expr>` or `else <expr>`".into(),
                        },
                        span: self.peek_span(),
                    });
                }
            }
            self.expect_newline()?;
        }

        let end = self.peek_span();
        if matches!(self.peek(), TokKind::Dedent) {
            self.bump();
        }

        let Some(else_body) = else_body else {
            return Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: "replay block without an `else` arm".into(),
                    expected: "every replay block must end with `else <expr>` (fallback is required)".into(),
                },
                span: start,
            });
        };

        Ok(Expr::Replay {
            trace: Box::new(trace),
            arms,
            else_body: Box::new(else_body),
            span: start.merge(end),
        })
    }

    fn parse_replay_when_arm(&mut self) -> Result<ReplayArm, ParseError> {
        let start = self.peek_span();
        self.bump(); // `when`

        let pattern = self.parse_replay_pattern()?;

        // Optional `as <ident>` tail — binds the matched event's
        // recorded payload as a local visible in the arm body.
        let capture = if matches!(self.peek(), TokKind::KwAs) {
            self.bump(); // `as`
            let (name, span) = self.expect_ident()?;
            Some(Ident::new(name, span))
        } else {
            None
        };

        self.expect(TokKind::Arrow, "`->` after replay pattern")?;
        let body = self.parse_expr()?;

        let span = start.merge(body.span());
        Ok(ReplayArm {
            pattern,
            capture,
            body,
            span,
        })
    }

    fn parse_replay_pattern(&mut self) -> Result<ReplayPattern, ParseError> {
        let start = self.peek_span();

        // The event-kind words `tool` and `approve` are already
        // Corvid keywords (`KwTool`, `KwApprove`) and must be
        // matched as such; `llm` is not a keyword so it lexes as
        // an identifier.
        let kind = match self.peek().clone() {
            TokKind::KwTool => EventKind::Tool,
            TokKind::KwApprove => EventKind::Approve,
            TokKind::Ident(name) if name == "llm" => EventKind::Llm,
            other => {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: describe_token(&other),
                        expected: "`llm`, `tool`, or `approve`".into(),
                    },
                    span: start,
                });
            }
        };
        self.bump();

        self.expect(TokKind::LParen, "`(` after event kind")?;

        let pattern = match kind {
            EventKind::Llm => {
                let prompt = self.expect_string_lit("prompt name string literal")?;
                let end = self.expect(TokKind::RParen, "`)` after `llm(\"<prompt>\")`")?;
                ReplayPattern::Llm {
                    prompt,
                    span: start.merge(end),
                }
            }
            EventKind::Tool => {
                let tool_name = self.expect_string_lit("tool name string literal")?;
                self.expect(TokKind::Comma, "`,` after tool name")?;
                let arg = self.parse_tool_arg_pattern()?;
                let end = self.expect(
                    TokKind::RParen,
                    "`)` after `tool(\"<name>\", <arg_pattern>)`",
                )?;
                ReplayPattern::Tool {
                    tool: tool_name,
                    arg,
                    span: start.merge(end),
                }
            }
            EventKind::Approve => {
                let label = self.expect_string_lit("approval label string literal")?;
                let end = self.expect(TokKind::RParen, "`)` after `approve(\"<label>\")`")?;
                ReplayPattern::Approve {
                    label,
                    span: start.merge(end),
                }
            }
        };

        Ok(pattern)
    }

    fn parse_tool_arg_pattern(&mut self) -> Result<ToolArgPattern, ParseError> {
        let span = self.peek_span();
        match self.peek().clone() {
            // `_` lexes as an identifier but its semantics are
            // "match anything, bind nothing." Distinguished from
            // a real capture before the generic ident branch.
            TokKind::Ident(name) if name == "_" => {
                self.bump();
                Ok(ToolArgPattern::Wildcard { span })
            }
            TokKind::StringLit(value) => {
                self.bump();
                Ok(ToolArgPattern::StringLit { value, span })
            }
            // A bare identifier binds the recorded tool-arg value
            // as a local visible in the arm body. Resolver wires
            // the scope in E-2b.
            TokKind::Ident(name) => {
                self.bump();
                Ok(ToolArgPattern::Capture {
                    name: Ident::new(name, span),
                    span,
                })
            }
            other => Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(&other),
                    expected: "`_` wildcard, an identifier capture, or a string literal".into(),
                },
                span,
            }),
        }
    }

    fn expect_string_lit(&mut self, description: &str) -> Result<String, ParseError> {
        let span = self.peek_span();
        match self.peek().clone() {
            TokKind::StringLit(s) => {
                self.bump();
                Ok(s)
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
