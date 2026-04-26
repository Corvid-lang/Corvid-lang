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
use crate::errors::{TypeError, TypeErrorKind, TypeWarning, TypeWarningKind};
use crate::types::Type;
use corvid_ast::{Expr, Ident, Literal, ReplayArm, ReplayPattern, Span, ToolArgPattern};
use corvid_resolve::{Binding, BuiltIn, DeclKind, DefId, ReplayPatternBinding};
use std::path::Path;

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
            Expr::Call { callee, args, span } => self.check_call(callee, args, *span, expected),
            Expr::FieldAccess {
                target,
                field,
                span,
            } => self.check_field(target, field, *span),
            Expr::Index {
                target,
                index,
                span,
            } => {
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
            Expr::BinOp {
                op,
                left,
                right,
                span,
            } => self.check_binop(*op, left, right, *span),
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
                            || matches!(elem_ty, Type::Float) && matches!(item_ty, Type::Int))
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
            } => self.check_replay_expr(trace, arms, else_body),
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
            Binding::Local(lid) => self.local_types.get(lid).cloned().unwrap_or(Type::Unknown),
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
                | BuiltIn::Partial
                | BuiltIn::ResumeToken
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
                | BuiltIn::WeakUpgrade
                | BuiltIn::StreamMerge
                | BuiltIn::Resume
                | BuiltIn::StreamResumeToken
                | BuiltIn::Ask
                | BuiltIn::Choose => {
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
            DeclKind::Tool | DeclKind::Prompt | DeclKind::Agent | DeclKind::Fixture => {
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
            DeclKind::Import
            | DeclKind::ImportedUse
            | DeclKind::Eval
            | DeclKind::Test
            | DeclKind::Mock
            | DeclKind::Effect
            | DeclKind::Model => Type::Unknown,
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
                if let Some(f) = type_decl.fields.iter().find(|f| f.name.name == field.name) {
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
            Type::ImportedStruct(imported) => {
                let Some(module) = self
                    .module_resolution
                    .and_then(|modules| modules.lookup_by_path(Path::new(&imported.module_path)))
                else {
                    return Type::Unknown;
                };
                let Some((struct_name, fields)) = imported_type_fields(module, imported.def_id)
                else {
                    return Type::Unknown;
                };
                if let Some(f) = fields.iter().find(|f| f.name.name == field.name) {
                    self.imported_type_ref_to_type(&f.ty, module)
                } else {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::UnknownField {
                            struct_name: struct_name.to_string(),
                            field: field.name.clone(),
                        },
                        span,
                    ));
                    Type::Unknown
                }
            }
            Type::Partial(inner) => self.check_partial_field(inner, field, span),
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

    fn check_partial_field(&mut self, inner: &Type, field: &Ident, span: Span) -> Type {
        match inner {
            Type::Struct(def_id) => {
                let type_decl = *self
                    .types_by_id
                    .get(def_id)
                    .expect("struct DefId not indexed");
                if let Some(f) = type_decl.fields.iter().find(|f| f.name.name == field.name) {
                    Type::Option(Box::new(self.type_ref_to_type(&f.ty)))
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
            Type::ImportedStruct(imported) => {
                let Some(module) = self
                    .module_resolution
                    .and_then(|modules| modules.lookup_by_path(Path::new(&imported.module_path)))
                else {
                    return Type::Unknown;
                };
                let Some((struct_name, fields)) = imported_type_fields(module, imported.def_id)
                else {
                    return Type::Unknown;
                };
                if let Some(f) = fields.iter().find(|f| f.name.name == field.name) {
                    Type::Option(Box::new(self.imported_type_ref_to_type(&f.ty, module)))
                } else {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::UnknownField {
                            struct_name: struct_name.to_string(),
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
                        got: format!("Partial<{}>", other.display_name()),
                    },
                    span,
                ));
                Type::Unknown
            }
        }
    }

    /// Typecheck a `replay <trace>: when <pat> ... else <body>`
    /// expression (Phase 21 slice 21-inv-E-3).
    ///
    /// - `<trace>` must be `TraceId`, `String` (coerces), or `Unknown`.
    /// - Each arm's capture (whole-event `as <ident>` and per-arg
    ///   `tool(name, <ident>)`) gets its type from the resolver's
    ///   `replay_pattern_bindings` side-table: LLM captures take the
    ///   prompt's return type; tool-arg captures take the tool's
    ///   first-arg type; approve captures are `Bool`.
    /// - The replay expression's result type is the join of every
    ///   arm body and the `else` body. Arms whose bodies can't be
    ///   unified with the first arm emit `ReplayArmTypeMismatch`.
    /// - Duplicate `when` patterns warn `ReplayUnreachableArm`.
    pub(super) fn check_replay_expr(
        &mut self,
        trace: &Expr,
        arms: &[ReplayArm],
        else_body: &Expr,
    ) -> Type {
        let trace_ty = self.check_expr(trace);
        match &trace_ty {
            Type::TraceId | Type::String | Type::Unknown => {}
            other => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::ReplayTraceNotATraceId {
                        got: other.display_name(),
                    },
                    trace.span(),
                ));
            }
        }

        // Track earlier arms by normalized pattern fingerprint for
        // unreachable-arm diagnostics.
        let mut seen_patterns: Vec<(String, Span)> = Vec::new();

        let mut joined: Option<Type> = None;

        for arm in arms {
            let fingerprint = replay_pattern_fingerprint(&arm.pattern);
            if let Some((_, first_span)) = seen_patterns
                .iter()
                .find(|(fp, _)| fp == &fingerprint)
                .cloned()
            {
                self.warnings.push(TypeWarning::new(
                    TypeWarningKind::ReplayUnreachableArm {
                        pattern: fingerprint.clone(),
                        first_arm_span: first_span,
                    },
                    arm.span,
                ));
            } else {
                seen_patterns.push((fingerprint, arm.span));
            }

            self.register_replay_arm_capture_types(arm);

            let arm_ty = self.check_expr(&arm.body);
            unify_replay_arm_type(&mut joined, arm_ty, &mut self.errors, arm, "a `when` arm");
        }

        let else_ty = self.check_expr(else_body);
        unify_replay_arm_type_for_else(&mut joined, else_ty, &mut self.errors, else_body);

        joined.unwrap_or(Type::Unknown)
    }

    /// Register types for the locals a replay arm's captures
    /// introduce. `whole-event as <ident>` binds a local whose type
    /// mirrors the recorded event's payload; `tool(..., <ident>)`
    /// binds a local of the tool's first-arg type.
    fn register_replay_arm_capture_types(&mut self, arm: &ReplayArm) {
        // Whole-event capture type: varies by pattern kind.
        if let Some(capture) = &arm.capture {
            let ty = self.replay_whole_event_capture_type(&arm.pattern);
            if let Some(Binding::Local(local_id)) = self.bindings.get(&capture.span).cloned() {
                self.local_types.insert(local_id, ty);
            }
        }

        // Tool-arg capture type: first-arg type of the resolved tool.
        if let ReplayPattern::Tool {
            arg: ToolArgPattern::Capture { span, .. },
            span: pattern_span,
            ..
        } = &arm.pattern
        {
            let ty = self.replay_tool_arg_capture_type(*pattern_span);
            if let Some(Binding::Local(local_id)) = self.bindings.get(span).cloned() {
                self.local_types.insert(local_id, ty);
            }
        }
    }

    /// Compute the whole-event `as <ident>` capture's type for one
    /// of the three pattern kinds. Falls back to `Unknown` when the
    /// resolver couldn't bind the pattern (e.g. on an unknown prompt
    /// name — the error is already in `self.errors`, so downstream
    /// code degrades to Unknown gracefully).
    fn replay_whole_event_capture_type(&self, pattern: &ReplayPattern) -> Type {
        match pattern {
            ReplayPattern::Llm { span, .. } => match self.replay_pattern_bindings.get(span) {
                Some(ReplayPatternBinding::Llm(def_id)) => self.prompt_return_type(*def_id),
                _ => Type::Unknown,
            },
            ReplayPattern::Tool { span, .. } => match self.replay_pattern_bindings.get(span) {
                Some(ReplayPatternBinding::Tool(def_id)) => self.tool_return_type(*def_id),
                _ => Type::Unknown,
            },
            ReplayPattern::Approve { .. } => Type::Bool,
        }
    }

    /// Compute the per-arg capture type for a `tool("name", <ident>)`
    /// pattern: the tool's first parameter type. `pattern_span` is
    /// used to look up the resolved tool DefId.
    fn replay_tool_arg_capture_type(&self, pattern_span: Span) -> Type {
        let Some(ReplayPatternBinding::Tool(def_id)) =
            self.replay_pattern_bindings.get(&pattern_span).copied()
        else {
            return Type::Unknown;
        };
        self.tool_first_param_type(def_id)
    }

    /// Look up the return type of a prompt by its DefId. The prompt
    /// decl stores a `TypeRef` (surface form); resolve it with
    /// `type_ref_to_type`. Requires `&mut self` for that helper, so
    /// this path is factored as a take-&mut method.
    fn prompt_return_type(&self, def_id: DefId) -> Type {
        let Some(prompt) = self.prompts_by_id.get(&def_id) else {
            return Type::Unknown;
        };
        type_ref_to_type_readonly(&prompt.return_ty, self)
    }

    fn tool_return_type(&self, def_id: DefId) -> Type {
        let Some(tool) = self.tools_by_id.get(&def_id) else {
            return Type::Unknown;
        };
        type_ref_to_type_readonly(&tool.return_ty, self)
    }

    fn tool_first_param_type(&self, def_id: DefId) -> Type {
        let Some(tool) = self.tools_by_id.get(&def_id) else {
            return Type::Unknown;
        };
        let Some(first) = tool.params.first() else {
            return Type::Unknown;
        };
        type_ref_to_type_readonly(&first.ty, self)
    }
}

