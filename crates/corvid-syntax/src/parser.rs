//! Parser for Corvid expressions.
//!
//! Parser implementation for expressions, statements, and declarations.
//!
//! Technique: recursive descent for structural rules + Pratt-style
//! precedence climbing for binary operators.

use crate::errors::{ParseError, ParseErrorKind};
use crate::token::{TokKind, Token};
use corvid_ast::{
    AdversarialSpec, AgentDecl, Backoff, BackpressurePolicy, BinaryOp, Block, Decl,
    DimensionDecl, DimensionValue, Effect, EffectConstraint, EffectDecl, EffectRef, EffectRow,
    EnsembleSpec, EvalAssert, EvalDecl, Expr, ExtendDecl, ExtendMethod, ExtendMethodKind, Field,
    File, Ident, ImportDecl, ImportSource, Literal, ModelDecl, ModelField, Param,
    ProgressiveChain, ProgressiveStage, PromptDecl, PromptStreamSettings, RolloutSpec, RouteArm,
    RoutePattern, RouteTable, Span, Stmt, ToolDecl, TypeDecl, TypeRef, UnaryOp, Visibility,
    VoteStrategy, WeakEffect, WeakEffectRow,
};

#[derive(Debug, Clone, PartialEq)]
pub enum ReplItem {
    Decl(Decl),
    Stmt(Stmt),
    Expr(Expr),
}

/// Parse a full expression from a token stream.
///
/// The token stream is typically produced by [`crate::lex`]. Structural
/// tokens (`Newline`, `Indent`, `Dedent`, `Eof`) terminate the expression
/// — the parser stops before them and leaves them for the caller.
pub fn parse_expr(tokens: &[Token]) -> Result<Expr, ParseError> {
    let mut p = Parser::new(tokens);
    let expr = p.parse_expr()?;
    Ok(expr)
}

/// Parse a full `.cor` file into a `File` AST.
///
/// Errors are collected — a broken declaration reports an error and the
/// parser recovers to the next top-level keyword before continuing.
pub fn parse_file(tokens: &[Token]) -> (File, Vec<ParseError>) {
    let mut p = Parser::new(tokens);
    let file = p.parse_file_inner();
    (file, p.errors)
}

/// Parse a single REPL turn as either a declaration, a statement, or
/// an expression. Classification uses first-token lookahead so errors
/// stay local and predictable.
pub fn parse_repl_input(tokens: &[Token]) -> Result<ReplItem, Vec<ParseError>> {
    let mut p = Parser::new(tokens);
    p.skip_newlines();
    let item = match p.parse_repl_item() {
        Ok(item) => item,
        Err(err) => return Err(vec![err]),
    };
    p.skip_newlines();
    if !matches!(p.peek(), TokKind::Eof) {
        p.errors.push(ParseError {
            kind: ParseErrorKind::UnexpectedToken {
                got: describe_token(p.peek()),
                expected: "end of input".into(),
            },
            span: p.peek_span(),
        });
    }
    if p.errors.is_empty() {
        Ok(item)
    } else {
        Err(p.errors)
    }
}

