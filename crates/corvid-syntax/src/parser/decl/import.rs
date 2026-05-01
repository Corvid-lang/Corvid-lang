//! Import declaration parsing — `import python "foo" as bar`,
//! `import "./path" as alias`, package imports (`corvid://...`),
//! and remote imports with `hash:sha256:<digest>` pins.

use crate::errors::{ParseError, ParseErrorKind};
use crate::parser::{describe_token, Parser};
use crate::token::TokKind;
use corvid_ast::{
    EffectRef, EffectRow, Ident, ImportContentHash, ImportDecl, ImportSource, ImportUseItem,
};

impl<'a> Parser<'a> {
    pub(super) fn parse_import_decl(&mut self) -> Result<ImportDecl, ParseError> {
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
                let source = if is_package_corvid_uri(&path) {
                    ImportSource::PackageCorvid
                } else if is_remote_corvid_url(&path) {
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
        if matches!(source, ImportSource::PackageCorvid) && content_hash.is_some() {
            return Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: "`hash` on package import".into(),
                    expected: "package imports get their hash from `Corvid.lock`".into(),
                },
                span: content_hash.as_ref().unwrap().span,
            });
        }
        if !matches!(
            source,
            ImportSource::Corvid | ImportSource::RemoteCorvid | ImportSource::PackageCorvid
        ) {
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

        let effect_row = if self.peek_ident_is("effects") {
            self.parse_import_effects_clause()?
        } else {
            EffectRow::default()
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
            effect_row,
            alias,
            use_items,
            span: start.merge(end),
        })
    }

    fn parse_import_effects_clause(&mut self) -> Result<EffectRow, ParseError> {
        let start = self.peek_span();
        self.bump(); // effects
        self.expect(TokKind::Colon, "`:` after import effects")?;
        let (first_name, first_span) = self.expect_ident()?;
        let mut effects = vec![EffectRef {
            name: Ident::new(first_name, first_span),
            span: first_span,
        }];
        while matches!(self.peek(), TokKind::Comma) {
            self.bump();
            let (name, span) = self.expect_ident()?;
            effects.push(EffectRef {
                name: Ident::new(name, span),
                span,
            });
        }
        let end = effects.last().map(|effect| effect.span).unwrap_or(start);
        Ok(EffectRow {
            effects,
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
}

fn is_remote_corvid_url(path: &str) -> bool {
    path.starts_with("https://") || path.starts_with("http://")
}

fn is_package_corvid_uri(path: &str) -> bool {
    path.starts_with("corvid://")
}
