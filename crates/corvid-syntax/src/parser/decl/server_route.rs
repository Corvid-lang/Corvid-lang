//! `schedule`, `server`, and HTTP-route declaration parsing —
//! cron schedules calling agents/tools, server blocks holding
//! `route GET "/path"` definitions, and the per-route grammar.

use crate::errors::{ParseError, ParseErrorKind};
use crate::parser::{describe_token, Parser};
use crate::token::TokKind;
use corvid_ast::{
    HttpMethod, HttpRouteDecl, Ident, RoutePathParam, RouteResponse, RouteResponseKind,
    ScheduleDecl, ServerDecl, Span, TypeRef,
};

impl<'a> Parser<'a> {
    pub(super) fn parse_schedule_decl(&mut self) -> Result<ScheduleDecl, ParseError> {
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

    pub(super) fn parse_server_decl(&mut self) -> Result<ServerDecl, ParseError> {
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
