//! Qualified imported call checking.
//!
//! `call.rs` owns ordinary call-shape dispatch. This module owns the
//! import-specific branch for `alias.member(args)`: validating that
//! `alias` is a Corvid import, resolving the public export, checking
//! arguments in the imported module's type context, and recording the
//! imported target for IR lowering.

use super::{pascal_case, snake_case, Checker, ImportedCallKind, ImportedCallTarget};
use crate::errors::{TypeError, TypeErrorKind, TypeWarning, TypeWarningKind};
use crate::effects::{ComposedProfile, EffectRegistry};
use crate::types::{ImportedStructType, Type};
use corvid_ast::{AgentAttribute, Decl, Effect, Expr, Ident, Param, Span, WeakEffect};
use corvid_resolve::{Binding, DeclKind, DefId, ModuleLookup, ResolvedModule};
use std::collections::HashMap;

impl<'a> Checker<'a> {
    pub(super) fn validate_python_import_effects(&mut self, file: &corvid_ast::File) {
        for decl in &file.decls {
            let Decl::Import(import) = decl else { continue };
            if !matches!(import.source, corvid_ast::ImportSource::Python) {
                continue;
            }
            if import.effect_row.effects.is_empty() {
                self.errors.push(TypeError::with_guarantee(
                    TypeErrorKind::EffectConstraintViolation {
                        agent: format!("python import `{}`", import.module),
                        dimension: "effects".to_string(),
                        message: "Python imports must declare their host capabilities with `effects: ...`; use `effects: unsafe` only as an explicit review escape hatch".to_string(),
                    },
                    import.span,
                    "effect_row.import_boundary",
                ));
                continue;
            }
            if import
                .effect_row
                .effects
                .iter()
                .any(|effect| effect.name.name == "unsafe")
            {
                self.warnings.push(TypeWarning::new(
                    TypeWarningKind::UnsafePythonImport {
                        module: import.module.clone(),
                        message: "dynamic Python code can access capabilities the compiler cannot inspect".to_string(),
                    },
                    import.effect_row.span,
                ));
            }
        }
    }