/// Parse a top-level block (expects `Indent`, statements, `Dedent`).
///
/// Returns a `Block` plus any errors encountered. Errors are collected:
/// parsing does not stop at the first problem.
pub fn parse_block(tokens: &[Token]) -> (Block, Vec<ParseError>) {
    let mut p = Parser::new(tokens);
    let mut errors = Vec::new();
    match p.parse_indented_block() {
        Ok(block) => {
            errors.extend(p.errors);
            (block, errors)
        }
        Err(e) => {
            errors.push(e);
            errors.extend(p.errors);
            (
                Block {
                    stmts: Vec::new(),
                    span: Span::new(0, 0),
                },
                errors,
            )
        }
    }
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    /// Errors collected during statement/block parsing. Expression-level
    /// errors are fatal and returned via `Result`.
    errors: Vec<ParseError>,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self {
            tokens,
            pos: 0,
            errors: Vec::new(),
        }
    }

    /// Peek at the next token without consuming.
    fn peek(&self) -> &TokKind {
        self.tokens
            .get(self.pos)
            .map(|t| &t.kind)
            .unwrap_or(&TokKind::Eof)
    }

    /// Current token's span (or a zero-width span at EOF).
    fn peek_span(&self) -> Span {
        self.tokens
            .get(self.pos)
            .map(|t| t.span)
            .unwrap_or(Span::new(0, 0))
    }

    fn prev_span(&self) -> Span {
        self.pos
            .checked_sub(1)
            .and_then(|idx| self.tokens.get(idx))
            .map(|t| t.span)
            .unwrap_or(Span::new(0, 0))
    }

    /// Consume the next token.
    fn bump(&mut self) -> &'a Token {
        let tok = &self.tokens[self.pos];
        self.pos += 1;
        tok
    }

    fn at_end(&self) -> bool {
        matches!(
            self.peek(),
            TokKind::Eof | TokKind::Newline | TokKind::Indent | TokKind::Dedent
        )
    }


    fn expect_ident(&mut self) -> Result<(String, Span), ParseError> {
        let span = self.peek_span();
        match self.peek().clone() {
            TokKind::Ident(name) => {
                self.bump();
                Ok((name, span))
            }
            other => Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(&other),
                    expected: "an identifier".into(),
                },
                span,
            }),
        }
    }

    fn expect(&mut self, kind: TokKind, description: &str) -> Result<Span, ParseError> {
        let span = self.peek_span();
        if std::mem::discriminant(self.peek()) == std::mem::discriminant(&kind) {
            self.bump();
            Ok(span)
        } else {
            Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(self.peek()),
                    expected: description.into(),
                },
                span,
            })
        }
    }

    /// Skip tokens until we hit the next `Newline`, `Dedent`, or `Eof`.
    /// Used for error recovery — consume the broken statement so parsing
    /// can continue with the next line.
    fn sync_to_statement_boundary(&mut self) {
        while !matches!(
            self.peek(),
            TokKind::Newline | TokKind::Dedent | TokKind::Eof
        ) {
            self.bump();
        }
        // Consume the newline itself if present, so the next statement starts clean.
        if matches!(self.peek(), TokKind::Newline) {
            self.bump();
        }
    }


    fn expect_newline(&mut self) -> Result<(), ParseError> {
        match self.peek() {
            TokKind::Newline => {
                self.bump();
                Ok(())
            }
            TokKind::Eof | TokKind::Dedent => Ok(()),
            other => Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(other),
                    expected: "end of line".into(),
                },
                span: self.peek_span(),
            }),
        }
    }

    // ------------------------------------------------------------
    // File and declaration parsing.
    // ------------------------------------------------------------

    fn parse_file_inner(&mut self) -> File {
        let start_span = self.peek_span();
        let mut decls = Vec::new();

        // Skip any leading newlines (files may start with a blank line).
        self.skip_newlines();

        while !matches!(self.peek(), TokKind::Eof) {
            match self.parse_decl() {
                Ok(d) => decls.push(d),
                Err(e) => {
                    self.errors.push(e);
                    self.sync_to_next_decl();
                }
            }
            self.skip_newlines();
        }

        let end_span = self.peek_span();
        File {
            decls,
            span: start_span.merge(end_span),
        }
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek(), TokKind::Newline) {
            self.bump();
        }
    }

    fn parse_repl_item(&mut self) -> Result<ReplItem, ParseError> {
        if matches!(
            self.peek(),
            TokKind::KwImport
                | TokKind::KwType
                | TokKind::KwTool
                | TokKind::KwPrompt
                | TokKind::KwEval
                | TokKind::KwAgent
                | TokKind::KwExtend
                | TokKind::KwEffect
                | TokKind::KwModel
                | TokKind::At
        ) {
            return self.parse_decl().map(ReplItem::Decl);
        }

        if self.starts_stmt() {
            return self.parse_stmt().map(ReplItem::Stmt);
        }

        let expr = self.parse_expr()?;
        self.expect_newline()?;
        Ok(ReplItem::Expr(expr))
    }

    fn starts_stmt(&self) -> bool {
        match self.peek() {
            TokKind::KwReturn
            | TokKind::KwYield
            | TokKind::KwIf
            | TokKind::KwFor
            | TokKind::KwApprove
            | TokKind::KwBreak
            | TokKind::KwContinue
            | TokKind::KwPass => true,
            TokKind::Ident(_) => matches!(
                self.tokens.get(self.pos + 1).map(|t| &t.kind),
                Some(TokKind::Assign)
            ),
            _ => false,
        }
    }

    /// Skip tokens until we reach a token that could start a declaration
    /// (or EOF). Used after a parse error at the top level.
    fn sync_to_next_decl(&mut self) {
        loop {
            match self.peek() {
                TokKind::KwImport
                | TokKind::KwType
                | TokKind::KwTool
                | TokKind::KwPrompt
                | TokKind::KwEval
                | TokKind::KwAgent
                | TokKind::KwEffect
                | TokKind::KwModel
                | TokKind::At
                | TokKind::KwExtend
                | TokKind::Eof => return,
                _ => {
                    self.bump();
                }
            }
        }
    }


    fn peek_ident_is(&self, expected: &str) -> bool {
        matches!(self.peek(), TokKind::Ident(name) if name == expected)
    }

    fn expect_contextual_ident(&mut self, expected: &str) -> Result<Span, ParseError> {
        let span = self.peek_span();
        match self.peek() {
            TokKind::Ident(name) if name == expected => {
                self.bump();
                Ok(span)
            }
            other => Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(other),
                    expected: format!("`{expected}`"),
                },
                span,
            }),
        }
    }

    fn parse_confidence_literal(&mut self) -> Result<f64, ParseError> {
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

    fn expect_positive_int_literal(
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

    fn parse_cost_literal(&mut self) -> Result<f64, ParseError> {
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

    fn parse_uses_clause(&mut self) -> Result<EffectRow, ParseError> {
        if !matches!(self.peek(), TokKind::KwUses) {
            return Ok(EffectRow::default());
        }
        let start = self.peek_span();
        self.bump(); // uses
        let mut effects = Vec::new();
        let (first_name, first_span) = self.expect_ident()?;
        effects.push(EffectRef {
            name: Ident::new(first_name, first_span),
            span: first_span,
        });
        while matches!(self.peek(), TokKind::Comma) {
            self.bump();
            let (name, span) = self.expect_ident()?;
            effects.push(EffectRef {
                name: Ident::new(name, span),
                span,
            });
        }
        let end = effects.last().map(|e| e.span).unwrap_or(start);
        Ok(EffectRow {
            effects,
            span: start.merge(end),
        })
    }

    fn parse_prompt_stream_settings(&mut self) -> Result<PromptStreamSettings, ParseError> {
        let mut settings = PromptStreamSettings::default();
        while self.peek_ident_is("with") {
            self.bump(); // with
            let (name, span) = self.expect_ident()?;
            match name.as_str() {
                "min_confidence" => {
                    settings.min_confidence = Some(self.parse_confidence_literal()?);
                }
                "max_tokens" => {
                    let (max_tokens, _) =
                        self.expect_positive_int_literal("a positive max token count")?;
                    settings.max_tokens = Some(max_tokens);
                }
                "backpressure" => {
                    settings.backpressure = Some(self.parse_backpressure_policy()?);
                }
                _ => {
                    return Err(ParseError {
                        kind: ParseErrorKind::UnexpectedToken {
                            got: format!("identifier `{name}`"),
                            expected: "`min_confidence`, `max_tokens`, or `backpressure`".into(),
                        },
                        span,
                    });
                }
            }
            self.expect_newline()?;
        }
        Ok(settings)
    }

    fn parse_backpressure_policy(&mut self) -> Result<BackpressurePolicy, ParseError> {
        let span = self.peek_span();
        match self.peek().clone() {
            TokKind::Ident(name) if name == "unbounded" => {
                self.bump();
                Ok(BackpressurePolicy::Unbounded)
            }
            TokKind::Ident(name) if name == "bounded" => {
                self.bump();
                self.expect(TokKind::LParen, "`(` after `bounded`")?;
                let (capacity, _) =
                    self.expect_positive_int_literal("a positive bounded backpressure size")?;
                self.expect(TokKind::RParen, "`)` after bounded backpressure size")?;
                Ok(BackpressurePolicy::Bounded(capacity))
            }
            other => Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(&other),
                    expected: "`bounded(N)` or `unbounded`".into(),
                },
                span,
            }),
        }
    }

    fn parse_constraints(&mut self) -> Result<Vec<EffectConstraint>, ParseError> {
        let mut constraints = Vec::new();
        while matches!(self.peek(), TokKind::At) {
            let start = self.peek_span();
            self.bump(); // @
            let (name, name_span) = self.expect_ident()?;
            if name == "budget" && matches!(self.peek(), TokKind::LParen) {
                constraints.extend(self.parse_budget_constraints(start, name_span)?);
                self.expect_newline()?;
                continue;
            }
            let value = if matches!(self.peek(), TokKind::LParen) {
                self.bump();
                if matches!(self.peek(), TokKind::RParen) {
                    self.bump();
                    None
                } else {
                    let value = self.parse_dimension_value()?;
                    self.expect(TokKind::RParen, "`)` after constraint value")?;
                    Some(value)
                }
            } else {
                None
            };
            let end = self.prev_span();
            self.expect_newline()?;
            constraints.push(EffectConstraint {
                dimension: Ident::new(name, name_span),
                value,
                span: start.merge(end),
            });
        }
        Ok(constraints)
    }

    fn parse_budget_constraints(
        &mut self,
        start: Span,
        name_span: Span,
    ) -> Result<Vec<EffectConstraint>, ParseError> {
        self.expect(TokKind::LParen, "`(` after `@budget`")?;
        let mut constraints = Vec::new();

        if !matches!(self.peek(), TokKind::RParen) {
            loop {
                if matches!(self.peek(), TokKind::Dollar) {
                    let value = self.parse_dimension_value()?;
                    constraints.push(EffectConstraint {
                        dimension: Ident::new("cost", name_span),
                        value: Some(value),
                        span: start.merge(self.prev_span()),
                    });
                } else {
                    let (dim_name, dim_span) = self.expect_ident()?;
                    self.expect(TokKind::Colon, "`:` after budget dimension name")?;
                    let value = self.parse_dimension_value()?;
                    let canonical_name = match dim_name.as_str() {
                        "latency" => "latency_ms".to_string(),
                        other => other.to_string(),
                    };
                    constraints.push(EffectConstraint {
                        dimension: Ident::new(canonical_name, dim_span),
                        value: Some(value),
                        span: start.merge(self.prev_span()),
                    });
                }

                if !matches!(self.peek(), TokKind::Comma) {
                    break;
                }
                self.bump();
            }
        }

        self.expect(TokKind::RParen, "`)` after budget constraints")?;
        Ok(constraints)
    }

    fn consume_optional_duration_suffix(&mut self, value: f64) -> f64 {
        match self.peek() {
            TokKind::Ident(unit) if unit == "ms" => {
                self.bump();
                value
            }
            TokKind::Ident(unit) if unit == "s" => {
                self.bump();
                value * 1000.0
            }
            _ => value,
        }
    }

    // -- extend methods ------------------------------

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
    fn parse_params(&mut self) -> Result<Vec<Param>, ParseError> {
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

fn describe_token(t: &TokKind) -> String {
    match t {
        TokKind::Ident(s) => format!("identifier `{s}`"),
        TokKind::Int(n) => format!("integer `{n}`"),
        TokKind::Float(f) => format!("float `{f}`"),
        TokKind::StringLit(s) => format!("string `\"{s}\"`"),
        TokKind::Eof => "end of input".into(),
        other => format!("{other:?}"),
    }
}


mod decl;
mod expr;
mod prompt;
mod stmt;
mod types;

#[cfg(test)]
mod tests;
