//! `check_prompt` — validates an entire prompt declaration,
//! including all five dispatch clauses (route / progressive /
//! rollout / ensemble / adversarial), stream modifiers, stage
//! chaining contracts, and the prompt body template.
//!
//! This is the largest single method in the type checker. It
//! enforces the invariants declared in docs/effects-spec/13 and
//! emits the phase 20h dispatch-family errors.
//!
//! Extracted from `checker.rs` as part of Phase 20i responsibility
//! decomposition.

use super::Checker;
use crate::errors::{TypeError, TypeErrorKind};
use crate::types::Type;
use corvid_ast::{Ident, PromptDecl, TypeRef};
use corvid_resolve::Binding;

impl<'a> Checker<'a> {
    pub(super) fn check_prompt(&mut self, p: &PromptDecl) {
        let return_ty = self.type_ref_to_type(&p.return_ty);
        let has_stream_modifiers = p.stream.min_confidence.is_some()
            || p.stream.max_tokens.is_some()
            || p.stream.backpressure.is_some()
            || p.stream.escalate_to.is_some();

        if has_stream_modifiers && !matches!(return_ty, Type::Stream(_) | Type::Unknown) {
            self.errors.push(TypeError::new(
                TypeErrorKind::TypeMismatch {
                    expected: "Stream<T>".into(),
                    got: return_ty.display_name(),
                    context: format!("stream modifiers on prompt `{}`", p.name.name),
                },
                p.span,
            ));
        }

        if let Some(confidence) = p.stream.min_confidence {
            if !(0.0..=1.0).contains(&confidence) {
                self.errors.push(TypeError::new(
                    TypeErrorKind::InvalidConfidence { value: confidence },
                    p.span,
                ));
            }
        }

        if let Some(model) = &p.stream.escalate_to {
            match self.bindings.get(&model.span) {
                Some(Binding::Decl(def_id)) => {
                    let entry = self.symbols.get(*def_id);
                    if entry.kind != corvid_resolve::DeclKind::Model {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::RouteTargetNotModel {
                                prompt: p.name.name.clone(),
                                target: model.name.clone(),
                                got_kind: format!("{:?}", entry.kind).to_lowercase(),
                            },
                            model.span,
                        ));
                    }
                }
                _ => {}
            }
        }

        if let Some(param_name) = &p.cites_strictly {
            match p
                .params
                .iter()
                .find(|param| param.name.name == *param_name)
            {
                Some(param) => {
                    let param_ty = self.type_ref_to_type(&param.ty);
                    if !matches!(param_ty, Type::Grounded(_) | Type::Unknown) {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::PromptCitationRequiresGrounded {
                                prompt: p.name.name.clone(),
                                param: param_name.clone(),
                                got: param_ty.display_name(),
                            },
                            param.name.span,
                        ));
                    }
                }
                None => self.errors.push(TypeError::new(
                    TypeErrorKind::PromptCitationUnknownParam {
                        prompt: p.name.name.clone(),
                        param: param_name.clone(),
                    },
                    p.span,
                )),
            }
        }

        // Phase 20h slice C: validate each route arm.
        if let Some(route) = &p.route {
            // Bind the prompt's params into scope so guard expressions
            // can reference them — guards typically look like
            // `domain(question) == math` where `question` is a param.
            self.bind_params(&p.params);
            for arm in &route.arms {
                if let corvid_ast::RoutePattern::Guard(expr) = &arm.pattern {
                    let ty = self.check_expr(expr);
                    if !matches!(ty, Type::Bool | Type::Unknown) {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::RouteGuardNotBool {
                                prompt: p.name.name.clone(),
                                got: ty.display_name(),
                            },
                            expr.span(),
                        ));
                    }
                }
                // Model ref — must resolve to a Decl::Model.
                if let Some(Binding::Decl(def_id)) = self.bindings.get(&arm.model.span) {
                    let def_id = *def_id;
                    let entry = self.symbols.get(def_id);
                    if entry.kind != corvid_resolve::DeclKind::Model {
                        let got = format!("{:?}", entry.kind).to_lowercase();
                        self.errors.push(TypeError::new(
                            TypeErrorKind::RouteTargetNotModel {
                                prompt: p.name.name.clone(),
                                target: arm.model.name.clone(),
                                got_kind: got,
                            },
                            arm.model.span,
                        ));
                    }
                }
            }
        }

        // Phase 20h slice E: validate each progressive stage. Model
        // refs must be Models (reuse `RouteTargetNotModel` — the
        // underlying invariant is identical). Thresholds must be in
        // [0.0, 1.0] — a confidence outside that range is ill-formed.
        if let Some(chain) = &p.progressive {
            for stage in &chain.stages {
                if let Some(Binding::Decl(def_id)) =
                    self.bindings.get(&stage.model.span)
                {
                    let def_id = *def_id;
                    let entry = self.symbols.get(def_id);
                    if entry.kind != corvid_resolve::DeclKind::Model {
                        let got = format!("{:?}", entry.kind).to_lowercase();
                        self.errors.push(TypeError::new(
                            TypeErrorKind::RouteTargetNotModel {
                                prompt: p.name.name.clone(),
                                target: stage.model.name.clone(),
                                got_kind: got,
                            },
                            stage.model.span,
                        ));
                    }
                }
                if let Some(t) = stage.threshold {
                    if !(0.0..=1.0).contains(&t) {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::InvalidConfidence { value: t },
                            stage.span,
                        ));
                    }
                }
            }
        }

        // Phase 20h slice I: validate the rollout percentage is in
        // [0.0, 100.0] and both idents are Models.
        if let Some(spec) = &p.rollout {
            if !(0.0..=100.0).contains(&spec.variant_percent) {
                self.errors.push(TypeError::new(
                    TypeErrorKind::RolloutPercentOutOfRange {
                        prompt: p.name.name.clone(),
                        got: spec.variant_percent,
                    },
                    spec.span,
                ));
            }
            for ident in [&spec.variant, &spec.baseline] {
                if let Some(Binding::Decl(def_id)) = self.bindings.get(&ident.span) {
                    let def_id = *def_id;
                    let entry = self.symbols.get(def_id);
                    if entry.kind != corvid_resolve::DeclKind::Model {
                        let got = format!("{:?}", entry.kind).to_lowercase();
                        self.errors.push(TypeError::new(
                            TypeErrorKind::RouteTargetNotModel {
                                prompt: p.name.name.clone(),
                                target: ident.name.clone(),
                                got_kind: got,
                            },
                            ident.span,
                        ));
                    }
                }
            }
        }

        // Phase 20h slice F: every ensemble model must resolve to a
        // `Decl::Model`. Duplicate model names in the list are flagged
        // as `EnsembleDuplicateModel` — a vote where two slots are the
        // same provider-model pair degenerates to voting only twice,
        // which is almost always unintended.
        if let Some(spec) = &p.ensemble {
            let mut seen = std::collections::BTreeSet::new();
            for model in &spec.models {
                if !seen.insert(model.name.clone()) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::EnsembleDuplicateModel {
                            prompt: p.name.name.clone(),
                            model: model.name.clone(),
                        },
                        model.span,
                    ));
                }
                if let Some(Binding::Decl(def_id)) = self.bindings.get(&model.span) {
                    let def_id = *def_id;
                    let entry = self.symbols.get(def_id);
                    if entry.kind != corvid_resolve::DeclKind::Model {
                        let got = format!("{:?}", entry.kind).to_lowercase();
                        self.errors.push(TypeError::new(
                            TypeErrorKind::RouteTargetNotModel {
                                prompt: p.name.name.clone(),
                                target: model.name.clone(),
                                got_kind: got,
                            },
                            model.span,
                        ));
                    }
                }
            }
        }

        // Phase 20h slice G (compiler follow-up): each adversarial
        // stage must resolve to a `DeclKind::Prompt`. The runtime
        // chains stage outputs as positional arguments to the next
        // stage, so stages must be function-like — prompts qualify,
        // bare models do not.
        //
        // Validation contract:
        //   propose:    arity + param types match the outer prompt;
        //               return type T1 is unconstrained.
        //   challenge:  arity = 1, param[0] accepts T1;
        //               return type T2 is unconstrained.
        //   adjudicate: arity = 2, param[0] accepts T1, param[1]
        //               accepts T2; return type must match the
        //               outer prompt's return type AND be a struct
        //               with a `contradiction: Bool` field.
        //
        // The `contradiction: Bool` contract makes
        // `TraceEvent::AdversarialContradiction` reachable — the
        // runtime reads this field to decide whether to emit it.
        if let Some(spec) = &p.adversarial {
            // Resolve each stage to a prompt decl. Stages that are
            // not prompts produce their own diagnostic and are
            // skipped for downstream arity / type checks.
            let stages: [(&'static str, &Ident); 3] = [
                ("propose", &spec.proposer),
                ("challenge", &spec.challenger),
                ("adjudicate", &spec.adjudicator),
            ];
            let mut prompt_refs: [Option<&'a PromptDecl>; 3] = [None, None, None];
            for (idx, (role, ident)) in stages.iter().enumerate() {
                if let Some(Binding::Decl(def_id)) = self.bindings.get(&ident.span) {
                    let def_id = *def_id;
                    let kind = self.symbols.get(def_id).kind;
                    if kind == corvid_resolve::DeclKind::Prompt {
                        prompt_refs[idx] = self.prompts_by_id.get(&def_id).copied();
                    } else {
                        let got = format!("{kind:?}").to_lowercase();
                        self.errors.push(TypeError::new(
                            TypeErrorKind::AdversarialStageNotPrompt {
                                prompt: p.name.name.clone(),
                                stage: (*role).into(),
                                target: ident.name.clone(),
                                got_kind: got,
                            },
                            ident.span,
                        ));
                    }
                }
            }

            // Only run arity / type chaining when every stage
            // resolved to a prompt — otherwise errors cascade on
            // unresolved stage shapes.
            if let [Some(proposer), Some(challenger), Some(adjudicator)] = prompt_refs {
                let outer_params: Vec<Type> = p
                    .params
                    .iter()
                    .map(|param| self.type_ref_to_type(&param.ty))
                    .collect();
                let outer_ret = self.type_ref_to_type(&p.return_ty);

                // ---- propose: arity + params match outer prompt --
                let prop_params: Vec<Type> = proposer
                    .params
                    .iter()
                    .map(|param| self.type_ref_to_type(&param.ty))
                    .collect();
                if prop_params.len() != outer_params.len() {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::AdversarialStageArity {
                            prompt: p.name.name.clone(),
                            stage: "propose".into(),
                            target: spec.proposer.name.clone(),
                            expected: outer_params.len(),
                            got: prop_params.len(),
                        },
                        spec.proposer.span,
                    ));
                } else {
                    for (i, (outer, got)) in
                        outer_params.iter().zip(prop_params.iter()).enumerate()
                    {
                        if !outer.is_assignable_to(got)
                            && !matches!(outer, Type::Unknown)
                            && !matches!(got, Type::Unknown)
                        {
                            self.errors.push(TypeError::new(
                                TypeErrorKind::AdversarialStageParamType {
                                    prompt: p.name.name.clone(),
                                    stage: "propose".into(),
                                    target: spec.proposer.name.clone(),
                                    index: i,
                                    expected: outer.display_name(),
                                    got: got.display_name(),
                                },
                                spec.proposer.span,
                            ));
                        }
                    }
                }
                let prop_ret = self.type_ref_to_type(&proposer.return_ty);

                // ---- challenge: arity = 1, accepts prop_ret ------
                let chal_params: Vec<Type> = challenger
                    .params
                    .iter()
                    .map(|param| self.type_ref_to_type(&param.ty))
                    .collect();
                if chal_params.len() != 1 {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::AdversarialStageArity {
                            prompt: p.name.name.clone(),
                            stage: "challenge".into(),
                            target: spec.challenger.name.clone(),
                            expected: 1,
                            got: chal_params.len(),
                        },
                        spec.challenger.span,
                    ));
                } else if !prop_ret.is_assignable_to(&chal_params[0])
                    && !matches!(prop_ret, Type::Unknown)
                    && !matches!(chal_params[0], Type::Unknown)
                {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::AdversarialStageParamType {
                            prompt: p.name.name.clone(),
                            stage: "challenge".into(),
                            target: spec.challenger.name.clone(),
                            index: 0,
                            expected: prop_ret.display_name(),
                            got: chal_params[0].display_name(),
                        },
                        spec.challenger.span,
                    ));
                }
                let chal_ret = self.type_ref_to_type(&challenger.return_ty);

                // ---- adjudicate: arity = 2, accepts (prop_ret, chal_ret) --
                let adj_params: Vec<Type> = adjudicator
                    .params
                    .iter()
                    .map(|param| self.type_ref_to_type(&param.ty))
                    .collect();
                if adj_params.len() != 2 {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::AdversarialStageArity {
                            prompt: p.name.name.clone(),
                            stage: "adjudicate".into(),
                            target: spec.adjudicator.name.clone(),
                            expected: 2,
                            got: adj_params.len(),
                        },
                        spec.adjudicator.span,
                    ));
                } else {
                    if !prop_ret.is_assignable_to(&adj_params[0])
                        && !matches!(prop_ret, Type::Unknown)
                        && !matches!(adj_params[0], Type::Unknown)
                    {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::AdversarialStageParamType {
                                prompt: p.name.name.clone(),
                                stage: "adjudicate".into(),
                                target: spec.adjudicator.name.clone(),
                                index: 0,
                                expected: prop_ret.display_name(),
                                got: adj_params[0].display_name(),
                            },
                            spec.adjudicator.span,
                        ));
                    }
                    if !chal_ret.is_assignable_to(&adj_params[1])
                        && !matches!(chal_ret, Type::Unknown)
                        && !matches!(adj_params[1], Type::Unknown)
                    {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::AdversarialStageParamType {
                                prompt: p.name.name.clone(),
                                stage: "adjudicate".into(),
                                target: spec.adjudicator.name.clone(),
                                index: 1,
                                expected: chal_ret.display_name(),
                                got: adj_params[1].display_name(),
                            },
                            spec.adjudicator.span,
                        ));
                    }
                }
                let adj_ret = self.type_ref_to_type(&adjudicator.return_ty);

                // ---- adjudicator return = outer return -----------
                if !adj_ret.is_assignable_to(&outer_ret)
                    && !matches!(adj_ret, Type::Unknown)
                    && !matches!(outer_ret, Type::Unknown)
                {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::AdversarialStageReturnType {
                            prompt: p.name.name.clone(),
                            stage: "adjudicate".into(),
                            target: spec.adjudicator.name.clone(),
                            expected: outer_ret.display_name(),
                            got: adj_ret.display_name(),
                        },
                        spec.adjudicator.span,
                    ));
                }

                // ---- adjudicator return carries `contradiction: Bool` --
                // Clone out the matching field's TypeRef first so
                // we can release the `types_by_id` borrow before
                // calling `type_ref_to_type` (which needs &mut self).
                let contradiction_ok = match &adj_ret {
                    Type::Struct(def_id) => {
                        let field_types: Vec<TypeRef> = self
                            .types_by_id
                            .get(def_id)
                            .map(|td| {
                                td.fields
                                    .iter()
                                    .filter(|f| f.name.name == "contradiction")
                                    .map(|f| f.ty.clone())
                                    .collect()
                            })
                            .unwrap_or_default();
                        field_types
                            .iter()
                            .any(|tr| matches!(self.type_ref_to_type(tr), Type::Bool))
                    }
                    Type::Unknown => true,
                    _ => false,
                };
                if !contradiction_ok {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::AdversarialAdjudicatorMissingContradictionField {
                            prompt: p.name.name.clone(),
                            target: spec.adjudicator.name.clone(),
                            got: adj_ret.display_name(),
                        },
                        spec.adjudicator.span,
                    ));
                }
            }
        }
    }
}
