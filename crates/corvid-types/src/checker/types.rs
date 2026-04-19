//! TypeRef -> Type resolution.
//!
//! Given an AST type reference (what the user wrote), produce the
//! structural `Type` the rest of the checker works with. Handles
//! primitives, user-declared structs, and the compiler-known
//! generics (`List`, `Stream`, `Option`, `Result`, `Grounded`,
//! `Weak`), plus arity validation on each generic.
//!
//! Extracted from `checker.rs` as part of Phase 20i responsibility
//! decomposition.

use super::{is_weakable_type, Checker};
use crate::errors::{TypeError, TypeErrorKind};
use crate::types::Type;
use corvid_ast::{TypeRef, WeakEffectRow};

impl<'a> Checker<'a> {
    // ------------------------------------------------------------
    // Type-reference resolution (TypeRef → Type).
    // ------------------------------------------------------------

    pub(super) fn type_ref_to_type(&mut self, tr: &TypeRef) -> Type {
        match tr {
            TypeRef::Named { name, .. } => self.named_type_to_type(&name.name),
            TypeRef::Generic { name, args, span } => match name.name.as_str() {
                "List" => {
                    if args.len() != 1 {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::GenericArityMismatch {
                                name: name.name.clone(),
                                expected: 1,
                                got: args.len(),
                            },
                            *span,
                        ));
                        return Type::Unknown;
                    }
                    Type::List(Box::new(self.type_ref_to_type(&args[0])))
                }
                "Stream" => {
                    if args.len() != 1 {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::GenericArityMismatch {
                                name: name.name.clone(),
                                expected: 1,
                                got: args.len(),
                            },
                            *span,
                        ));
                        return Type::Unknown;
                    }
                    Type::Stream(Box::new(self.type_ref_to_type(&args[0])))
                }
                "Option" => {
                    if args.len() != 1 {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::GenericArityMismatch {
                                name: name.name.clone(),
                                expected: 1,
                                got: args.len(),
                            },
                            *span,
                        ));
                        return Type::Unknown;
                    }
                    Type::Option(Box::new(self.type_ref_to_type(&args[0])))
                }
                "Result" => {
                    if args.len() != 2 {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::GenericArityMismatch {
                                name: name.name.clone(),
                                expected: 2,
                                got: args.len(),
                            },
                            *span,
                        ));
                        return Type::Unknown;
                    }
                    Type::Result(
                        Box::new(self.type_ref_to_type(&args[0])),
                        Box::new(self.type_ref_to_type(&args[1])),
                    )
                }
                "Grounded" => {
                    if args.len() != 1 {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::GenericArityMismatch {
                                name: name.name.clone(),
                                expected: 1,
                                got: args.len(),
                            },
                            *span,
                        ));
                        return Type::Unknown;
                    }
                    Type::Grounded(Box::new(self.type_ref_to_type(&args[0])))
                }
                _ => Type::Unknown,
            },
            TypeRef::Weak { inner, effects, span } => {
                let inner_ty = self.type_ref_to_type(inner);
                if !is_weakable_type(&inner_ty) && !matches!(inner_ty, Type::Unknown) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::InvalidWeakTargetType {
                            got: inner_ty.display_name(),
                        },
                        *span,
                    ));
                    return Type::Weak(Box::new(Type::Unknown), effects.unwrap_or_else(WeakEffectRow::any));
                }
                Type::Weak(
                    Box::new(inner_ty),
                    effects.unwrap_or_else(WeakEffectRow::any),
                )
            }
            TypeRef::Function { .. } => Type::Unknown,
        }
    }

    pub(super) fn named_type_to_type(&self, name: &str) -> Type {
        match name {
            "Int" => Type::Int,
            "Float" => Type::Float,
            "String" => Type::String,
            "Bool" => Type::Bool,
            "Nothing" => Type::Nothing,
            _ => match self.symbols.lookup_def(name) {
                Some(id) => Type::Struct(id),
                None => Type::Unknown,
            },
        }
    }
}
