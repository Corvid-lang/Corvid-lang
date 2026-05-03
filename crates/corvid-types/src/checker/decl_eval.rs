use super::Checker;
use crate::errors::{TypeError, TypeErrorKind};
use crate::types::Type;
use corvid_ast::{Block, EvalAssert, EvalDecl, FixtureDecl, Ident, MockDecl, Span, TestDecl};
use corvid_resolve::{Binding, DeclKind};

impl<'a> Checker<'a> {
    pub(super) fn check_eval(&mut self, e: &EvalDecl) {
        self.check_assertion_decl(&e.body, &e.assertions);
    }

    pub(super) fn check_test(&mut self, t: &TestDecl) {
        let prev = std::mem::replace(&mut self.in_test_body, true);
        self.check_assertion_decl(&t.body, &t.assertions);
        self.in_test_body = prev;
    }

    pub(super) fn check_fixture(&mut self, f: &FixtureDecl) {
        self.bind_params(&f.params);
        let declared_ret = self.type_ref_to_type(&f.return_ty);
        let prev_ret = std::mem::replace(&mut self.current_return, Some(declared_ret));
        let prev_in_agent = std::mem::replace(&mut self.in_agent_body, false);
        let prev_in_test = std::mem::replace(&mut self.in_test_body, true);
        let prev_saw_yield = std::mem::replace(&mut self.saw_yield, false);
        self.check_block(&f.body);
        self.current_return = prev_ret;
        self.in_agent_body = prev_in_agent;
        self.in_test_body = prev_in_test;
        self.saw_yield = prev_saw_yield;
    }

    pub(super) fn check_mock(&mut self, m: &MockDecl) {
        let target = match self.bindings.get(&m.target.span) {
            Some(Binding::Decl(def_id)) if self.symbols.get(*def_id).kind == DeclKind::Tool => {
                Some(*def_id)
            }
            _ => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::NotCallable {
                        got: format!("mock target `{}`", m.target.name),
                    },
                    m.target.span,
                ));
                None
            }
        };
        if let Some(def_id) = target {
            let tool = *self
                .tools_by_id
                .get(&def_id)
                .expect("tool DefId not indexed");
            if tool.params.len() != m.params.len() {
                self.errors.push(TypeError::new(
                    TypeErrorKind::ArityMismatch {
                        callee: format!("mock {}", m.target.name),
                        expected: tool.params.len(),
                        got: m.params.len(),
                    },
                    m.span,
                ));
            }
            for (tool_param, mock_param) in tool.params.iter().zip(&m.params) {
                let expected = self.type_ref_to_type(&tool_param.ty);
                let got = self.type_ref_to_type(&mock_param.ty);
                if expected != got {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: expected.display_name(),
                            got: got.display_name(),
                            context: format!(
                                "mock `{}` parameter `{}`",
                                m.target.name, mock_param.name.name
                            ),
                        },
                        mock_param.span,
                    ));
                }
            }
            let expected = self.type_ref_to_type(&tool.return_ty);
            let got = self.type_ref_to_type(&m.return_ty);
            if expected != got {
                self.errors.push(TypeError::new(
                    TypeErrorKind::TypeMismatch {
                        expected: expected.display_name(),
                        got: got.display_name(),
                        context: format!("mock `{}` return type", m.target.name),
                    },
                    m.return_ty.span(),
                ));
            }
        }
        self.bind_params(&m.params);
        let declared_ret = self.type_ref_to_type(&m.return_ty);
        let prev_ret = std::mem::replace(&mut self.current_return, Some(declared_ret));
        let prev_in_agent = std::mem::replace(&mut self.in_agent_body, false);
        let prev_in_test = std::mem::replace(&mut self.in_test_body, true);
        let prev_saw_yield = std::mem::replace(&mut self.saw_yield, false);
        self.resolve_effect_row_in_mock(&m.effect_row);
        self.check_block(&m.body);
        self.current_return = prev_ret;
        self.in_agent_body = prev_in_agent;
        self.in_test_body = prev_in_test;
        self.saw_yield = prev_saw_yield;
    }

    fn resolve_effect_row_in_mock(&mut self, row: &corvid_ast::EffectRow) {
        for effect in &row.effects {
            if !matches!(self.bindings.get(&effect.name.span), Some(Binding::Decl(_))) {
                self.errors.push(TypeError::new(
                    TypeErrorKind::EvalUnknownTool {
                        name: effect.name.name.clone(),
                    },
                    effect.span,
                ));
            }
        }
    }

    fn check_assertion_decl(&mut self, body: &Block, assertions: &[EvalAssert]) {
        let prev_ret = self.current_return.take();
        let prev_in_agent = std::mem::replace(&mut self.in_agent_body, false);
        let prev_saw_yield = self.saw_yield;
        self.check_block(body);
        for assertion in assertions {
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
                        self.errors.push(TypeError::with_guarantee(
                            TypeErrorKind::InvalidConfidence { value: *value },
                            *span,
                            "confidence.min_threshold",
                        ));
                    }
                }
                if matches!(runs, Some(0)) {
                    self.errors.push(TypeError::with_guarantee(
                        TypeErrorKind::InvalidConfidence { value: 0.0 },
                        *span,
                        "confidence.min_threshold",
                    ));
                }
            }
            EvalAssert::Snapshot { expr, .. } => {
                self.check_expr(expr);
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
