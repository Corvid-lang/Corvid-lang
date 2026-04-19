//! Expression dispatch, identifier/decl lookup, and field access.
//!
//! `check_expr_as` is the main expression dispatch — it walks
//! every `Expr` variant and routes to the right sub-checker
//! (ops for binary/unary/try; call for call nodes; primitive
//! checks for literal/ident/field/list/index). `type_of_ident`
//! and `type_of_decl` resolve names to types. `check_field`
//! validates struct field access.
//!
//! Extracted from `checker.rs` as part of Phase 20i responsibility
//! decomposition.

use super::Checker;
use crate::errors::{TypeError, TypeErrorKind};
use crate::types::Type;
use corvid_ast::{Expr, Ident, Literal, Span};
use corvid_resolve::{Binding, BuiltIn, DeclKind, DefId};

impl<'a> Checker<'a> {
    pub(super) fn check_expr(&mut self, e: &Expr) -> Type {
        self.check_expr_as(e, None)
    }

    pub(super) fn check_expr_as(&mut self, e: &Expr, expected: Option<&Type>) -> Type {
        let ty = match e {
            Expr::Literal { value, .. } => match value {
                Literal::Int(_) => Type::Int,
                Literal::Float(_) => Type::Float,
                Literal::String(_) => Type::String,
                Literal::Bool(_) => Type::Bool,
                Literal::Nothing => Type::Nothing,
            },
            Expr::Ident { name, .. } => self.type_of_ident(name),
            Expr::Call { callee, args, span } => {
                self.check_call(callee, args, *span, expected)
            }
            Expr::FieldAccess { target, field, span } => self.check_field(target, field, *span),
            Expr::Index { target, index, span } => {
                let target_ty = self.check_expr(target);
                let index_ty = self.check_expr(index);
                // Index must be Int.
                if !matches!(index_ty, Type::Int | Type::Unknown) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: "Int".into(),
                            got: index_ty.display_name(),
                            context: "list index".into(),
                        },
                        index.span(),
                    ));
                }
                match target_ty {
                    Type::List(elem) => (*elem).clone(),
                    Type::Unknown => Type::Unknown,
                    other => {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::TypeMismatch {
                                expected: "List".into(),
                                got: other.display_name(),
                                context: "indexed value".into(),
                            },
                            *span,
                        ));
                        Type::Unknown
                    }
                }
            }
            Expr::BinOp { op, left, right, span } => self.check_binop(*op, left, right, *span),
            Expr::UnOp { op, operand, .. } => self.check_unop(*op, operand),
            Expr::List { items, span } => {
                // Infer element type from the first item; every other
                // item must be assignable to it.
                let mut elem_ty = Type::Unknown;
                for (i, item) in items.iter().enumerate() {
                    let item_ty = self.check_expr(item);
                    if i == 0 {
                        elem_ty = item_ty;
                    } else if !item_ty.is_assignable_to(&elem_ty)
                        && !matches!(elem_ty, Type::Unknown)
                        && !matches!(item_ty, Type::Unknown)
                    {
                        // Allow Int → Float promotion (matching binop rule).
                        if !(matches!(elem_ty, Type::Int) && matches!(item_ty, Type::Float)
                            || matches!(elem_ty, Type::Float)
                                && matches!(item_ty, Type::Int))
                        {
                            self.errors.push(TypeError::new(
                                TypeErrorKind::TypeMismatch {
                                    expected: elem_ty.display_name(),
                                    got: item_ty.display_name(),
                                    context: format!("list element {}", i + 1),
                                },
                                item.span(),
                            ));
                        } else if matches!(elem_ty, Type::Int) && matches!(item_ty, Type::Float) {
                            // Promote list to Float.
                            elem_ty = Type::Float;
                        }
                    }
                }
                let _ = span;
                Type::List(Box::new(elem_ty))
            }
            Expr::TryPropagate { inner, span } => self.check_try_propagate(inner, *span),
            Expr::TryRetry { body, span, .. } => self.check_try_retry(body, *span),
            Expr::Replay {
                trace,
                arms,
                else_body,
                ..
            } => {
                // Surface-level check: typecheck subexpressions so
                // their errors surface, but treat the replay block
                // itself as Unknown-typed. The pattern-exhaustiveness
                // + TraceId / TraceEvent types land with
                // 21-inv-E-3; until then a replay block is a valid
                // expression whose result type the checker doesn't
                // yet pin down.
                self.check_expr(trace);
                for arm in arms {
                    self.check_expr(&arm.body);
                }
                self.check_expr(else_body);
                Type::Unknown
            }
        };
        self.types.insert(e.span(), ty.clone());
        ty
    }

    pub(super) fn type_of_ident(&mut self, id: &Ident) -> Type {
        let Some(binding) = self.bindings.get(&id.span) else {
            // Could be the resolver-skipped callee of an approve label —
            // the approve path handles that; in other contexts we give up
            // gracefully to avoid cascading errors.
            return Type::Unknown;
        };
        match binding {
            Binding::Local(lid) => self
                .local_types
                .get(lid)
                .cloned()
                .unwrap_or(Type::Unknown),
            Binding::Decl(def_id) => self.type_of_decl(*def_id, id),
            Binding::BuiltIn(b) => match b {
                BuiltIn::Int
                | BuiltIn::Float
                | BuiltIn::String
                | BuiltIn::Bool
                | BuiltIn::Nothing
                | BuiltIn::List
                | BuiltIn::Stream
                | BuiltIn::Result
                | BuiltIn::Option
                | BuiltIn::Weak
                | BuiltIn::Grounded => {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeAsValue {
                            name: id.name.clone(),
                        },
                        id.span,
                    ));
                    Type::Unknown
                }
                BuiltIn::Ok
                | BuiltIn::Err
                | BuiltIn::Some
                | BuiltIn::WeakNew
                | BuiltIn::WeakUpgrade => {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::BareFunctionReference {
                            name: id.name.clone(),
                        },
                        id.span,
                    ));
                    Type::Unknown
                }
                BuiltIn::None => Type::Option(Box::new(Type::Unknown)),
                BuiltIn::Break | BuiltIn::Continue | BuiltIn::Pass => Type::Nothing,
            },
        }
    }

    /// Produce the value-position type of a top-level declaration.
    pub(super) fn type_of_decl(&mut self, id: DefId, ident: &Ident) -> Type {
        let entry = self.symbols.get(id);
        match entry.kind {
            DeclKind::Tool | DeclKind::Prompt | DeclKind::Agent => {
                // Referencing without a call is currently an error.
                // (Callers that need the function signature look it up by id.)
                self.errors.push(TypeError::new(
                    TypeErrorKind::BareFunctionReference {
                        name: ident.name.clone(),
                    },
                    ident.span,
                ));
                Type::Unknown
            }
            DeclKind::Type => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::TypeAsValue {
                        name: ident.name.clone(),
                    },
                    ident.span,
                ));
                Type::Unknown
            }
            DeclKind::Import | DeclKind::Eval | DeclKind::Effect | DeclKind::Model => {
                Type::Unknown
            }
        }
    }


    pub(super) fn check_field(&mut self, target: &Expr, field: &Ident, span: Span) -> Type {
        let target_ty = self.check_expr(target);
        match &target_ty {
            Type::Struct(def_id) => {
                let type_decl = *self
                    .types_by_id
                    .get(def_id)
                    .expect("struct DefId not indexed");
                if let Some(f) = type_decl
                    .fields
                    .iter()
                    .find(|f| f.name.name == field.name)
                {
                    self.type_ref_to_type(&f.ty)
                } else {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::UnknownField {
                            struct_name: type_decl.name.name.clone(),
                            field: field.name.clone(),
                        },
                        span,
                    ));
                    Type::Unknown
                }
            }
            Type::Unknown => Type::Unknown,
            other => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::NotAStruct {
                        got: other.display_name(),
                    },
                    target.span(),
                ));
                Type::Unknown
            }
        }
    }
}