// Free helpers outside the impl block.
//
// `type_ref_to_type` (the method-side version) mutates `self.errors`
// on malformed generic arity, which would cascade noise if we called
// it from deep inside replay checking — errors about the replay
// arm should point at the arm, not at a prompt's internal TypeRef.
// This read-only mirror accepts the same input shape but doesn't
// record diagnostics; malformed types degrade to `Type::Unknown`
// (the prompt/tool decl's own typecheck has already emitted any
// diagnostics).
fn type_ref_to_type_readonly(tr: &corvid_ast::TypeRef, checker: &Checker<'_>) -> Type {
    use corvid_ast::TypeRef;
    match tr {
        TypeRef::Named { name, .. } => match name.name.as_str() {
            "Int" => Type::Int,
            "Float" => Type::Float,
            "String" => Type::String,
            "Bool" => Type::Bool,
            "Nothing" => Type::Nothing,
            _ => checker
                .symbols
                .lookup_def(&name.name)
                .map(Type::Struct)
                .unwrap_or(Type::Unknown),
        },
        TypeRef::Generic { name, args, .. } if args.len() == 1 => {
            let inner = type_ref_to_type_readonly(&args[0], checker);
            match name.name.as_str() {
                "List" => Type::List(Box::new(inner)),
                "Stream" => Type::Stream(Box::new(inner)),
                "Option" => Type::Option(Box::new(inner)),
                "Grounded" => Type::Grounded(Box::new(inner)),
                "Partial" => Type::Partial(Box::new(inner)),
                "ResumeToken" => Type::ResumeToken(Box::new(inner)),
                _ => Type::Unknown,
            }
        }
        _ => Type::Unknown,
    }
}

