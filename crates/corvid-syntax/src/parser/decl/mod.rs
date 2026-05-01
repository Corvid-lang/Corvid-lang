//! Top-level declaration parsing — the `parse_decl` dispatch and
//! every decl parser except `parse_prompt_decl` (which lives in
//! `parser/prompt.rs` alongside its dispatch-clause helpers).
//!
//! Covers: import, type + field, tool, effect, dimension, model
//! + model field + dimension value, agent, eval/test + assertion.
//!
//! Extracted from `parser.rs` as part of Phase 20i responsibility
//! decomposition.

use super::{describe_token, Parser};
use crate::errors::{ParseError, ParseErrorKind};
use crate::token::TokKind;
use corvid_ast::{
    AgentDecl, BinaryOp, Block, Decl, DimensionDecl, DimensionValue, Effect, EffectDecl,
    ExternAbi, OwnershipAnnotation, OwnershipMode,
    EvalAssert, EvalDecl, ExtendDecl, ExtendMethod, ExtendMethodKind, FixtureDecl, Ident,
    HttpMethod, HttpRouteDecl,
    MockDecl, ModelDecl, ModelField, Param, RoutePathParam, RouteResponse, RouteResponseKind,
    ScheduleDecl, ServerDecl, Span, StoreKind, ToolDecl, TestDecl,
    TypeRef, Visibility,
};

mod import;
mod store;
mod type_field;

