//! Prompt declaration parsing — `parse_prompt_decl` plus the five
//! mutually-exclusive dispatch clauses.
//!
//!   route:        content-aware pattern dispatch
//!   progressive:  confidence-threshold refinement chain
//!   rollout:      A/B variant split with a percentage
//!   ensemble:     concurrent dispatch + vote
//!   adversarial:  three-stage propose / challenge / adjudicate pipeline
//!
//! Extracted from `parser.rs` as part of Phase 20i responsibility
//! decomposition.

use super::{describe_token, Parser};
use crate::errors::{ParseError, ParseErrorKind};
use crate::token::TokKind;
use corvid_ast::{
    AdversarialSpec, EnsembleSpec, Ident, ProgressiveChain, ProgressiveStage, PromptDecl,
    RolloutSpec, RouteArm, RoutePattern, RouteTable, Visibility, VoteStrategy,
};

impl<'a> Parser<'a> {
    pub(super) fn parse_prompt_decl(&mut self, visibility: Visibility) -> Result<PromptDecl, ParseError> {
        let start = self.peek_span();
        self.bump(); // prompt

        let (name, name_span) = self.expect_ident()?;
        let params = self.parse_params()?;
        self.expect(TokKind::Arrow, "`->` before return type")?;
        let return_ty = self.parse_type_ref()?;
        let return_ownership = self.parse_optional_ownership_annotation()?;
        let effect_row = self.parse_uses_clause()?;
        self.expect(TokKind::Colon, "`:` after prompt signature")?;
        self.expect_newline()?;

        if !matches!(self.peek(), TokKind::Indent) {
            return Err(ParseError {
                kind: ParseErrorKind::ExpectedBlock,
                span: self.peek_span(),
            });
        }
        self.bump(); // Indent

        // Phase 20h: optional `requires: <capability>` clause.
        // Must appear before `with ...` stream settings and the
        // template, so the body order is: requires → route → with → template.
        let capability_required = if matches!(self.peek(), TokKind::KwRequires) {
            self.bump(); // requires
            self.expect(TokKind::Colon, "`:` after `requires`")?;
            let (ident, ident_span) = self.expect_ident()?;
            self.expect_newline()?;
            Some(Ident::new(ident, ident_span))
        } else {
            None
        };

        let cites_strictly = self.parse_prompt_cites_strictly_clause()?;

        // Phase 20h slice C: optional `route:` block. Each arm is
        // `<guard-expr> -> <model-ident>` or `_ -> <model-ident>`.
        let route = if matches!(self.peek(), TokKind::KwRoute) {
            Some(self.parse_prompt_route_block()?)
        } else {
            None
        };

        // Phase 20h slice E: optional `progressive:` block. Mutually
        // exclusive with `route:`; the parser reports a dedicated
        // error if both appear on the same prompt.
        let progressive = if matches!(self.peek(), TokKind::KwProgressive) {
            if route.is_some() {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: "`progressive:` after `route:`".into(),
                        expected: "a prompt template string (a prompt uses either `route:` or `progressive:`, not both)".into(),
                    },
                    span: self.peek_span(),
                });
            }
            Some(self.parse_prompt_progressive_block()?)
        } else {
            None
        };

        // Phase 20h slice I: optional `rollout ...` one-liner.
        // Mutually exclusive with both `route:` and `progressive:`.
        let rollout = if matches!(self.peek(), TokKind::KwRollout) {
            if route.is_some() || progressive.is_some() {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: "`rollout` after another dispatch clause".into(),
                        expected: "a prompt template string (a prompt uses exactly one of `route:`, `progressive:`, `rollout`, or `ensemble`)".into(),
                    },
                    span: self.peek_span(),
                });
            }
            Some(self.parse_prompt_rollout_clause()?)
        } else {
            None
        };

        // Phase 20h slice F: optional `ensemble [...] vote <strategy>`.
        // Mutually exclusive with route / progressive / rollout.
        let ensemble = if matches!(self.peek(), TokKind::KwEnsemble) {
            if route.is_some() || progressive.is_some() || rollout.is_some() {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: "`ensemble` after another dispatch clause".into(),
                        expected: "a prompt template string (a prompt uses exactly one of `route:`, `progressive:`, `rollout`, `ensemble`, or `adversarial:`)".into(),
                    },
                    span: self.peek_span(),
                });
            }
            Some(self.parse_prompt_ensemble_clause()?)
        } else {
            None
        };

        // Phase 20h slice G: optional `adversarial:` block.
        // Mutually exclusive with every other dispatch clause.
        let adversarial = if matches!(self.peek(), TokKind::KwAdversarial) {
            if route.is_some()
                || progressive.is_some()
                || rollout.is_some()
                || ensemble.is_some()
            {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: "`adversarial:` after another dispatch clause".into(),
                        expected: "a prompt template string (a prompt uses exactly one of `route:`, `progressive:`, `rollout`, `ensemble`, or `adversarial:`)".into(),
                    },
                    span: self.peek_span(),
                });
            }
            Some(self.parse_prompt_adversarial_clause()?)
        } else {
            None
        };

        let (calibrated, cacheable) = self.parse_prompt_flags()?;

        let stream = self.parse_prompt_stream_settings()?;

        // Expect a single string literal as the template.
        let template = match self.peek().clone() {
            TokKind::StringLit(s) => {
                self.bump();
                s
            }
            other => {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: describe_token(&other),
                        expected: "a prompt template string".into(),
                    },
                    span: self.peek_span(),
                });
            }
        };
        self.expect_newline()?;

        let end = self.peek_span();
        if matches!(self.peek(), TokKind::Dedent) {
            self.bump();
        }

        Ok(PromptDecl {
            name: Ident::new(name, name_span),
            params,
            return_ty,
            return_ownership,
            template,
            effect_row,
            cites_strictly,
            stream,
            calibrated,
            cacheable,
            capability_required,
            route,
            progressive,
            rollout,
            ensemble,
            adversarial,
            visibility,
            span: start.merge(end),
        })
    }

    fn parse_prompt_flags(&mut self) -> Result<(bool, bool), ParseError> {
        let mut calibrated = false;
        let mut cacheable = false;
        loop {
            if self.peek_ident_is("calibrated") {
                self.bump();
                self.expect_newline()?;
                calibrated = true;
                continue;
            }
            if self.peek_ident_is("cacheable") {
                let start = self.peek_span();
                self.bump(); // cacheable
                self.expect(TokKind::Colon, "`:` after `cacheable`")?;
                cacheable = match self.peek().clone() {
                    TokKind::KwTrue => {
                        self.bump();
                        true
                    }
                    TokKind::KwFalse => {
                        self.bump();
                        false
                    }
                    other => {
                        return Err(ParseError {
                            kind: ParseErrorKind::UnexpectedToken {
                                got: describe_token(&other),
                                expected: "`true` or `false` after `cacheable:`".into(),
                            },
                            span: start.merge(self.peek_span()),
                        });
                    }
                };
                self.expect_newline()?;
                continue;
            }
            return Ok((calibrated, cacheable));
        }
    }

    fn parse_prompt_cites_strictly_clause(
        &mut self,
    ) -> Result<Option<String>, ParseError> {
        if !matches!(self.peek(), TokKind::Ident(name) if name == "cites") {
            return Ok(None);
        }

        self.bump(); // cites
        let (param, _) = self.expect_ident()?;
        let (strictly, strictly_span) = self.expect_ident()?;
        if strictly != "strictly" {
            return Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: format!("identifier `{strictly}`"),
                    expected: "`strictly` after `cites <param>`".into(),
                },
                span: strictly_span,
            });
        }
        self.expect_newline()?;
        Ok(Some(param))
    }

    /// Parse:
    ///
    /// ```text
    /// adversarial:
    ///     propose: <model>
    ///     challenge: <model>
    ///     adjudicate: <model>
    /// ```
    ///
    /// Every stage is required. Caller has already positioned at
    /// the `adversarial` keyword.
    fn parse_prompt_adversarial_clause(
        &mut self,
    ) -> Result<AdversarialSpec, ParseError> {
        let start = self.peek_span();
        self.bump(); // adversarial
        self.expect(TokKind::Colon, "`:` after `adversarial`")?;
        self.expect_newline()?;

        if !matches!(self.peek(), TokKind::Indent) {
            return Err(ParseError {
                kind: ParseErrorKind::ExpectedBlock,
                span: self.peek_span(),
            });
        }
        self.bump(); // Indent

        self.skip_newlines();
        let proposer = self.parse_adversarial_stage(TokKind::KwPropose, "propose")?;
        self.skip_newlines();
        let challenger =
            self.parse_adversarial_stage(TokKind::KwChallenge, "challenge")?;
        self.skip_newlines();
        let adjudicator =
            self.parse_adversarial_stage(TokKind::KwAdjudicate, "adjudicate")?;
        self.skip_newlines();

        let end = self.peek_span();
        if matches!(self.peek(), TokKind::Dedent) {
            self.bump();
        }

        Ok(AdversarialSpec {
            proposer,
            challenger,
            adjudicator,
            span: start.merge(end),
        })
    }

    fn parse_adversarial_stage(
        &mut self,
        expected_kw: TokKind,
        label: &str,
    ) -> Result<Ident, ParseError> {
        if !matches!(self.peek(), k if k == &expected_kw) {
            return Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: describe_token(self.peek()),
                    expected: format!("`{label}:` stage in adversarial block"),
                },
                span: self.peek_span(),
            });
        }
        self.bump(); // propose / challenge / adjudicate
        self.expect(TokKind::Colon, "`:` after adversarial stage")?;
        let (name, span) = self.expect_ident()?;
        self.expect_newline()?;
        Ok(Ident::new(name, span))
    }

    /// Parse `ensemble [<m1>, <m2>, <m3>] vote <strategy>`. Caller has
    /// already positioned at the `ensemble` keyword.
    fn parse_prompt_ensemble_clause(&mut self) -> Result<EnsembleSpec, ParseError> {
        let start = self.peek_span();
        self.bump(); // ensemble

        self.expect(TokKind::LBracket, "`[` after `ensemble`")?;

        let mut models = Vec::new();
        loop {
            if matches!(self.peek(), TokKind::RBracket) {
                break;
            }
            let (name, span) = self.expect_ident()?;
            models.push(Ident::new(name, span));
            match self.peek() {
                TokKind::Comma => {
                    self.bump();
                }
                TokKind::RBracket => break,
                other => {
                    return Err(ParseError {
                        kind: ParseErrorKind::UnexpectedToken {
                            got: describe_token(other),
                            expected: "`,` or `]` after ensemble model".into(),
                        },
                        span: self.peek_span(),
                    });
                }
            }
        }
        self.expect(TokKind::RBracket, "`]` after ensemble models")?;

        if models.len() < 2 {
            return Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: format!("{} ensemble model(s)", models.len()),
                    expected: "at least two models in the ensemble — voting is undefined with fewer".into(),
                },
                span: start,
            });
        }

        self.expect(TokKind::KwVote, "`vote` after ensemble model list")?;

        // Strategy ident. Currently only `majority` is supported.
        let vote = match self.peek().clone() {
            TokKind::Ident(name) if name == "majority" => {
                self.bump();
                VoteStrategy::Majority
            }
            other => {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: describe_token(&other),
                        expected: "a vote strategy (`majority`)".into(),
                    },
                    span: self.peek_span(),
                });
            }
        };

        let end = self.prev_span();
        self.expect_newline()?;

        Ok(EnsembleSpec {
            models,
            vote,
            span: start.merge(end),
        })
    }

    /// Parse `rollout N% <variant>, else <baseline>`. Caller has
    /// already positioned at the `rollout` keyword.
    fn parse_prompt_rollout_clause(&mut self) -> Result<RolloutSpec, ParseError> {
        let start = self.peek_span();
        self.bump(); // rollout

        // Percentage — accept Int or Float, mandatory `%`.
        let variant_percent = match self.peek().clone() {
            TokKind::Float(n) => {
                self.bump();
                n
            }
            TokKind::Int(n) => {
                self.bump();
                n as f64
            }
            other => {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: describe_token(&other),
                        expected: "a percentage after `rollout`".into(),
                    },
                    span: self.peek_span(),
                });
            }
        };
        self.expect(TokKind::Percent, "`%` after rollout percentage")?;

        let (variant_name, variant_span) = self.expect_ident()?;
        self.expect(TokKind::Comma, "`,` after rollout variant")?;
        self.expect(TokKind::KwElse, "`else` before rollout baseline")?;
        let (baseline_name, baseline_span) = self.expect_ident()?;
        let end = self.prev_span();
        self.expect_newline()?;

        Ok(RolloutSpec {
            variant_percent,
            variant: Ident::new(variant_name, variant_span),
            baseline: Ident::new(baseline_name, baseline_span),
            span: start.merge(end),
        })
    }

    /// Parse a `route:` block inside a prompt body. Caller has
    /// already positioned at the `route` keyword.
    fn parse_prompt_route_block(&mut self) -> Result<RouteTable, ParseError> {
        let start = self.peek_span();
        self.bump(); // route
        self.expect(TokKind::Colon, "`:` after `route`")?;
        self.expect_newline()?;

        if !matches!(self.peek(), TokKind::Indent) {
            return Err(ParseError {
                kind: ParseErrorKind::ExpectedBlock,
                span: self.peek_span(),
            });
        }
        self.bump(); // Indent

        let mut arms = Vec::new();
        while !matches!(self.peek(), TokKind::Dedent | TokKind::Eof) {
            self.skip_newlines();
            if matches!(self.peek(), TokKind::Dedent | TokKind::Eof) {
                break;
            }
            arms.push(self.parse_route_arm()?);
        }

        let end = self.peek_span();
        if matches!(self.peek(), TokKind::Dedent) {
            self.bump();
        }

        if arms.is_empty() {
            return Err(ParseError {
                kind: ParseErrorKind::ExpectedBlock,
                span: start,
            });
        }

        Ok(RouteTable {
            arms,
            span: start.merge(end),
        })
    }

    fn parse_route_arm(&mut self) -> Result<RouteArm, ParseError> {
        let arm_start = self.peek_span();

        // A bare `_` on its own is the wildcard pattern. Anything
        // else is a guard expression, which we parse with the full
        // expression grammar (boolean ops, comparisons, calls).
        let pattern = if self.is_wildcard_token() {
            let span = self.peek_span();
            self.bump();
            RoutePattern::Wildcard { span }
        } else {
            RoutePattern::Guard(self.parse_expr()?)
        };

        self.expect(TokKind::Arrow, "`->` after route pattern")?;
        let (model_name, model_span) = self.expect_ident()?;
        let end = self.prev_span();
        self.expect_newline()?;

        Ok(RouteArm {
            pattern,
            model: Ident::new(model_name, model_span),
            span: arm_start.merge(end),
        })
    }

    /// Is the next token a lone `_` identifier?
    fn is_wildcard_token(&self) -> bool {
        matches!(self.peek(), TokKind::Ident(name) if name == "_")
    }

    /// Parse a `progressive:` block inside a prompt body. Caller has
    /// already positioned at the `progressive` keyword.
    fn parse_prompt_progressive_block(
        &mut self,
    ) -> Result<ProgressiveChain, ParseError> {
        let start = self.peek_span();
        self.bump(); // progressive
        self.expect(TokKind::Colon, "`:` after `progressive`")?;
        self.expect_newline()?;

        if !matches!(self.peek(), TokKind::Indent) {
            return Err(ParseError {
                kind: ParseErrorKind::ExpectedBlock,
                span: self.peek_span(),
            });
        }
        self.bump(); // Indent

        let mut stages = Vec::new();
        while !matches!(self.peek(), TokKind::Dedent | TokKind::Eof) {
            self.skip_newlines();
            if matches!(self.peek(), TokKind::Dedent | TokKind::Eof) {
                break;
            }
            stages.push(self.parse_progressive_stage()?);
        }

        let end = self.peek_span();
        if matches!(self.peek(), TokKind::Dedent) {
            self.bump();
        }

        if stages.len() < 2 {
            return Err(ParseError {
                kind: ParseErrorKind::UnexpectedToken {
                    got: format!("{} stage(s)", stages.len()),
                    expected: "at least two `progressive:` stages (primary + terminal fallback)".into(),
                },
                span: start,
            });
        }

        // Every stage except the last must declare a threshold.
        // The last stage must NOT declare one — it's the terminal
        // fallback that always runs.
        for (idx, stage) in stages.iter().enumerate() {
            let is_last = idx == stages.len() - 1;
            if is_last && stage.threshold.is_some() {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: "`below <threshold>` on the terminal stage".into(),
                        expected: "a bare model name on the last stage (terminal fallback always runs)".into(),
                    },
                    span: stage.span,
                });
            }
            if !is_last && stage.threshold.is_none() {
                return Err(ParseError {
                    kind: ParseErrorKind::UnexpectedToken {
                        got: "a non-terminal stage without `below <threshold>`".into(),
                        expected: "`<model> below <threshold>` on every stage except the last".into(),
                    },
                    span: stage.span,
                });
            }
        }

        Ok(ProgressiveChain {
            stages,
            span: start.merge(end),
        })
    }

    fn parse_progressive_stage(&mut self) -> Result<ProgressiveStage, ParseError> {
        let start = self.peek_span();
        let (model_name, model_span) = self.expect_ident()?;

        let threshold = if matches!(self.peek(), TokKind::KwBelow) {
            self.bump(); // below
            match self.peek().clone() {
                TokKind::Float(n) => {
                    self.bump();
                    Some(n)
                }
                TokKind::Int(n) => {
                    self.bump();
                    Some(n as f64)
                }
                other => {
                    return Err(ParseError {
                        kind: ParseErrorKind::UnexpectedToken {
                            got: describe_token(&other),
                            expected: "a numeric confidence threshold after `below`".into(),
                        },
                        span: self.peek_span(),
                    });
                }
            }
        } else {
            None
        };

        let end = self.prev_span();
        self.expect_newline()?;
        Ok(ProgressiveStage {
            model: Ident::new(model_name, model_span),
            threshold,
            span: start.merge(end),
        })
    }
}
