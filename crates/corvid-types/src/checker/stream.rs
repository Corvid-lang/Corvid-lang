//! Stream built-in call and method type rules.

use super::Checker;
use crate::errors::{TypeError, TypeErrorKind};
use crate::types::Type;
use corvid_ast::{Expr, Ident, Literal, Span};

impl<'a> Checker<'a> {
    pub(super) fn check_stream_merge_call(&mut self, name: &Ident, args: &[Expr]) -> Type {
        if args.len() != 1 {
            self.errors.push(TypeError::new(
                TypeErrorKind::ArityMismatch {
                    callee: name.name.clone(),
                    expected: 1,
                    got: args.len(),
                },
                name.span,
            ));
            for arg in args {
                let _ = self.check_expr(arg);
            }
            return Type::Stream(Box::new(Type::Unknown));
        }

        match self.check_expr(&args[0]) {
            Type::List(inner) => match *inner {
                Type::Stream(stream_inner) => Type::Stream(stream_inner),
                Type::Unknown => Type::Stream(Box::new(Type::Unknown)),
                other => {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: "List<Stream<T>>".into(),
                            got: format!("List<{}>", other.display_name()),
                            context: "merge argument".into(),
                        },
                        args[0].span(),
                    ));
                    Type::Stream(Box::new(Type::Unknown))
                }
            },
            Type::Unknown => Type::Stream(Box::new(Type::Unknown)),
            other => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::TypeMismatch {
                        expected: "List<Stream<T>>".into(),
                        got: other.display_name(),
                        context: "merge argument".into(),
                    },
                    args[0].span(),
                ));
                Type::Stream(Box::new(Type::Unknown))
            }
        }
    }

    pub(super) fn check_stream_split_by_method(
        &mut self,
        target: &Expr,
        recv_ty: &Type,
        method_name: &Ident,
        args: &[Expr],
    ) -> Type {
        if args.len() != 1 {
            self.errors.push(TypeError::new(
                TypeErrorKind::ArityMismatch {
                    callee: method_name.name.clone(),
                    expected: 1,
                    got: args.len(),
                },
                method_name.span,
            ));
            for arg in args {
                let _ = self.check_expr(arg);
            }
            return Type::List(Box::new(Type::Stream(Box::new(Type::Unknown))));
        }
        let key = match &args[0] {
            Expr::Literal {
                value: Literal::String(key),
                ..
            } => Some(key.as_str()),
            other => {
                let got = self.check_expr(other);
                self.errors.push(TypeError::new(
                    TypeErrorKind::TypeMismatch {
                        expected: "string literal field name".into(),
                        got: got.display_name(),
                        context: "split_by key".into(),
                    },
                    other.span(),
                ));
                None
            }
        };

        match recv_ty {
            Type::Stream(inner) => {
                self.validate_split_key(inner.as_ref(), key, args[0].span());
                Type::List(Box::new(Type::Stream(inner.clone())))
            }
            Type::Unknown => Type::List(Box::new(Type::Stream(Box::new(Type::Unknown)))),
            other => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::TypeMismatch {
                        expected: "Stream<Struct>".into(),
                        got: other.display_name(),
                        context: "split_by receiver".into(),
                    },
                    target.span(),
                ));
                Type::List(Box::new(Type::Stream(Box::new(Type::Unknown))))
            }
        }
    }

    pub(super) fn check_stream_ordered_by_method(
        &mut self,
        target: &Expr,
        recv_ty: Type,
        method_name: &Ident,
        args: &[Expr],
    ) -> Type {
        if args.len() != 1 {
            self.errors.push(TypeError::new(
                TypeErrorKind::ArityMismatch {
                    callee: method_name.name.clone(),
                    expected: 1,
                    got: args.len(),
                },
                method_name.span,
            ));
            for arg in args {
                let _ = self.check_expr(arg);
            }
            return recv_ty;
        }
        match &args[0] {
            Expr::Literal {
                value: Literal::String(policy),
                ..
            } if matches!(policy.as_str(), "fifo" | "fair_round_robin" | "sorted") => {}
            Expr::Literal {
                value: Literal::String(policy),
                ..
            } => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::TypeMismatch {
                        expected: "fifo, fair_round_robin, or sorted".into(),
                        got: policy.clone(),
                        context: "ordered_by policy".into(),
                    },
                    args[0].span(),
                ));
            }
            other => {
                let got = self.check_expr(other);
                self.errors.push(TypeError::new(
                    TypeErrorKind::TypeMismatch {
                        expected: "string literal ordering policy".into(),
                        got: got.display_name(),
                        context: "ordered_by policy".into(),
                    },
                    other.span(),
                ));
            }
        }
        match recv_ty {
            Type::Stream(_) => recv_ty,
            Type::Unknown => Type::Unknown,
            other => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::TypeMismatch {
                        expected: "Stream<T>".into(),
                        got: other.display_name(),
                        context: "ordered_by receiver".into(),
                    },
                    target.span(),
                ));
                Type::Unknown
            }
        }
    }

    fn validate_split_key(&mut self, inner: &Type, key: Option<&str>, span: Span) {
        let Some(key) = key else {
            return;
        };
        match inner {
            Type::Struct(def_id) => {
                let type_decl = *self
                    .types_by_id
                    .get(def_id)
                    .expect("struct DefId not indexed");
                if !type_decl.fields.iter().any(|field| field.name.name == key) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::UnknownField {
                            struct_name: type_decl.name.name.clone(),
                            field: key.to_string(),
                        },
                        span,
                    ));
                }
            }
            Type::Unknown | Type::ImportedStruct(_) => {}
            other => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::TypeMismatch {
                        expected: "Stream<Struct>".into(),
                        got: format!("Stream<{}>", other.display_name()),
                        context: "split_by receiver".into(),
                    },
                    span,
                ));
            }
        }
    }
}