impl<'a> Parser<'a> {
    pub(super) fn parse_decl(&mut self) -> Result<Decl, ParseError> {
        // Optional `public` / `public(package)` visibility prefix on
        // top-level type / tool / prompt / agent declarations. The
        // visibility modifier becomes load-bearing once cross-file
        // `.cor` imports land; on its own, it changes nothing about
        // existing single-file programs because same-file callers see
        // both `public` and private items regardless.
        let visibility = self.parse_optional_visibility()?;
        if !matches!(visibility, Visibility::Private) {
            match self.peek() {
                TokKind::KwType
                | TokKind::KwSession
                | TokKind::KwMemory
                | TokKind::KwTool
                | TokKind::KwPrompt
                | TokKind::KwServer
                | TokKind::KwAgent
                | TokKind::At => {}
                other => {
                    return Err(ParseError {
                        kind: ParseErrorKind::UnexpectedToken {
                            got: describe_token(other),
                            expected:
                                "`type`, `session`, `memory`, `tool`, `prompt`, `agent`, or `@annotation` after `public`".into(),
                        },
                        span: self.peek_span(),
                    });
                }
            }
        }

        match self.peek() {
            TokKind::KwImport => self.parse_import_decl().map(Decl::Import),
            TokKind::KwType => self.parse_type_decl(visibility).map(Decl::Type),
            TokKind::KwSession => self
                .parse_store_decl(StoreKind::Session, visibility)
                .map(Decl::Store),
            TokKind::KwMemory => self
                .parse_store_decl(StoreKind::Memory, visibility)
                .map(Decl::Store),
            TokKind::KwTool => self.parse_tool_decl(visibility).map(Decl::Tool),
            TokKind::KwPrompt => self.parse_prompt_decl(visibility).map(Decl::Prompt),
            TokKind::KwServer => self.parse_server_decl().map(Decl::Server),
            TokKind::KwSchedule => self.parse_schedule_decl().map(Decl::Schedule),
            TokKind::KwEval => self.parse_eval_decl().map(Decl::Eval),
            TokKind::KwTest => self.parse_test_decl().map(Decl::Test),
            TokKind::KwFixture => self.parse_fixture_decl().map(Decl::Fixture),
            TokKind::KwMock => self.parse_mock_decl().map(Decl::Mock),
            TokKind::KwAgent => self.parse_agent_decl(visibility).map(Decl::Agent),
            TokKind::KwPub => self.parse_extern_agent_decl().map(Decl::Agent),
            TokKind::KwExtend => self.parse_extend_decl().map(Decl::Extend),
            TokKind::KwEffect => self.parse_effect_decl().map(Decl::Effect),
            TokKind::KwModel => self.parse_model_decl().map(Decl::Model),
            TokKind::At => {
                let (attributes, constraints) = self.parse_agent_annotations()?;
                let extern_abi = if matches!(self.peek(), TokKind::KwPub) {
                    let abi = self.parse_extern_abi_prefix()?;
                    self.skip_newlines();
                    Some(abi)
                } else {
                    None
                };
                if !matches!(self.peek(), TokKind::KwAgent) {
                    return Err(ParseError {
                        kind: ParseErrorKind::UnexpectedToken {
                            got: describe_token(self.peek()),
                            expected: "`agent` after constraint annotations".into(),
                        },
                        span: self.peek_span(),
                    });
                }
                // `pub extern "c"` agents are implicitly Public
                // regardless of any preceding `public` keyword — FFI
                // export requires external visibility by definition.
                let effective_visibility = if extern_abi.is_some() {
                    Visibility::Public
                } else {
                    visibility
                };
                let mut agent = self.parse_agent_decl(effective_visibility)?;
                agent.extern_abi = extern_abi;
                agent.constraints = constraints;
                agent.attributes = attributes;
                Ok(Decl::Agent(agent))
            }
            other => Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(other),
                    expected: "a top-level declaration (agent, tool, prompt, server, eval, test, fixture, mock, type, session, memory, import, extend, effect, @annotation)".into(),
                },
                span: self.peek_span(),
            }),
        }
    }

    // -- tool ----------------------------------------------------

    pub(super) fn parse_tool_decl(&mut self, visibility: Visibility) -> Result<ToolDecl, ParseError> {
        let start = self.peek_span();
        self.bump(); // tool

        let (name, name_span) = self.expect_ident()?;
        let params = self.parse_params()?;
        self.expect(TokKind::Arrow, "`->` before return type")?;
        let return_ty = self.parse_type_ref()?;
        let return_ownership = self.parse_optional_ownership_annotation()?;

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
            return_ownership,
            effect,
            effect_row,
            visibility,
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

    fn parse_extern_agent_decl(&mut self) -> Result<AgentDecl, ParseError> {
        let extern_abi = self.parse_extern_abi_prefix()?;
        self.skip_newlines();
        if !matches!(self.peek(), TokKind::KwAgent) {
            return Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(self.peek()),
                    expected: "`agent` after `pub extern \"c\"`".into(),
                },
                span: self.peek_span(),
            });
        }
        // `pub extern "c" agent ...` is implicitly Public — FFI
        // export means the agent is by definition visible to
        // external callers.
        let mut agent = self.parse_agent_decl(Visibility::Public)?;
        agent.extern_abi = Some(extern_abi);
        Ok(agent)
    }

    fn parse_extern_abi_prefix(&mut self) -> Result<ExternAbi, ParseError> {
        self.expect(TokKind::KwPub, "`pub` before `extern`")?;
        self.expect(TokKind::KwExtern, "`extern` after `pub`")?;
        let span = self.peek_span();
        let abi = match self.peek().clone() {
            TokKind::StringLit(name) => {
                self.bump();
                name
            }
            other => {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: describe_token(&other),
                        expected: "an ABI string literal like `\"c\"`".into(),
                    },
                    span,
                })
            }
        };
        match abi.to_ascii_lowercase().as_str() {
            "c" => Ok(ExternAbi::C),
            _ => Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: format!("ABI string `{abi}`"),
                    expected: "`\"c\"`".into(),
                },
                span,
            }),
        }
    }

    pub(super) fn parse_agent_decl(&mut self, visibility: Visibility) -> Result<AgentDecl, ParseError> {
        let start = self.peek_span();
        self.bump(); // agent

        let (name, name_span) = self.expect_ident()?;
        let params = self.parse_params()?;
        self.expect(TokKind::Arrow, "`->` before return type")?;
        let return_ty = self.parse_type_ref()?;
        let return_ownership = self.parse_optional_ownership_annotation()?;
        let effect_row = self.parse_uses_clause()?;
        self.expect(TokKind::Colon, "`:` after agent signature")?;
        self.expect_newline()?;

        let body = self.parse_indented_block()?;
        let end = body.span;

        Ok(AgentDecl {
            name: Ident::new(name, name_span),
            extern_abi: None,
            params,
            return_ty,
            return_ownership,
            body,
            effect_row,
            constraints: Vec::new(),
            attributes: Vec::new(),
            visibility,
            span: start.merge(end),
        })
    }

    // -- schedules -----------------------------------------------

    fn parse_schedule_decl(&mut self) -> Result<ScheduleDecl, ParseError> {
        let start = self.peek_span();
        self.expect(TokKind::KwSchedule, "`schedule` declaration")?;
        let cron_span = self.peek_span();
        let cron = match self.peek().clone() {
            TokKind::StringLit(cron) => {
                self.bump();
                cron
            }
            other => {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: describe_token(&other),
                        expected: "a cron expression string literal".into(),
                    },
                    span: cron_span,
                });
            }
        };
        self.expect(TokKind::KwZone, "`zone` after schedule cron expression")?;
        let zone_span = self.peek_span();
        let zone = match self.peek().clone() {
            TokKind::StringLit(zone) => {
                self.bump();
                zone
            }
            other => {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: describe_token(&other),
                        expected: "an IANA time zone string literal".into(),
                    },
                    span: zone_span,
                });
            }
        };
        self.expect(TokKind::Arrow, "`->` before schedule target")?;
        let (target, target_span) = self.expect_ident()?;
        self.expect(TokKind::LParen, "`(` after schedule target")?;
        let mut args = Vec::new();
        if !matches!(self.peek(), TokKind::RParen) {
            args.push(self.parse_expr()?);
            while matches!(self.peek(), TokKind::Comma) {
                self.bump();
                if matches!(self.peek(), TokKind::RParen) {
                    break;
                }
                args.push(self.parse_expr()?);
            }
        }
        self.expect(TokKind::RParen, "`)` after schedule target arguments")?;
        let effect_row = self.parse_uses_clause()?;
        let end = self.peek_span();
        self.expect_newline()?;
        Ok(ScheduleDecl {
            cron,
            zone,
            target: Ident::new(target, target_span),
            args,
            effect_row,
            span: start.merge(end),
        })
    }

    // -- server --------------------------------------------------

    fn parse_server_decl(&mut self) -> Result<ServerDecl, ParseError> {
        let start = self.peek_span();
        self.bump(); // server

        let (name, name_span) = self.expect_ident()?;
        self.expect(TokKind::Colon, "`:` after server name")?;
        self.expect_newline()?;

        if !matches!(self.peek(), TokKind::Indent) {
            return Err(ParseError {
                kind: ParseErrorKind::ExpectedBlock,
                span: self.peek_span(),
            });
        }
        self.bump(); // Indent

        let mut routes = Vec::new();
        while !matches!(self.peek(), TokKind::Dedent | TokKind::Eof) {
            self.skip_newlines();
            if matches!(self.peek(), TokKind::Dedent | TokKind::Eof) {
                break;
            }
            match self.parse_http_route_decl() {
                Ok(route) => routes.push(route),
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

        Ok(ServerDecl {
            name: Ident::new(name, name_span),
            routes,
            span: start.merge(end),
        })
    }

    fn parse_http_route_decl(&mut self) -> Result<HttpRouteDecl, ParseError> {
        let start = self.peek_span();
        self.expect(TokKind::KwRoute, "`route` inside a server block")?;
        let method = self.parse_http_method()?;
        let path_span = self.peek_span();
        let path = match self.peek().clone() {
            TokKind::StringLit(path) => {
                self.bump();
                path
            }
            other => {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: describe_token(&other),
                        expected: "a route path string literal".into(),
                    },
                    span: path_span,
                });
            }
        };

        let path_params = parse_route_path_params(&path, path_span);
        let mut query_ty = None;
        let mut body_ty = None;
        loop {
            if self.peek_ident_is("query") {
                self.bump();
                if query_ty.is_some() {
                    return Err(ParseError {
                        kind: ParseErrorKind::UnexpectedToken {
                            got: "duplicate `query` route clause".into(),
                            expected: "at most one `query Type` clause".into(),
                        },
                        span: self.peek_span(),
                    });
                }
                query_ty = Some(self.parse_type_ref()?);
                continue;
            }
            if self.peek_ident_is("body") {
                self.bump();
                if body_ty.is_some() {
                    return Err(ParseError {
                        kind: ParseErrorKind::UnexpectedToken {
                            got: "duplicate `body` route clause".into(),
                            expected: "at most one `body Type` clause".into(),
                        },
                        span: self.peek_span(),
                    });
                }
                body_ty = Some(self.parse_type_ref()?);
                continue;
            }
            break;
        }

        self.expect(TokKind::Arrow, "`->` before route response")?;
        let response_start = self.expect_contextual_ident("json")?;
        let response_ty = self.parse_type_ref()?;
        let response = RouteResponse {
            kind: RouteResponseKind::Json,
            span: response_start.merge(response_ty.span()),
            ty: response_ty,
        };
        let effect_row = self.parse_uses_clause()?;
        self.expect(TokKind::Colon, "`:` after route signature")?;
        self.expect_newline()?;
        let body = self.parse_indented_block()?;
        let end = body.span;

        Ok(HttpRouteDecl {
            method,
            path,
            path_params,
            query_ty,
            body_ty,
            response,
            effect_row,
            body,
            span: start.merge(end),
        })
    }

    fn parse_http_method(&mut self) -> Result<HttpMethod, ParseError> {
        let span = self.peek_span();
        let (method, method_span) = self.expect_ident()?;
        match method.as_str() {
            "GET" => Ok(HttpMethod::Get),
            "POST" => Ok(HttpMethod::Post),
            "PUT" => Ok(HttpMethod::Put),
            "PATCH" => Ok(HttpMethod::Patch),
            "DELETE" => Ok(HttpMethod::Delete),
            _ => Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: format!("HTTP method `{method}`"),
                    expected: "`GET`, `POST`, `PUT`, `PATCH`, or `DELETE`".into(),
                },
                span: span.merge(method_span),
            }),
        }
    }

    // -- eval ----------------------------------------------------

    fn parse_eval_decl(&mut self) -> Result<EvalDecl, ParseError> {
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

    fn parse_test_decl(&mut self) -> Result<TestDecl, ParseError> {
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

    fn parse_fixture_decl(&mut self) -> Result<FixtureDecl, ParseError> {
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

    fn parse_mock_decl(&mut self) -> Result<MockDecl, ParseError> {
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
                    let d = self.parse_agent_decl(visibility)?;
                    ExtendMethodKind::Agent(d)
                }
                TokKind::KwPrompt => {
                    let d = self.parse_prompt_decl(visibility)?;
                    ExtendMethodKind::Prompt(d)
                }
                TokKind::KwTool => {
                    let d = self.parse_tool_decl(visibility)?;
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
        let ownership = self.parse_optional_ownership_annotation()?;
        let end = ownership.as_ref().map(|o| o.span).unwrap_or_else(|| ty.span());
        Ok(Param {
            name: Ident::new(name, name_span),
            ty,
            ownership,
            span: start.merge(end),
        })
    }

    pub(super) fn parse_optional_ownership_annotation(
        &mut self,
    ) -> Result<Option<OwnershipAnnotation>, ParseError> {
        if !matches!(self.peek(), TokKind::At) {
            return Ok(None);
        }
        let start = self.peek_span();
        self.bump(); // @
        let (mode_name, mode_span) = self.expect_ident()?;
        let mode = match mode_name.as_str() {
            "owned" => OwnershipMode::Owned,
            "borrowed" => OwnershipMode::Borrowed,
            "shared" => OwnershipMode::Shared,
            "static" => OwnershipMode::Static,
            _ => {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: format!("ownership annotation `@{mode_name}`"),
                        expected:
                            "one of `@owned`, `@borrowed`, `@shared`, or `@static`".into(),
                    },
                    span: mode_span,
                });
            }
        };

        let lifetime = if matches!(mode, OwnershipMode::Borrowed)
            && matches!(self.peek(), TokKind::Lt)
        {
            self.bump(); // <
            self.expect(TokKind::Apostrophe, "`'` before borrowed lifetime name")?;
            let (lifetime, _) = self.expect_ident()?;
            self.expect(TokKind::Gt, "`>` after borrowed lifetime")?;
            Some(lifetime)
        } else {
            None
        };
        let end = self.prev_span();
        Ok(Some(OwnershipAnnotation {
            mode,
            lifetime,
            span: start.merge(end),
        }))
    }
}

fn parse_route_path_params(path: &str, path_span: Span) -> Vec<RoutePathParam> {
    let mut params = Vec::new();
    let mut offset = 0usize;
    while let Some(open_rel) = path[offset..].find('{') {
        let open = offset + open_rel;
        let Some(close_rel) = path[open + 1..].find('}') else {
            break;
        };
        let close = open + 1 + close_rel;
        let name = path[open + 1..close].trim();
        if !name.is_empty() && name.chars().all(is_route_param_char) {
            let span = Span::new(path_span.start + open + 1, path_span.start + close);
            params.push(RoutePathParam {
                name: Ident::new(name.to_string(), span),
                ty: TypeRef::Named {
                    name: Ident::new("String", span),
                    span,
                },
                span,
            });
        }
        offset = close + 1;
    }
    params
}

fn is_route_param_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}
