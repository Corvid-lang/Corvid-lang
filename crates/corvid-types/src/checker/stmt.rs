//! Statement + block + approve tracking.
//!
//! Walks every statement in a block body, tracking control-flow
//! side effects (let-bind types, approve-stack additions, yield
//! legality, return type matches, effect-frontier bumps).
//!
//! Extracted from `checker.rs` as part of Phase 20i responsibility
//! decomposition.

use super::{Approval, Checker};
use crate::errors::{TypeError, TypeErrorKind};
use crate::types::Type;
use corvid_ast::{Block, Expr, Stmt, WeakEffect};
use corvid_resolve::Binding;

impl<'a> Checker<'a> {
    pub(super) fn check_block(&mut self, b: &Block) {
        // Save approval-stack depth so approvals don't leak out of this block.
        let saved_depth = self.approvals.len();
        for stmt in &b.stmts {
            self.check_stmt(stmt);
        }
        self.approvals.truncate(saved_depth);
    }

    pub(super) fn check_stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let { name, ty, value, .. } => {
                let explicit_ty = ty.as_ref().map(|t| self.type_ref_to_type(t));
                let value_ty = self.check_expr_as(value, explicit_ty.as_ref());
                let local_ty = match ty {
                    Some(_) => explicit_ty.expect("explicit let type already computed"),
                    None => value_ty.clone(),
                };
                if !value_ty.is_assignable_to(&local_ty) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: local_ty.display_name(),
                            got: value_ty.display_name(),
                            context: format!("assignment to `{}`", name.name),
                        },
                        value.span(),
                    ));
                }
                if let Some(Binding::Local(local_id)) = self.bindings.get(&name.span) {
                    self.update_weak_local_on_assignment(*local_id, value, &local_ty);
                    self.local_types.insert(*local_id, local_ty);
                }
            }
            Stmt::Return { value, span } => {
                let got = match value {
                    Some(e) => {
                        let expected = self.current_return.clone();
                        self.check_expr_as(e, expected.as_ref())
                    }
                    None => Type::Nothing,
                };
                if let Some(expected) = &self.current_return {
                    if !got.is_assignable_to(expected) {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::ReturnTypeMismatch {
                                expected: expected.display_name(),
                                got: got.display_name(),
                            },
                            *span,
                        ));
                    }
                }
            }
            Stmt::If { cond, then_block, else_block, .. } => {
                let cond_ty = self.check_expr(cond);
                if !matches!(cond_ty, Type::Bool | Type::Unknown) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: "Bool".into(),
                            got: cond_ty.display_name(),
                            context: "`if` condition".into(),
                        },
                        cond.span(),
                    ));
                }
                let entry_frontier = self.effect_frontier;
                let entry_weak_refresh = self.weak_refresh.clone();

                self.effect_frontier = entry_frontier;
                self.weak_refresh = entry_weak_refresh.clone();
                self.check_block(then_block);
                let then_frontier = self.effect_frontier;
                let then_refresh = self.weak_refresh.clone();

                let (else_frontier, else_refresh) = if let Some(b) = else_block {
                    self.effect_frontier = entry_frontier;
                    self.weak_refresh = entry_weak_refresh.clone();
                    self.check_block(b);
                    (self.effect_frontier, self.weak_refresh.clone())
                } else {
                    (entry_frontier, entry_weak_refresh.clone())
                };

                self.effect_frontier = then_frontier.merge_max(else_frontier);
                self.weak_refresh = self.merge_weak_refresh(
                    &entry_weak_refresh,
                    &then_refresh,
                    &else_refresh,
                );
            }
            Stmt::Yield { value, span } => {
                let yielded = self.check_expr(value);
                if !self.in_agent_body {
                    self.errors
                        .push(TypeError::new(TypeErrorKind::YieldOutsideAgent, *span));
                    return;
                }
                match self.current_return.as_ref() {
                    Some(Type::Stream(inner)) => {
                        self.saw_yield = true;
                        if !yielded.is_assignable_to(inner) {
                            self.errors.push(TypeError::new(
                                TypeErrorKind::YieldReturnTypeMismatch {
                                    expected: inner.display_name(),
                                    got: yielded.display_name(),
                                },
                                value.span(),
                            ));
                        }
                    }
                    Some(other) => {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::YieldRequiresStreamReturn {
                                declared: other.display_name(),
                            },
                            *span,
                        ));
                    }
                    None => {
                        self.errors
                            .push(TypeError::new(TypeErrorKind::YieldOutsideAgent, *span));
                    }
                }
            }
            Stmt::For { var, iter, body, .. } => {
                let iter_ty = self.check_expr(iter);
                // Derive the loop variable's type from the iterable.
                // Lists iterate their element type; Strings iterate
                // chars (which Corvid currently models as String).
                let var_ty = match &iter_ty {
                    Type::List(elem) => (**elem).clone(),
                    Type::Stream(elem) => (**elem).clone(),
                    Type::String => Type::String,
                    Type::Unknown => Type::Unknown,
                    _other => Type::Unknown,
                };
                if let Some(Binding::Local(local_id)) = self.bindings.get(&var.span) {
                    self.local_types.insert(*local_id, var_ty);
                }
                let entry_frontier = self.effect_frontier;
                let entry_weak_refresh = self.weak_refresh.clone();
                self.check_block(body);
                let body_frontier = self.effect_frontier;
                let body_refresh = self.weak_refresh.clone();
                self.effect_frontier = entry_frontier.merge_max(body_frontier);
                self.weak_refresh = self.merge_weak_refresh(
                    &entry_weak_refresh,
                    &entry_weak_refresh,
                    &body_refresh,
                );
            }
            Stmt::Approve { action, .. } => {
                self.check_approve(action);
                self.bump_effect(WeakEffect::Approve);
            }
            Stmt::Expr { expr, .. } => {
                let _ = self.check_expr(expr);
            }
        }
    }

    pub(super) fn check_approve(&mut self, action: &Expr) {
        if let Expr::Call { callee, args, .. } = action {
            if let Expr::Ident { name, .. } = &**callee {
                self.approvals.push(Approval {
                    label: name.name.clone(),
                    arity: args.len(),
                });
            }
            // Always typecheck the args themselves for binding validity.
            for arg in args {
                let _ = self.check_expr(arg);
            }
        } else {
            let _ = self.check_expr(action);
        }
    }
}
