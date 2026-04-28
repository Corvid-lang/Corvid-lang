//! Operator and try-form expression checks.
//!
//! Covers the subexpression shapes that aren't primary values or
//! calls: binary operators (arithmetic + comparison + logical),
//! unary operators (negation + boolean not), and the two `try`
//! forms (`expr?` propagation and `try expr on error retry …`).
//!
//! Extracted from `checker.rs` as part of Phase 20i responsibility
//! decomposition. All four methods extend the `Checker` impl in a
//! sibling submodule.

use super::Checker;
use crate::errors::{TypeError, TypeErrorKind};
use crate::types::Type;
use corvid_ast::{BinaryOp, Expr, Span, UnaryOp};

impl<'a> Checker<'a> {
    pub(super) fn check_binop(&mut self, op: BinaryOp, l: &Expr, r: &Expr, _span: Span) -> Type {
        let lt = self.check_expr(l);
        let rt = self.check_expr(r);
        use BinaryOp::*;
        match op {
            // `+` is overloaded: numeric addition OR string concatenation.
            Add => match (&lt, &rt) {
                (Type::Int, Type::Int) => Type::Int,
                (Type::Float, Type::Float)
                | (Type::Int, Type::Float)
                | (Type::Float, Type::Int) => Type::Float,
                (Type::String, Type::String) => Type::String,
                (Type::List(a), Type::List(b)) if a.is_assignable_to(b) => Type::List(b.clone()),
                (Type::List(a), Type::List(b)) if b.is_assignable_to(a) => Type::List(a.clone()),
                (Type::Unknown, _) | (_, Type::Unknown) => Type::Unknown,
                (a, b) => {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: "Int, Float, two Strings, or two compatible Lists".into(),
                            got: format!("{} and {}", a.display_name(), b.display_name()),
                            context: "`+` operator".into(),
                        },
                        l.span().merge(r.span()),
                    ));
                    Type::Unknown
                }
            },
            Sub | Mul | Div | Mod => match (&lt, &rt) {
                (Type::Int, Type::Int) => Type::Int,
                (Type::Float, Type::Float)
                | (Type::Int, Type::Float)
                | (Type::Float, Type::Int) => Type::Float,
                (Type::Unknown, _) | (_, Type::Unknown) => Type::Unknown,
                (a, b) => {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: "Int or Float".into(),
                            got: format!("{} and {}", a.display_name(), b.display_name()),
                            context: "arithmetic operator".into(),
                        },
                        l.span().merge(r.span()),
                    ));
                    Type::Unknown
                }
            },
            Eq | NotEq | Lt | LtEq | Gt | GtEq => {
                if !lt.is_assignable_to(&rt) && !rt.is_assignable_to(&lt) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: lt.display_name(),
                            got: rt.display_name(),
                            context: "comparison".into(),
                        },
                        l.span().merge(r.span()),
                    ));
                }
                Type::Bool
            }
            And | Or => {
                if !matches!(lt, Type::Bool | Type::Unknown) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: "Bool".into(),
                            got: lt.display_name(),
                            context: "logical operator".into(),
                        },
                        l.span(),
                    ));
                }
                if !matches!(rt, Type::Bool | Type::Unknown) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: "Bool".into(),
                            got: rt.display_name(),
                            context: "logical operator".into(),
                        },
                        r.span(),
                    ));
                }
                Type::Bool
            }
        }
    }

    pub(super) fn check_unop(&mut self, op: UnaryOp, operand: &Expr) -> Type {
        let t = self.check_expr(operand);
        match op {
            UnaryOp::Neg => match t {
                Type::Int => Type::Int,
                Type::Float => Type::Float,
                Type::Unknown => Type::Unknown,
                other => {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: "Int or Float".into(),
                            got: other.display_name(),
                            context: "unary `-`".into(),
                        },
                        operand.span(),
                    ));
                    Type::Unknown
                }
            },
            UnaryOp::Not => match t {
                Type::Bool | Type::Unknown => Type::Bool,
                other => {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: "Bool".into(),
                            got: other.display_name(),
                            context: "unary `not`".into(),
                        },
                        operand.span(),
                    ));
                    Type::Bool
                }
            },
        }
    }

    pub(super) fn check_try_propagate(&mut self, inner: &Expr, span: Span) -> Type {
        let inner_ty = self.check_expr(inner);
        match inner_ty {
            Type::Result(ok, err) => {
                self.ensure_try_return_context(
                    &Type::Result(Box::new(Type::Unknown), err.clone()),
                    span,
                );
                (*ok).clone()
            }
            Type::Option(inner) => {
                self.ensure_try_return_context(&Type::Option(Box::new(Type::Unknown)), span);
                (*inner).clone()
            }
            Type::Unknown => Type::Unknown,
            other => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::InvalidTryPropagate {
                        got: other.display_name(),
                    },
                    span,
                ));
                Type::Unknown
            }
        }
    }

    pub(super) fn check_try_retry(&mut self, body: &Expr, span: Span) -> Type {
        let body_ty = self.check_expr(body);
        match body_ty {
            Type::Result(_, _) | Type::Option(_) | Type::Stream(_) | Type::Unknown => body_ty,
            other => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::InvalidRetryTarget {
                        got: other.display_name(),
                    },
                    span,
                ));
                Type::Unknown
            }
        }
    }

    fn ensure_try_return_context(&mut self, required: &Type, span: Span) {
        match &self.current_return {
            Some(current) if required.is_assignable_to(current) => {}
            Some(current) => self.errors.push(TypeError::new(
                TypeErrorKind::TryPropagateReturnMismatch {
                    expected: required.display_name(),
                    got: current.display_name(),
                },
                span,
            )),
            None => self.errors.push(TypeError::new(
                TypeErrorKind::TryPropagateReturnMismatch {
                    expected: required.display_name(),
                    got: "no enclosing return type".into(),
                },
                span,
            )),
        }
    }
}
