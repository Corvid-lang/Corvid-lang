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
    ExternAbi, OwnershipAnnotation, OwnershipMode,
    EvalAssert, EvalDecl, ExtendDecl, ExtendMethod, ExtendMethodKind, Field, Ident, ImportDecl,
    ImportContentHash, ImportSource, ImportUseItem, ModelDecl, ModelField, Param, ToolDecl,
    TypeDecl, Visibility,
};

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
                | TokKind::KwTool
                | TokKind::KwPrompt
                | TokKind::KwAgent
                | TokKind::At => {}
                other => {
                    return Err(ParseError {
                        kind: ParseErrorKind::UnexpectedToken {
                            got: describe_token(other),
                            expected:
                                "`type`, `tool`, `prompt`, `agent`, or `@annotation` after `public`".into(),
                        },
                        span: self.peek_span(),
                    });
                }
            }
        }

        match self.peek() {
            TokKind::KwImport => self.parse_import_decl().map(Decl::Import),
            TokKind::KwType => self.parse_type_decl(visibility).map(Decl::Type),
            TokKind::KwTool => self.parse_tool_decl(visibility).map(Decl::Tool),
            TokKind::KwPrompt => self.parse_prompt_decl(visibility).map(Decl::Prompt),
            TokKind::KwEval => self.parse_eval_decl().map(Decl::Eval),
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

        // Two shapes are accepted:
        //
        //   (1) External ecosystem: `import python "foo" as bar`
        //       — the next token is an identifier naming the source.
        //
        //   (2) Local Corvid file: `import "./path" as alias`
        //       — the next token is a string literal. The extension
        //       is implicit (`.cor`); the resolver handles path
        //       resolution.
        //
        // The first token after `import` disambiguates.
        let (source, module) = match self.peek().clone() {
            TokKind::StringLit(path) => {
                self.bump();
                let source = if is_remote_corvid_url(&path) {
                    ImportSource::RemoteCorvid
                } else {
                    ImportSource::Corvid
                };
                (source, path)
            }
            TokKind::Ident(_) => {
                let (source_name, source_span) = self.expect_ident()?;
                let source = match source_name.as_str() {
                    "python" => ImportSource::Python,
                    _ => {
                        return Err(ParseError {
                            kind: ParseErrorKind::UnexpectedToken {
                                got: format!("identifier `{source_name}`"),
                                expected:
                                    "an import source (`python`) or a Corvid path string"
                                        .into(),
                            },
                            span: source_span,
                        });
                    }
                };
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
                (source, module)
            }
            other => {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: describe_token(&other),
                        expected:
                            "an import source (`python`) or a Corvid path string after `import`"
                                .into(),
                    },
                    span: self.peek_span(),
                });
            }
        };

        let mut content_hash = None;
        let mut required_attributes = Vec::new();
        let mut required_constraints = Vec::new();
        loop {
            if matches!(self.peek(), TokKind::KwRequires) {
                self.bump();
                let (attributes, constraints) = self.parse_inline_agent_annotations()?;
                required_attributes.extend(attributes);
                required_constraints.extend(constraints);
                continue;
            }
            if matches!(self.peek(), TokKind::Ident(word) if word == "hash") {
                if content_hash.is_some() {
                    return Err(ParseError {
                        kind: ParseErrorKind::UnexpectedToken {
                            got: "duplicate `hash` import pin".into(),
                            expected: "at most one `hash:sha256:<digest>` clause".into(),
                        },
                        span: self.peek_span(),
                    });
                }
                content_hash = Some(self.parse_import_content_hash()?);
                continue;
            }
            break;
        }
        if matches!(source, ImportSource::RemoteCorvid) && content_hash.is_none() {
            return Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: "remote Corvid import without `hash`".into(),
                    expected: "remote imports must declare `hash:sha256:<digest>`".into(),
                },
                span: start,
            });
        }
        if !matches!(source, ImportSource::Corvid | ImportSource::RemoteCorvid) {
            if let Some(hash) = &content_hash {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: "`hash` on non-Corvid import".into(),
                        expected: "hash pins only on local Corvid path imports".into(),
                    },
                    span: hash.span,
                });
            }
        }

        // Optional `as IDENT`. Note: Corvid imports (`import "./path"`)
        // strongly expect an alias for the v1 resolver's qualified-
        // access story, but the grammar accepts no-alias for
        // consistency with external imports. The resolver will
        // enforce alias-required once `lang-cor-imports-basic-resolve`
        // lands.
        let alias = if matches!(self.peek(), TokKind::KwAs) {
            self.bump();
            let (name, span) = self.expect_ident()?;
            Some(Ident::new(name, span))
        } else {
            None
        };

        let use_items = if matches!(self.peek(), TokKind::Ident(word) if word == "use") {
            self.bump();
            self.parse_import_use_items()?
        } else {
            Vec::new()
        };

        let end = self.peek_span();
        self.expect_newline()?;
        Ok(ImportDecl {
            source,
            module,
            content_hash,
            required_attributes,
            required_constraints,
            alias,
            use_items,
            span: start.merge(end),
        })
    }

    fn parse_import_content_hash(&mut self) -> Result<ImportContentHash, ParseError> {
        let start = self.peek_span();
        self.bump(); // hash
        self.expect(TokKind::Colon, "`:` after import hash")?;
        let (algorithm, algorithm_span) = self.expect_ident()?;
        if algorithm != "sha256" {
            return Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: format!("hash algorithm `{algorithm}`"),
                    expected: "`sha256`".into(),
                },
                span: algorithm_span,
            });
        }
        self.expect(TokKind::Colon, "`:` after import hash algorithm")?;
        let digest_span = self.peek_span();
        let digest = match self.peek().clone() {
            TokKind::Ident(value) | TokKind::StringLit(value) => {
                self.bump();
                value
            }
            other => {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: describe_token(&other),
                        expected: "a 64-character SHA-256 hex digest".into(),
                    },
                    span: digest_span,
                });
            }
        };
        let digest = digest.to_ascii_lowercase();
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: format!("hash digest `{digest}`"),
                    expected: "a 64-character SHA-256 hex digest".into(),
                },
                span: digest_span,
            });
        }
        Ok(ImportContentHash {
            algorithm,
            hex: digest,
            span: start.merge(digest_span),
        })
    }

    fn parse_import_use_items(&mut self) -> Result<Vec<ImportUseItem>, ParseError> {
        let mut items = Vec::new();
        loop {
            let (name, name_span) = self.expect_ident()?;
            let name_ident = Ident::new(name, name_span);
            let alias = if matches!(self.peek(), TokKind::KwAs) {
                self.bump();
                let (alias, alias_span) = self.expect_ident()?;
                Some(Ident::new(alias, alias_span))
            } else {
                None
            };
            let span = alias
                .as_ref()
                .map(|alias| name_span.merge(alias.span))
                .unwrap_or(name_span);
            items.push(ImportUseItem {
                name: name_ident,
                alias,
                span,
            });
            if !matches!(self.peek(), TokKind::Comma) {
                break;
            }
            self.bump();
        }
        Ok(items)
    }

    // -- type ----------------------------------------------------

    fn parse_type_decl(&mut self, visibility: Visibility) -> Result<TypeDecl, ParseError> {
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
            visibility,
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

fn is_remote_corvid_url(path: &str) -> bool {
    path.starts_with("https://") || path.starts_with("http://")
}