    pub(super) fn validate_import_use_items(&mut self, file: &corvid_ast::File) {
        let Some(modules) = self.module_resolution else {
            return;
        };
        for decl in &file.decls {
            let Decl::Import(import) = decl else { continue };
            if !matches!(
                import.source,
                corvid_ast::ImportSource::Corvid
                    | corvid_ast::ImportSource::RemoteCorvid
                    | corvid_ast::ImportSource::PackageCorvid
            ) {
                continue;
            }
            if let Some(module) = modules.lookup_root_import(&import.module) {
                self.validate_import_requirements(import, module);
            }
            let module_label = import
                .alias
                .as_ref()
                .map(|alias| alias.name.clone())
                .unwrap_or_else(|| import.module.clone());
            for item in &import.use_items {
                let lifted = item
                    .alias
                    .as_ref()
                    .map(|alias| alias.name.clone())
                    .unwrap_or_else(|| item.name.name.clone());
                if modules.lookup_imported_use(&lifted).is_none() {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::UnknownImportMember {
                            alias: module_label.clone(),
                            name: item.name.name.clone(),
                        },
                        item.span,
                    ));
                }
            }
        }
    }

    fn validate_import_requirements(
        &mut self,
        import: &corvid_ast::ImportDecl,
        module: &ResolvedModule,
    ) {
        for attr in &import.required_attributes {
            match attr {
                AgentAttribute::Deterministic { .. } => {
                    self.validate_import_requires_deterministic(import, module);
                }
                AgentAttribute::Replayable { .. } => {
                    self.validate_import_requires_replayable(import, module);
                }
                AgentAttribute::Wrapping { .. } => {
                    self.errors.push(TypeError::with_guarantee(
                        TypeErrorKind::EffectConstraintViolation {
                            agent: format!("import `{}`", import.module),
                            dimension: "wrapping".to_string(),
                            message: "`@wrapping` is an agent execution mode and cannot be required at an import boundary".to_string(),
                        },
                        import.span,
                        "effect_row.import_boundary",
                    ));
                }
            }
        }
        if import.required_constraints.is_empty() {
            return;
        }
        let effect_decls = module
            .file
            .decls
            .iter()
            .filter_map(|decl| match decl {
                Decl::Effect(effect) => Some(effect.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let registry = EffectRegistry::from_decls(&effect_decls);
        for summary in module.semantic_summary.agents.values() {
            let profile = ComposedProfile {
                dimensions: summary
                    .composed_dimensions
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect::<HashMap<_, _>>(),
                effect_names: Vec::new(),
            };
            for violation in
                registry.check_constraints(&profile, &import.required_constraints)
            {
                let message = violation.to_string();
                self.errors.push(TypeError::with_guarantee(
                    TypeErrorKind::EffectConstraintViolation {
                        agent: format!(
                            "import `{}` exported agent `{}`",
                            import.module, summary.name
                        ),
                        dimension: violation.dimension,
                        message,
                    },
                    import.span,
                    "effect_row.import_boundary",
                ));
            }
        }
    }

    fn validate_import_requires_deterministic(
        &mut self,
        import: &corvid_ast::ImportDecl,
        module: &ResolvedModule,
    ) {
        for export in module.exports.values() {
            match export.kind {
                DeclKind::Tool | DeclKind::Prompt => {
                    self.errors.push(TypeError::with_guarantee(
                        TypeErrorKind::EffectConstraintViolation {
                            agent: format!("import `{}`", import.module),
                            dimension: "deterministic".to_string(),
                            message: format!(
                                "export `{}` is a {}, which is not deterministic at a module boundary",
                                export.name,
                                match export.kind {
                                    DeclKind::Tool => "tool",
                                    DeclKind::Prompt => "prompt",
                                    _ => "declaration",
                                }
                            ),
                        },
                        import.span,
                        "effect_row.import_boundary",
                    ));
                }
                DeclKind::Agent => {
                    let deterministic = module
                        .semantic_summary
                        .agents
                        .get(&export.name)
                        .is_some_and(|summary| summary.deterministic);
                    if !deterministic {
                        self.errors.push(TypeError::with_guarantee(
                            TypeErrorKind::EffectConstraintViolation {
                                agent: format!(
                                    "import `{}` exported agent `{}`",
                                    import.module, export.name
                                ),
                                dimension: "deterministic".to_string(),
                                message: "exported agent is not marked `@deterministic`".to_string(),
                            },
                            import.span,
                            "effect_row.import_boundary",
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    fn validate_import_requires_replayable(
        &mut self,
        import: &corvid_ast::ImportDecl,
        module: &ResolvedModule,
    ) {
        for export in module.exports.values() {
            if export.kind != DeclKind::Agent {
                continue;
            }
            let replayable = module
                .semantic_summary
                .agents
                .get(&export.name)
                .is_some_and(|summary| summary.replayable);
            if !replayable {
                self.errors.push(TypeError::with_guarantee(
                    TypeErrorKind::EffectConstraintViolation {
                        agent: format!(
                            "import `{}` exported agent `{}`",
                            import.module, export.name
                        ),
                        dimension: "replayable".to_string(),
                        message: "exported agent is not marked `@replayable` or `@deterministic`"
                            .to_string(),
                    },
                    import.span,
                    "effect_row.import_boundary",
                ));
            }
        }
    }

    pub(super) fn check_imported_use_call(
        &mut self,
        def_id: DefId,
        name: &str,
        args: &[Expr],
        callee_span: Span,
        call_span: Span,
    ) -> Type {
        let Some(modules) = self.module_resolution else {
            for arg in args {
                let _ = self.check_expr(arg);
            }
            return Type::Unknown;
        };
        let Some(target) = modules.lookup_imported_use(name) else {
            for arg in args {
                let _ = self.check_expr(arg);
            }
            self.errors.push(TypeError::new(
                TypeErrorKind::UnknownImportMember {
                    alias: "<import use>".to_string(),
                    name: name.to_string(),
                },
                callee_span,
            ));
            return Type::Unknown;
        };
        let Some(module) = modules.lookup_by_path(&target.module_path) else {
            for arg in args {
                let _ = self.check_expr(arg);
            }
            return Type::Unknown;
        };
        let kind = match target.export.kind {
            DeclKind::Type => ImportedCallKind::Type,
            DeclKind::Tool => ImportedCallKind::Tool,
            DeclKind::Prompt => ImportedCallKind::Prompt,
            DeclKind::Agent => ImportedCallKind::Agent,
            _ => {
                for arg in args {
                    let _ = self.check_expr(arg);
                }
                return Type::Unknown;
            }
        };
        self.imported_calls.insert(
            callee_span,
            ImportedCallTarget {
                module_path: target.module_path.to_string_lossy().into_owned(),
                def_id: target.export.def_id,
                name: target.export.name.clone(),
                kind,
            },
        );
        let _ = def_id;
        self.check_imported_decl_call(
            module,
            target.export.def_id,
            target.export.kind,
            &target.export.name,
            args,
            call_span,
        )
    }

    pub(super) fn check_imported_call(
        &mut self,
        target: &Expr,
        field: &Ident,
        args: &[Expr],
        callee_span: Span,
        call_span: Span,
    ) -> Option<Type> {
        let Expr::Ident { name: alias, .. } = target else {
            return None;
        };
        let Some(Binding::Decl(alias_def)) = self.bindings.get(&alias.span) else {
            return None;
        };
        if !matches!(self.symbols.get(*alias_def).kind, DeclKind::Import) {
            return None;
        }

        let Some(modules) = self.module_resolution else {
            self.errors.push(TypeError::new(
                TypeErrorKind::CorvidImportNotYetResolved {
                    alias: alias.name.clone(),
                    name: field.name.clone(),
                },
                callee_span,
            ));
            for arg in args {
                let _ = self.check_expr(arg);
            }
            return Some(Type::Unknown);
        };

        match modules.lookup_member(&alias.name, &field.name) {
            ModuleLookup::Found { module, export } => {
                let target = ImportedCallTarget {
                    module_path: module.path.to_string_lossy().into_owned(),
                    def_id: export.def_id,
                    name: export.name.clone(),
                    kind: match export.kind {
                        DeclKind::Type => ImportedCallKind::Type,
                        DeclKind::Tool => ImportedCallKind::Tool,
                        DeclKind::Prompt => ImportedCallKind::Prompt,
                        DeclKind::Agent => ImportedCallKind::Agent,
                        _ => {
                            for arg in args {
                                let _ = self.check_expr(arg);
                            }
                            return Some(Type::Unknown);
                        }
                    },
                };
                self.imported_calls.insert(callee_span, target);
                Some(self.check_imported_decl_call(
                    module,
                    export.def_id,
                    export.kind,
                    &export.name,
                    args,
                    call_span,
                ))
            }
            ModuleLookup::UnknownAlias => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::UnknownImportAlias {
                        alias: alias.name.clone(),
                    },
                    callee_span,
                ));
                for arg in args {
                    let _ = self.check_expr(arg);
                }
                Some(Type::Unknown)
            }
            ModuleLookup::Private => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::ImportedDeclIsPrivate {
                        alias: alias.name.clone(),
                        name: field.name.clone(),
                    },
                    callee_span,
                ));
                for arg in args {
                    let _ = self.check_expr(arg);
                }
                Some(Type::Unknown)
            }
            ModuleLookup::UnknownMember => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::UnknownImportMember {
                        alias: alias.name.clone(),
                        name: field.name.clone(),
                    },
                    callee_span,
                ));
                for arg in args {
                    let _ = self.check_expr(arg);
                }
                Some(Type::Unknown)
            }
        }
    }

    fn check_imported_decl_call(
        &mut self,
        module: &ResolvedModule,
        def_id: DefId,
        kind: DeclKind,
        name: &str,
        args: &[Expr],
        span: Span,
    ) -> Type {
        match kind {
            DeclKind::Tool => {
                let Some(Decl::Tool(tool)) = imported_decl(module, def_id) else {
                    for arg in args {
                        let _ = self.check_expr(arg);
                    }
                    return Type::Unknown;
                };
                self.check_args_against_imported_params(name, &tool.params, args, module);
                if matches!(tool.effect, Effect::Dangerous) {
                    let authorized = self
                        .approvals
                        .iter()
                        .any(|a| snake_case(&a.label) == name && a.arity == args.len());
                    if !authorized {
                        self.errors.push(TypeError::with_guarantee(
                            TypeErrorKind::UnapprovedDangerousCall {
                                tool: name.to_string(),
                                expected_approve_label: pascal_case(name),
                                arity: args.len(),
                            },
                            span,
                            "approval.dangerous_call_requires_token",
                        ));
                    }
                }
                self.bump_effect(WeakEffect::ToolCall);
                self.imported_type_ref_to_type(&tool.return_ty, module)
            }
            DeclKind::Prompt => {
                let Some(Decl::Prompt(prompt)) = imported_decl(module, def_id) else {
                    for arg in args {
                        let _ = self.check_expr(arg);
                    }
                    return Type::Unknown;
                };
                self.check_args_against_imported_params(name, &prompt.params, args, module);
                self.bump_effect(WeakEffect::Llm);
                self.imported_type_ref_to_type(&prompt.return_ty, module)
            }
            DeclKind::Agent => {
                let Some(Decl::Agent(agent)) = imported_decl(module, def_id) else {
                    for arg in args {
                        let _ = self.check_expr(arg);
                    }
                    return Type::Unknown;
                };
                self.check_args_against_imported_params(name, &agent.params, args, module);
                self.bump_effect(WeakEffect::ToolCall);
                self.bump_effect(WeakEffect::Llm);
                self.bump_effect(WeakEffect::Approve);
                self.imported_type_ref_to_type(&agent.return_ty, module)
            }
            DeclKind::Type => {
                let Some(Decl::Type(ty)) = imported_decl(module, def_id) else {
                    for arg in args {
                        let _ = self.check_expr(arg);
                    }
                    return Type::Unknown;
                };
                if args.len() != ty.fields.len() {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::ArityMismatch {
                            callee: name.to_string(),
                            expected: ty.fields.len(),
                            got: args.len(),
                        },
                        args.first().map(|arg| arg.span()).unwrap_or(ty.span),
                    ));
                }
                for (i, arg) in args.iter().enumerate() {
                    if let Some(field) = ty.fields.get(i) {
                        let field_ty = self.imported_type_ref_to_type(&field.ty, module);
                        let arg_ty = self.check_expr_as(arg, Some(&field_ty));
                        if !arg_ty.is_assignable_to(&field_ty) {
                            self.errors.push(TypeError::new(
                                TypeErrorKind::TypeMismatch {
                                    expected: field_ty.display_name(),
                                    got: arg_ty.display_name(),
                                    context: format!("field `{}` of `{name}`", field.name.name),
                                },
                                arg.span(),
                            ));
                        }
                    } else {
                        let _ = self.check_expr(arg);
                    }
                }
                Type::ImportedStruct(ImportedStructType {
                    module_path: module.path.to_string_lossy().into_owned(),
                    def_id,
                    name: name.to_string(),
                })
            }
            _ => {
                for arg in args {
                    let _ = self.check_expr(arg);
                }
                Type::Unknown
            }
        }
    }

    fn check_args_against_imported_params(
        &mut self,
        callee_name: &str,
        params: &[Param],
        args: &[Expr],
        module: &ResolvedModule,
    ) {
        if params.len() != args.len() {
            self.errors.push(TypeError::new(
                TypeErrorKind::ArityMismatch {
                    callee: callee_name.to_string(),
                    expected: params.len(),
                    got: args.len(),
                },
                args.first()
                    .map(|arg| arg.span())
                    .unwrap_or(Span::new(0, 0)),
            ));
        }
        for (i, arg) in args.iter().enumerate() {
            if let Some(param) = params.get(i) {
                let param_ty = self.imported_type_ref_to_type(&param.ty, module);
                let arg_ty = self.check_expr_as(arg, Some(&param_ty));
                if !arg_ty.is_assignable_to(&param_ty) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: param_ty.display_name(),
                            got: arg_ty.display_name(),
                            context: format!("argument {} to `{callee_name}`", i + 1),
                        },
                        arg.span(),
                    ));
                }
            } else {
                let _ = self.check_expr(arg);
            }
        }
    }
}

fn imported_decl<'a>(module: &'a ResolvedModule, def_id: DefId) -> Option<&'a Decl> {
    module.file.decls.iter().find(|decl| {
        let name = match decl {
            Decl::Type(decl) => &decl.name.name,
            Decl::Tool(decl) => &decl.name.name,
            Decl::Prompt(decl) => &decl.name.name,
            Decl::Agent(decl) => &decl.name.name,
            _ => return false,
        };
        module
            .resolved
            .symbols
            .lookup_def(name)
            .is_some_and(|candidate| candidate == def_id)
    })
}
