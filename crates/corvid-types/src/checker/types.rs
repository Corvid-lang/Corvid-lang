//! TypeRef -> Type resolution.
//!
//! Given an AST type reference (what the user wrote), produce the
//! structural `Type` the rest of the checker works with. Handles
//! primitives, user-declared structs, imported structs, and the
//! compiler-known generics (`List`, `Stream`, `Option`, `Result`,
//! `Grounded`, `Partial`, `ResumeToken`, `Weak`), plus arity validation on each generic.

use super::{is_weakable_type, Checker};
use crate::errors::{TypeError, TypeErrorKind};
use crate::types::{ImportedStructType, Type};
use corvid_ast::{TypeRef, WeakEffectRow};

impl<'a> Checker<'a> {
    pub(super) fn type_ref_to_type(&mut self, tr: &TypeRef) -> Type {
        match tr {
            TypeRef::Named { name, .. } => self.named_type_to_type(&name.name, name.span),
            TypeRef::Qualified { alias, name, span } => {
                self.qualified_type_ref_to_type(&alias.name, &name.name, *span)
            }
            TypeRef::Generic { name, args, span } => {
                self.generic_type_ref_to_type(&name.name, args, *span, TypeContext::Root)
            }
            TypeRef::Weak {
                inner,
                effects,
                span,
            } => {
                let inner_ty = self.type_ref_to_type(inner);
                if !is_weakable_type(&inner_ty) && !matches!(inner_ty, Type::Unknown) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::InvalidWeakTargetType {
                            got: inner_ty.display_name(),
                        },
                        *span,
                    ));
                    return Type::Weak(
                        Box::new(Type::Unknown),
                        effects.unwrap_or_else(WeakEffectRow::any),
                    );
                }
                Type::Weak(
                    Box::new(inner_ty),
                    effects.unwrap_or_else(WeakEffectRow::any),
                )
            }
            TypeRef::Function { .. } => Type::Unknown,
        }
    }

    pub(super) fn imported_type_ref_to_type(
        &mut self,
        tr: &TypeRef,
        module: &corvid_resolve::ResolvedModule,
    ) -> Type {
        match tr {
            TypeRef::Named { name, .. } => self.named_type_in_module(&name.name, module),
            TypeRef::Qualified { alias, name, span } => {
                let Some(modules) = self.module_resolution else {
                    return Type::Unknown;
                };
                let Some(target_module) = imported_module_alias_target(module, modules, &alias.name)
                else {
                    return Type::Unknown;
                };
                match target_module.exports.get(&name.name) {
                    Some(export) if matches!(export.kind, corvid_resolve::DeclKind::Type) => {
                        imported_struct_type(target_module, export.def_id, &export.name)
                    }
                    Some(_) => {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::TypeAsValue {
                                name: format!("{}.{}", alias.name, name.name),
                            },
                            *span,
                        ));
                        Type::Unknown
                    }
                    None => Type::Unknown,
                }
            }
            TypeRef::Generic { name, args, span } => self.generic_type_ref_to_type(
                &name.name,
                args,
                *span,
                TypeContext::Imported(module),
            ),
            TypeRef::Weak {
                inner, effects, ..
            } => Type::Weak(
                Box::new(self.imported_type_ref_to_type(inner, module)),
                effects.unwrap_or_else(WeakEffectRow::any),
            ),
            TypeRef::Function { .. } => Type::Unknown,
        }
    }

    pub(super) fn named_type_to_type(&mut self, name: &str, span: corvid_ast::Span) -> Type {
        match name {
            "Int" => Type::Int,
            "Float" => Type::Float,
            "String" => Type::String,
            "Bool" => Type::Bool,
            "Nothing" => Type::Nothing,
            _ => match self.symbols.lookup_def(name) {
                Some(id) => {
                    let entry = self.symbols.get(id);
                    if entry.kind == corvid_resolve::DeclKind::ImportedUse {
                        return self.imported_use_type_to_type(name, span);
                    }
                    Type::Struct(id)
                }
                None => Type::Unknown,
            },
        }
    }

    fn imported_use_type_to_type(&mut self, name: &str, span: corvid_ast::Span) -> Type {
        let Some(modules) = self.module_resolution else {
            return Type::Unknown;
        };
        let Some(target) = modules.lookup_imported_use(name) else {
            self.errors.push(TypeError::new(
                TypeErrorKind::UnknownImportMember {
                    alias: "<import use>".to_string(),
                    name: name.to_string(),
                },
                span,
            ));
            return Type::Unknown;
        };
        let Some(module) = modules.lookup_by_path(&target.module_path) else {
            return Type::Unknown;
        };
        if !matches!(target.export.kind, corvid_resolve::DeclKind::Type) {
            self.errors.push(TypeError::new(
                TypeErrorKind::TypeAsValue {
                    name: name.to_string(),
                },
                span,
            ));
            return Type::Unknown;
        }
        imported_struct_type(module, target.export.def_id, &target.export.name)
    }

    fn qualified_type_ref_to_type(&mut self, alias: &str, name: &str, span: corvid_ast::Span) -> Type {
        let Some(modules) = self.module_resolution else {
            self.errors.push(TypeError::new(
                TypeErrorKind::CorvidImportNotYetResolved {
                    alias: alias.to_string(),
                    name: name.to_string(),
                },
                span,
            ));
            return Type::Unknown;
        };

        match modules.lookup_member(alias, name) {
            corvid_resolve::ModuleLookup::Found { module, export } => {
                if !matches!(export.kind, corvid_resolve::DeclKind::Type) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeAsValue {
                            name: format!("{alias}.{name}"),
                        },
                        span,
                    ));
                    return Type::Unknown;
                }
                imported_struct_type(module, export.def_id, &export.name)
            }
            corvid_resolve::ModuleLookup::UnknownAlias => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::UnknownImportAlias {
                        alias: alias.to_string(),
                    },
                    span,
                ));
                Type::Unknown
            }
            corvid_resolve::ModuleLookup::Private => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::ImportedDeclIsPrivate {
                        alias: alias.to_string(),
                        name: name.to_string(),
                    },
                    span,
                ));
                Type::Unknown
            }
            corvid_resolve::ModuleLookup::UnknownMember => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::UnknownImportMember {
                        alias: alias.to_string(),
                        name: name.to_string(),
                    },
                    span,
                ));
                Type::Unknown
            }
        }
    }

    fn named_type_in_module(
        &self,
        name: &str,
        module: &corvid_resolve::ResolvedModule,
    ) -> Type {
        match name {
            "Int" => Type::Int,
            "Float" => Type::Float,
            "String" => Type::String,
            "Bool" => Type::Bool,
            "Nothing" => Type::Nothing,
            _ => match module.resolved.symbols.lookup_def(name) {
                Some(def_id)
                    if matches!(
                        module.resolved.symbols.get(def_id).kind,
                        corvid_resolve::DeclKind::Type
                    ) =>
                {
                    imported_struct_type(module, def_id, name)
                }
                _ => Type::Unknown,
            },
        }
    }

    fn generic_type_ref_to_type(
        &mut self,
        name: &str,
        args: &[TypeRef],
        span: corvid_ast::Span,
        context: TypeContext<'_>,
    ) -> Type {
        let resolve_arg = |checker: &mut Self, arg: &TypeRef| match context {
            TypeContext::Root => checker.type_ref_to_type(arg),
            TypeContext::Imported(module) => checker.imported_type_ref_to_type(arg, module),
        };

        match name {
            "List" | "Stream" | "Option" | "Grounded" | "Partial" | "ResumeToken" => {
                if args.len() != 1 {
                    if matches!(context, TypeContext::Root) {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::GenericArityMismatch {
                                name: name.to_string(),
                                expected: 1,
                                got: args.len(),
                            },
                            span,
                        ));
                    }
                    return Type::Unknown;
                }
                let inner = Box::new(resolve_arg(self, &args[0]));
                match name {
                    "List" => Type::List(inner),
                    "Stream" => Type::Stream(inner),
                    "Option" => Type::Option(inner),
                    "Grounded" => Type::Grounded(inner),
                    "Partial" => Type::Partial(inner),
                    "ResumeToken" => Type::ResumeToken(inner),
                    _ => unreachable!(),
                }
            }
            "Result" => {
                if args.len() != 2 {
                    if matches!(context, TypeContext::Root) {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::GenericArityMismatch {
                                name: name.to_string(),
                                expected: 2,
                                got: args.len(),
                            },
                            span,
                        ));
                    }
                    return Type::Unknown;
                }
                Type::Result(
                    Box::new(resolve_arg(self, &args[0])),
                    Box::new(resolve_arg(self, &args[1])),
                )
            }
            _ => Type::Unknown,
        }
    }
}

#[derive(Clone, Copy)]
enum TypeContext<'a> {
    Root,
    Imported(&'a corvid_resolve::ResolvedModule),
}

fn imported_struct_type(
    module: &corvid_resolve::ResolvedModule,
    def_id: corvid_resolve::DefId,
    name: &str,
) -> Type {
    Type::ImportedStruct(ImportedStructType {
        module_path: module.path.to_string_lossy().into_owned(),
        def_id,
        name: name.to_string(),
    })
}

fn imported_module_alias_target<'a>(
    module: &corvid_resolve::ResolvedModule,
    modules: &'a corvid_resolve::ModuleResolution,
    alias: &str,
) -> Option<&'a corvid_resolve::ResolvedModule> {
    let import = module.file.decls.iter().find_map(|decl| match decl {
        corvid_ast::Decl::Import(import)
            if matches!(import.source, corvid_ast::ImportSource::Corvid)
                && import.alias.as_ref().is_some_and(|a| a.name == alias) =>
        {
            Some(import)
        }
        _ => None,
    })?;
    let child = corvid_resolve::resolve_import_path(&module.path, &import.module);
    modules.lookup_by_path(&child)
}