fn imported_type_fields(
    module: &corvid_resolve::ResolvedModule,
    def_id: DefId,
) -> Option<(&str, &[corvid_ast::Field])> {
    module.file.decls.iter().find_map(|decl| match decl {
        corvid_ast::Decl::Type(t)
            if module
                .resolved
                .symbols
                .lookup_def(&t.name.name)
                .is_some_and(|id| id == def_id) =>
        {
            Some((t.name.name.as_str(), t.fields.as_slice()))
        }
        _ => None,
    })
}

/// Fingerprint a replay pattern for duplicate-arm detection. Two
/// patterns collide when their `(kind, name)` pairs match — the
/// capture name and tool-arg capture name don't affect matchability
/// (they're just bindings), but a literal tool-arg string does.
fn replay_pattern_fingerprint(pattern: &ReplayPattern) -> String {
    match pattern {
        ReplayPattern::Llm { prompt, .. } => format!("llm({prompt:?})"),
        ReplayPattern::Tool { tool, arg, .. } => {
            let arg_fp = match arg {
                ToolArgPattern::Wildcard { .. } => "_".into(),
                ToolArgPattern::Capture { .. } => "_".into(),
                ToolArgPattern::StringLit { value, .. } => format!("{value:?}"),
            };
            format!("tool({tool:?}, {arg_fp})")
        }
        ReplayPattern::Approve { label, .. } => format!("approve({label:?})"),
    }
}

/// Unify an arm body's type into the running join for the replay
/// expression. On the first arm, sets `joined` to the arm type. On
/// subsequent arms, checks assignability both ways; if they don't
/// unify, records a `ReplayArmTypeMismatch` error and keeps the
/// earlier type so later arms compare against a stable anchor.
fn unify_replay_arm_type(
    joined: &mut Option<Type>,
    arm_ty: Type,
    errors: &mut Vec<TypeError>,
    arm: &ReplayArm,
    context: &str,
) {
    match joined {
        None => {
            *joined = Some(arm_ty);
        }
        Some(existing) => {
            if !arm_ty.is_assignable_to(existing) && !existing.is_assignable_to(&arm_ty) {
                errors.push(TypeError::new(
                    TypeErrorKind::ReplayArmTypeMismatch {
                        expected: existing.display_name(),
                        got: arm_ty.display_name(),
                        context: context.to_string(),
                    },
                    arm.body.span(),
                ));
            }
        }
    }
}

fn unify_replay_arm_type_for_else(
    joined: &mut Option<Type>,
    else_ty: Type,
    errors: &mut Vec<TypeError>,
    else_body: &Expr,
) {
    match joined {
        None => {
            *joined = Some(else_ty);
        }
        Some(existing) => {
            if !else_ty.is_assignable_to(existing) && !existing.is_assignable_to(&else_ty) {
                errors.push(TypeError::new(
                    TypeErrorKind::ReplayArmTypeMismatch {
                        expected: existing.display_name(),
                        got: else_ty.display_name(),
                        context: "the `else` arm".into(),
                    },
                    else_body.span(),
                ));
            }
        }
    }
}
