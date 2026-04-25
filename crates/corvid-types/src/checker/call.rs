//! Call-shape checks: tool, prompt, agent, struct constructor,
//! built-in constructor (`Weak::new`, `Weak::upgrade`, `Stream::…`,
//! `List::…`), and method-call dispatch.
//!
//! Also hosts `check_args_against_params`, the shared arity +
//! type-compatibility validator used by every typed-callable
//! check.
//!
//! Extracted from `checker.rs` as part of Phase 20i responsibility
//! decomposition.

use super::{is_weakable_type, pascal_case, snake_case, Checker, EffectFrontier};
use crate::errors::{TypeError, TypeErrorKind};
use crate::types::Type;
use corvid_ast::{Effect, Expr, Ident, Param, Span, WeakEffect, WeakEffectRow};
use corvid_resolve::{resolver::MethodKind, Binding, BuiltIn, DeclKind, DefId, LocalId};
use std::collections::HashMap;

impl<'a> Checker<'a> {
    pub(super) fn check_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        span: Span,
        expected: Option<&Type>,
    ) -> Type {
        // A callee of shape `target.field` is a method call.
        // Lower it: typecheck the receiver, look up the method by
        // (receiver_type_def_id, method_name), validate args (with
        // the receiver implicitly prepended), reuse the appropriate
        // tool / prompt / agent dispatch path.
        if let Expr::FieldAccess {
            target,
            field,
            span: callee_span,
        } = callee
        {
            if let Some(ty) = self.check_imported_call(target, field, args, *callee_span, span) {
                return ty;
            }
            return self.check_method_call(target, field, args, span);
        }

        // Identify what's being called by looking at the callee's binding.
        let Expr::Ident { name, .. } = callee else {
            // Indirect or chained callee — typecheck args and give up.
            for a in args {
                let _ = self.check_expr(a);
            }
            return Type::Unknown;
        };

        let Some(binding) = self.bindings.get(&name.span) else {
            // Unresolved callee (e.g. approve label encountered outside an
            // approve — shouldn't happen for well-formed code). Typecheck args.
            for a in args {
                let _ = self.check_expr(a);
            }
            return Type::Unknown;
        };

        match binding {
            Binding::Decl(def_id) => {
                let def_id = *def_id;
                let entry = self.symbols.get(def_id);
                match entry.kind {
                    DeclKind::Tool => self.check_tool_call(def_id, &name.name, args, span),
                    DeclKind::Prompt => self.check_prompt_call(def_id, &name.name, args),
                    DeclKind::Agent => self.check_agent_call(def_id, &name.name, args),
                    DeclKind::Import
                    | DeclKind::Eval
                    | DeclKind::Effect
                    | DeclKind::Model => {
                        for a in args {
                            let _ = self.check_expr(a);
                        }
                        Type::Unknown
                    }
                    DeclKind::Type => self.check_struct_constructor(def_id, &name.name, args),
                }
            }
            Binding::BuiltIn(builtin) => {
                self.check_builtin_constructor_call(*builtin, name, args, expected)
            }
            Binding::Local(_) => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::NotCallable {
                        got: "<local value>".into(),
                    },
                    callee.span(),
                ));
                for a in args {
                    let _ = self.check_expr(a);
                }
                Type::Unknown
            }
        }
    }

    fn check_tool_call(
        &mut self,
        def_id: DefId,
        tool_name: &str,
        args: &[Expr],
        span: Span,
    ) -> Type {
        let tool = *self
            .tools_by_id
            .get(&def_id)
            .expect("tool DefId not indexed");

        self.check_args_against_params(tool_name, &tool.params, args);

        // Effect check: dangerous tool must have a prior matching approve.
        if matches!(tool.effect, Effect::Dangerous) {
            let authorized = self
                .approvals
                .iter()
                .any(|a| snake_case(&a.label) == tool_name && a.arity == args.len());
            if !authorized {
                self.errors.push(TypeError::new(
                    TypeErrorKind::UnapprovedDangerousCall {
                        tool: tool_name.to_string(),
                        expected_approve_label: pascal_case(tool_name),
                        arity: args.len(),
                    },
                    span,
                ));
            }
        }

        self.bump_effect(WeakEffect::ToolCall);
        self.type_ref_to_type(&tool.return_ty)
    }

    fn check_prompt_call(
        &mut self,
        def_id: DefId,
        name: &str,
        args: &[Expr],
    ) -> Type {
        let prompt = *self
            .prompts_by_id
            .get(&def_id)
            .expect("prompt DefId not indexed");
        self.check_args_against_params(name, &prompt.params, args);
        self.bump_effect(WeakEffect::Llm);
        self.type_ref_to_type(&prompt.return_ty)
    }

    fn check_agent_call(
        &mut self,
        def_id: DefId,
        name: &str,
        args: &[Expr],
    ) -> Type {
        let agent = *self
            .agents_by_id
            .get(&def_id)
            .expect("agent DefId not indexed");
        self.check_args_against_params(name, &agent.params, args);
        self.bump_effect(WeakEffect::ToolCall);
        self.bump_effect(WeakEffect::Llm);
        self.bump_effect(WeakEffect::Approve);
        self.type_ref_to_type(&agent.return_ty)
    }

    fn check_builtin_constructor_call(
        &mut self,
        builtin: BuiltIn,
        name: &Ident,
        args: &[Expr],
        expected: Option<&Type>,
    ) -> Type {
        match builtin {
            BuiltIn::Ok => {
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
                    return Type::Result(Box::new(Type::Unknown), Box::new(Type::Unknown));
                }
                let ok_ty = self.check_expr(&args[0]);
                let err_ty = match &self.current_return {
                    Some(Type::Result(_, err)) => (**err).clone(),
                    _ => Type::Unknown,
                };
                Type::Result(Box::new(ok_ty), Box::new(err_ty))
            }
            BuiltIn::Err => {
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
                    return Type::Result(Box::new(Type::Unknown), Box::new(Type::Unknown));
                }
                let err_ty = self.check_expr(&args[0]);
                let ok_ty = match &self.current_return {
                    Some(Type::Result(ok, _)) => (**ok).clone(),
                    _ => Type::Unknown,
                };
                Type::Result(Box::new(ok_ty), Box::new(err_ty))
            }
            BuiltIn::Some => {
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
                    return Type::Option(Box::new(Type::Unknown));
                }
                let expected_inner = match expected {
                    Some(Type::Option(inner)) => Some(&**inner),
                    _ => None,
                };
                let inner_ty = self.check_expr_as(&args[0], expected_inner);
                let final_inner_ty = match expected_inner {
                    Some(exp) if inner_ty.is_assignable_to(exp) => exp.clone(),
                    _ => inner_ty,
                };
                Type::Option(Box::new(final_inner_ty))
            }
            BuiltIn::WeakNew => {
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
                    return Type::Weak(Box::new(Type::Unknown), WeakEffectRow::any());
                }
                let target_ty = self.check_expr(&args[0]);
                if !is_weakable_type(&target_ty) && !matches!(target_ty, Type::Unknown) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::InvalidWeakNewTarget {
                            got: target_ty.display_name(),
                        },
                        args[0].span(),
                    ));
                }
                let row = match expected {
                    Some(Type::Weak(_, row)) => *row,
                    _ => WeakEffectRow::any(),
                };
                Type::Weak(Box::new(target_ty), row)
            }
            BuiltIn::WeakUpgrade => {
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
                    return Type::Option(Box::new(Type::Unknown));
                }
                let weak_ty = self.check_expr(&args[0]);
                let refreshed_at = self.refresh_frontier_for_expr(&args[0], &weak_ty);
                match weak_ty {
                    Type::Weak(inner, row) => {
                        let invalidating = self
                            .effect_frontier
                            .invalidating_effects_since(&refreshed_at, row);
                        if !invalidating.is_empty() {
                            self.errors.push(TypeError::new(
                                TypeErrorKind::WeakUpgradeAcrossEffects {
                                    effects: invalidating,
                                },
                                args[0].span(),
                            ));
                        } else {
                            self.refresh_after_upgrade(&args[0]);
                        }
                        Type::Option(inner)
                    }
                    Type::Unknown => Type::Option(Box::new(Type::Unknown)),
                    other => {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::InvalidWeakUpgradeTarget {
                                got: other.display_name(),
                            },
                            args[0].span(),
                        ));
                        Type::Option(Box::new(Type::Unknown))
                    }
                }
            }
            BuiltIn::StreamResumeToken => {
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
                    return Type::ResumeToken(Box::new(Type::Unknown));
                }
                match self.check_expr(&args[0]) {
                    Type::Stream(inner) => Type::ResumeToken(inner),
                    Type::Unknown => Type::ResumeToken(Box::new(Type::Unknown)),
                    other => {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::TypeMismatch {
                                expected: "Stream<T>".into(),
                                got: other.display_name(),
                                context: "resume_token argument".into(),
                            },
                            args[0].span(),
                        ));
                        Type::ResumeToken(Box::new(Type::Unknown))
                    }
                }
            }
            BuiltIn::StreamMerge => self.check_stream_merge_call(name, args),
            BuiltIn::Resume => self.check_resume_call(name, args),
            BuiltIn::None => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::NotCallable {
                        got: "Option".into(),
                    },
                    name.span,
                ));
                for arg in args {
                    let _ = self.check_expr(arg);
                }
                Type::Unknown
            }
            _ => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::NotCallable {
                        got: name.name.clone(),
                    },
                    name.span,
                ));
                for arg in args {
                    let _ = self.check_expr(arg);
                }
                Type::Unknown
            }
        }
    }

    fn check_resume_call(&mut self, name: &Ident, args: &[Expr]) -> Type {
        if args.len() != 2 {
            self.errors.push(TypeError::new(
                TypeErrorKind::ArityMismatch {
                    callee: name.name.clone(),
                    expected: 2,
                    got: args.len(),
                },
                name.span,
            ));
            for arg in args {
                let _ = self.check_expr(arg);
            }
            return Type::Stream(Box::new(Type::Unknown));
        }

        let prompt_ty = match &args[0] {
            Expr::Ident { name: prompt_name, .. } => match self.bindings.get(&prompt_name.span) {
                Some(Binding::Decl(def_id))
                    if self.symbols.get(*def_id).kind == DeclKind::Prompt =>
                {
                    let prompt = *self
                        .prompts_by_id
                        .get(def_id)
                        .expect("prompt DefId not indexed");
                    self.type_ref_to_type(&prompt.return_ty)
                }
                _ => {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: "prompt declaration".into(),
                            got: "non-prompt value".into(),
                            context: "first resume argument".into(),
                        },
                        args[0].span(),
                    ));
                    Type::Stream(Box::new(Type::Unknown))
                }
            },
            _ => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::TypeMismatch {
                        expected: "prompt declaration".into(),
                        got: "expression".into(),
                        context: "first resume argument".into(),
                    },
                    args[0].span(),
                ));
                Type::Stream(Box::new(Type::Unknown))
            }
        };

        let token_ty = self.check_expr(&args[1]);
        match (&prompt_ty, token_ty) {
            (Type::Stream(prompt_inner), Type::ResumeToken(token_inner)) => {
                if !token_inner.is_assignable_to(prompt_inner) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: format!("ResumeToken<{}>", prompt_inner.display_name()),
                            got: format!("ResumeToken<{}>", token_inner.display_name()),
                            context: "resume token".into(),
                        },
                        args[1].span(),
                    ));
                }
                Type::Stream(prompt_inner.clone())
            }
            (Type::Stream(prompt_inner), Type::Unknown) => Type::Stream(prompt_inner.clone()),
            (Type::Stream(_), other) => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::TypeMismatch {
                        expected: "ResumeToken<T>".into(),
                        got: other.display_name(),
                        context: "resume token".into(),
                    },
                    args[1].span(),
                ));
                prompt_ty
            }
            (other, _) => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::TypeMismatch {
                        expected: "streaming prompt".into(),
                        got: other.display_name(),
                        context: "first resume argument".into(),
                    },
                    args[0].span(),
                ));
                Type::Stream(Box::new(Type::Unknown))
            }
        }
    }

    /// `target.method(args)` rewritten to a regular
    /// function call with the receiver as the first argument. The
    /// receiver's type is looked up in the methods side-table to
    /// pick the matching method DefId; from there we reuse the
    /// existing tool / prompt / agent dispatch.
    ///
    /// Errors:
    ///   - receiver isn't a struct (no methods on built-ins yet).
    ///   - method name doesn't exist on the type.
    ///   - arity mismatch (argv vs declared params, accounting for
    ///     receiver-as-first-param).
    pub(super) fn check_method_call(
        &mut self,
        target: &Expr,
        method_name: &Ident,
        args: &[Expr],
        span: Span,
    ) -> Type {
        // 1. Typecheck the receiver and require a struct type.
        let recv_ty = self.check_expr(target);
        if method_name.name == "split_by" {
            return self.check_stream_split_by_method(target, &recv_ty, method_name, args);
        }
        if method_name.name == "ordered_by" {
            return self.check_stream_ordered_by_method(target, recv_ty, method_name, args);
        }
        if let Type::Grounded(inner) = &recv_ty {
            if method_name.name == "unwrap_discarding_sources" {
                if args.len() != 0 {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::ArityMismatch {
                            callee: method_name.name.clone(),
                            expected: 0,
                            got: args.len(),
                        },
                        span,
                    ));
                    for a in args {
                        let _ = self.check_expr(a);
                    }
                }
                return (**inner).clone();
            }
        }
        let recv_def_id = match recv_ty {
            Type::Struct(id) => id,
            other => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::NotCallable {
                        got: format!(
                            "method `{}` on receiver of type `{}` — methods currently work only on user-declared struct types. Built-in receiver methods are not implemented yet.",
                            method_name.name,
                            other.display_name()
                        ),
                    },
                    target.span(),
                ));
                // Still typecheck remaining args for diagnostics.
                for a in args {
                    let _ = self.check_expr(a);
                }
                return Type::Unknown;
            }
        };

        // 2. Look up the method.
        let method = match self
            .methods
            .get(&recv_def_id)
            .and_then(|m| m.get(&method_name.name))
        {
            Some(m) => m.clone(),
            None => {
                let type_name = self.symbols.get(recv_def_id).name.clone();
                self.errors.push(TypeError::new(
                    TypeErrorKind::NotCallable {
                        got: format!(
                            "no method `{}` on type `{type_name}`",
                            method_name.name
                        ),
                    },
                    method_name.span,
                ));
                for a in args {
                    let _ = self.check_expr(a);
                }
                return Type::Unknown;
            }
        };

        // 3. Build the effective argument list: receiver prepended.
        //    Then dispatch by method kind, reusing the existing
        //    free-call paths.
        let mut effective_args: Vec<Expr> = Vec::with_capacity(args.len() + 1);
        effective_args.push(target.clone());
        effective_args.extend_from_slice(args);

        match method.kind {
            MethodKind::Tool => self.check_tool_call(
                method.def_id,
                &method_name.name,
                &effective_args,
                span,
            ),
            MethodKind::Prompt => {
                self.check_prompt_call(method.def_id, &method_name.name, &effective_args)
            }
            MethodKind::Agent => {
                self.check_agent_call(method.def_id, &method_name.name, &effective_args)
            }
        }
    }

    /// `TypeName(field0, field1, ...)` — construct a struct. Field
    /// values must be assignable to each field's declared type.
    /// Returns `Struct(def_id)`.
    pub(super) fn check_struct_constructor(&mut self, def_id: DefId, name: &str, args: &[Expr]) -> Type {
        let ty_decl = *self
            .types_by_id
            .get(&def_id)
            .expect("type DefId not indexed");

        if args.len() != ty_decl.fields.len() {
            self.errors.push(TypeError::new(
                TypeErrorKind::ArityMismatch {
                    callee: name.to_string(),
                    expected: ty_decl.fields.len(),
                    got: args.len(),
                },
                args.first().map(|a| a.span()).unwrap_or(ty_decl.span),
            ));
        }
        for (i, arg) in args.iter().enumerate() {
            if let Some(field) = ty_decl.fields.get(i) {
                let field_ty = self.type_ref_to_type(&field.ty);
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
        Type::Struct(def_id)
    }

    fn check_args_against_params(
        &mut self,
        callee_name: &str,
        params: &[Param],
        args: &[Expr],
    ) {
        if params.len() != args.len() {
            self.errors.push(TypeError::new(
                TypeErrorKind::ArityMismatch {
                    callee: callee_name.to_string(),
                    expected: params.len(),
                    got: args.len(),
                },
                args.first()
                    .map(|a| a.span())
                    .unwrap_or(Span::new(0, 0)),
            ));
        }
        for (i, arg) in args.iter().enumerate() {
            if let Some(param) = params.get(i) {
                let param_ty = self.type_ref_to_type(&param.ty);
                let arg_ty = self.check_expr_as(arg, Some(&param_ty));
                if !arg_ty.is_assignable_to(&param_ty) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: param_ty.display_name(),
                            got: arg_ty.display_name(),
                            context: format!(
                                "argument {} to `{callee_name}`",
                                i + 1
                            ),
                        },
                        arg.span(),
                    ));
                }
            } else {
                let _ = self.check_expr(arg);
            }
        }
    }

    pub(super) fn bump_effect(&mut self, effect: WeakEffect) {
        self.effect_frontier = self.effect_frontier.bumped(effect);
    }

    pub(super) fn update_weak_local_on_assignment(
        &mut self,
        local_id: LocalId,
        value: &Expr,
        local_ty: &Type,
    ) {
        match local_ty {
            Type::Weak(_, _) => {
                let refreshed = self.refresh_frontier_for_expr(value, local_ty);
                self.weak_refresh.insert(local_id, refreshed);
            }
            _ => {
                self.weak_refresh.remove(&local_id);
            }
        }
    }

    fn refresh_frontier_for_expr(&self, expr: &Expr, ty: &Type) -> EffectFrontier {
        match expr {
            Expr::Ident { name, .. } => match self.bindings.get(&name.span) {
                Some(Binding::Local(local_id)) if matches!(ty, Type::Weak(_, _)) => self
                    .weak_refresh
                    .get(local_id)
                    .copied()
                    .unwrap_or(self.effect_frontier),
                _ => self.effect_frontier,
            },
            _ => self.effect_frontier,
        }
    }

    fn refresh_after_upgrade(&mut self, expr: &Expr) {
        if let Expr::Ident { name, .. } = expr {
            if let Some(Binding::Local(local_id)) = self.bindings.get(&name.span) {
                self.weak_refresh.insert(*local_id, self.effect_frontier);
            }
        }
    }

    pub(super) fn merge_weak_refresh(
        &self,
        entry: &HashMap<LocalId, EffectFrontier>,
        left: &HashMap<LocalId, EffectFrontier>,
        right: &HashMap<LocalId, EffectFrontier>,
    ) -> HashMap<LocalId, EffectFrontier> {
        let mut merged = HashMap::new();
        for (local_id, ty) in &self.local_types {
            if !matches!(ty, Type::Weak(_, _)) {
                continue;
            }
            let entry_state = entry.get(local_id).copied().unwrap_or_default();
            let left_state = left.get(local_id).copied().unwrap_or(entry_state);
            let right_state = right.get(local_id).copied().unwrap_or(entry_state);
            merged.insert(*local_id, left_state.meet_min(right_state));
        }
        merged
    }
}
