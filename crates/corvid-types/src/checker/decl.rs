//! Agent + eval declaration checking.
//!
//! `check_agent` validates an `agent name(params) -> T: body` —
//! parameter binding, return-type matching, yield/stream legality.
//! `check_eval` validates an `eval name: body` — including
//! trace-assert (`assert called X before Y`) and statistical
//! confidence modifiers.
//!
//! Extracted from `checker.rs` as part of Phase 20i responsibility
//! decomposition.

use super::Checker;
use crate::errors::{TypeError, TypeErrorKind, TypeWarning, TypeWarningKind};
use crate::types::Type;
use corvid_ast::{AgentDecl, EvalAssert, EvalDecl, Expr, Ident, Span};
use corvid_resolve::{Binding, DeclKind};

impl<'a> Checker<'a> {
    pub(super) fn check_agent(&mut self, a: &AgentDecl) {
        // Bind parameter types.
        self.bind_params(&a.params);

        let declared_ret = self.type_ref_to_type(&a.return_ty);
        let prev_ret = std::mem::replace(&mut self.current_return, Some(declared_ret.clone()));
        let prev_in_agent = std::mem::replace(&mut self.in_agent_body, true);
        let prev_saw_yield = std::mem::replace(&mut self.saw_yield, false);

        self.check_block(&a.body);

        if matches!(declared_ret, Type::Stream(_)) && !self.saw_yield {
            self.warnings.push(TypeWarning::new(
                TypeWarningKind::StreamReturnWithoutYield {
                    agent: a.name.name.clone(),
                },
                a.span,
            ));
        }

        self.current_return = prev_ret;
        self.in_agent_body = prev_in_agent;
        self.saw_yield = prev_saw_yield;
        // (Locals leak between agents in our single-scope model; harmless
        //  since each agent binds its params fresh at the start.)
    }


    pub(super) fn check_eval(&mut self, e: &EvalDecl) {
        let prev_ret = self.current_return.take();
        let prev_in_agent = std::mem::replace(&mut self.in_agent_body, false);
        let prev_saw_yield = self.saw_yield;
        self.check_block(&e.body);
        for assertion in &e.assertions {
            self.check_eval_assert(assertion);
        }
        self.current_return = prev_ret;
        self.in_agent_body = prev_in_agent;
        self.saw_yield = prev_saw_yield;
    }

    fn check_eval_assert(&mut self, assertion: &EvalAssert) {
        match assertion {
            EvalAssert::Value {
                expr,
                confidence,
                runs,
                span,
            } => {
                let ty = self.check_expr(expr);
                if !matches!(ty, Type::Bool | Type::Unknown) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::AssertNotBool {
                            got: ty.display_name(),
                        },
                        *span,
                    ));
                }
                if let Some(value) = confidence {
                    if !(0.0..=1.0).contains(value) {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::InvalidConfidence { value: *value },
                            *span,
                        ));
                    }
                }
                if matches!(runs, Some(0)) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::InvalidConfidence { value: 0.0 },
                        *span,
                    ));
                }
            }
            EvalAssert::Called { tool, span } => {
                self.check_eval_callable(tool, *span);
            }
            EvalAssert::Approved { label, span } => {
                if !self.has_known_approval_label(&label.name) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::EvalUnknownApproval {
                            label: label.name.clone(),
                        },
                        *span,
                    ));
                }
            }
            EvalAssert::Cost { .. } => {}
            EvalAssert::Ordering {
                before,
                after,
                span,
            } => {
                self.check_eval_callable(before, *span);
                self.check_eval_callable(after, *span);
            }
        }
    }

    fn check_eval_callable(&mut self, ident: &Ident, span: Span) {
        match self.bindings.get(&ident.span) {
            Some(Binding::Decl(def_id)) => match self.symbols.get(*def_id).kind {
                DeclKind::Tool | DeclKind::Prompt | DeclKind::Agent => {}
                _ => {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::EvalUnknownTool {
                            name: ident.name.clone(),
                        },
                        span,
                    ));
                }
            },
            _ => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::EvalUnknownTool {
                        name: ident.name.clone(),
                    },
                    span,
                ));
            }
        }
    }
}
