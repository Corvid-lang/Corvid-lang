//! Top-level declaration parsing — the `parse_decl` dispatch and
//! every decl parser except `parse_prompt_decl` (which lives in
//! `parser/prompt.rs` alongside its dispatch-clause helpers).
//!
//! Covers: import, type + field, tool, effect, dimension, model
//! + model field + dimension value, agent, eval + eval assertion.
//!
//! Extracted from `parser.rs` as part of Phase 20i responsibility
//! decomposition.

use super::{describe_token, Parser};
use crate::errors::{ParseError, ParseErrorKind};
use crate::token::TokKind;
use corvid_ast::{
    AgentDecl, BinaryOp, Block, Decl, DimensionDecl, DimensionValue, Effect, EffectDecl,
    EvalAssert, EvalDecl, ExtendDecl, ExtendMethod, ExtendMethodKind, Field, Ident, ImportDecl,
    ImportSource, ModelDecl, ModelField, Param, ToolDecl, TypeDecl, Visibility,
};

impl<'a> Parser<'a> {
    pub(super) fn parse_decl(&mut self) -> Result<Decl, ParseError> {
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

    pub(super) fn parse_tool_decl(&mut self) -> Result<ToolDecl, ParseError> {
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

    pub(super) fn parse_dimension_value(&mut self) -> Result<DimensionValue, ParseError> {
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

    // -- agent ---------------------------------------------------

    pub(super) fn parse_agent_decl(&mut self) -> Result<AgentDecl, ParseError> {
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
    pub(super) fn parse_params(&mut self) -> Result<Vec<Param>, ParseError> {
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
