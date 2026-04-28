//! Agent + eval declaration checking.
//!
//! `check_agent` validates an `agent name(params) -> T: body` —
//! parameter binding, return-type matching, yield/stream legality.
//! `check_eval` validates an `eval name: body` — including
//! trace-assert (`assert called X before Y`) and statistical
//! confidence modifiers.
//!
//! Extracted from `checker.rs` as part of Phase 20i responsibility
//! decomposition.

use super::Checker;
use crate::determinism::{classify_call_target, NondeterminismSource};
use crate::errors::{TypeError, TypeErrorKind, TypeWarning, TypeWarningKind};
use crate::types::Type;
use corvid_ast::{
    AgentAttribute, AgentDecl, Block, EvalAssert, EvalDecl, Expr, FixtureDecl, HttpMethod,
    HttpRouteDecl, Ident, MockDecl, OwnershipAnnotation, OwnershipMode, ServerDecl, Span, Stmt,
    TestDecl,
};
use corvid_resolve::{Binding, BuiltIn, DeclKind};
use std::collections::HashSet;

impl<'a> Checker<'a> {
    pub(super) fn check_agent(&mut self, a: &AgentDecl) {
        // Bind parameter types.
        self.bind_params(&a.params);

        if a.extern_abi.is_some() {
            self.check_extern_c_signature(a);
        }

        let declared_ret = self.type_ref_to_type(&a.return_ty);
        let prev_ret = std::mem::replace(&mut self.current_return, Some(declared_ret.clone()));
        let prev_in_agent = std::mem::replace(&mut self.in_agent_body, true);
        let prev_saw_yield = std::mem::replace(&mut self.saw_yield, false);

        self.check_block(&a.body);

        if matches!(declared_ret, Type::Stream(_)) && !self.saw_yield {
            self.warnings.push(TypeWarning::new(
                TypeWarningKind::StreamReturnWithoutYield {
                    agent: a.name.name.clone(),
                },
                a.span,
            ));
        }

        // Phase 21 slice 21-inv-A: enforce `@replayable`. An agent
        // carrying the attribute must call only functions whose
        // outputs the trace schema can capture; anything in the
        // determinism catalog (clocks, PRNGs, environment reads,
        // etc.) is off-limits.
        //
        // The catalog is empty as of Phase 21 v1 because Corvid
        // source does not yet expose any nondeterministic builtins.
        // The walk runs anyway so the enforcement path is live and
        // ready to fire the moment an entry lands.
        if AgentAttribute::is_replayable(&a.attributes) {
            self.check_replayable_body(&a.name.name, &a.body);
        }

        // Phase 21 slice 21-inv-F: enforce `@deterministic`. Strictly
        // stronger than `@replayable` — the body must be a pure
        // function of its parameters. No LLM / tool / approve
        // calls, no catalog-registered nondeterminism, and calls
        // to other agents must target agents that are themselves
        // marked `@deterministic`.
        if AgentAttribute::is_deterministic(&a.attributes) {
            self.check_deterministic_body(&a.name.name, &a.body);
        }

        self.current_return = prev_ret;
        self.in_agent_body = prev_in_agent;
        self.saw_yield = prev_saw_yield;
        // (Locals leak between agents in our single-scope model; harmless
        //  since each agent binds its params fresh at the start.)
    }

    pub(super) fn check_server(&mut self, server: &ServerDecl) {
        let mut seen = HashSet::new();
        for route in &server.routes {
            let key = (route.method, route.path.clone());
            if !seen.insert(key) {
                self.errors.push(TypeError::new(
                    TypeErrorKind::DuplicateServerRoute {
                        server: server.name.name.clone(),
                        method: route.method.as_str().into(),
                        path: route.path.clone(),
                    },
                    route.span,
                ));
            }
            if matches!(route.method, HttpMethod::Get) && route.body_ty.is_some() {
                self.errors.push(TypeError::new(
                    TypeErrorKind::GetRouteBody {
                        server: server.name.name.clone(),
                        path: route.path.clone(),
                    },
                    route.span,
                ));
            }
            self.check_http_route(server, route);
        }
    }

    fn check_http_route(&mut self, server: &ServerDecl, route: &HttpRouteDecl) {
        let path_fields = route
            .path_params
            .iter()
            .map(|param| (param.name.name.clone(), self.type_ref_to_type(&param.ty)))
            .collect();
        self.bind_route_local_by_name(&route.body, "path", Type::RouteParams(path_fields));
        if let Some(query_ty) = &route.query_ty {
            let ty = self.type_ref_to_type(query_ty);
            self.bind_route_local_by_name(&route.body, "query", ty);
        }
        if let Some(body_ty) = &route.body_ty {
            let ty = self.type_ref_to_type(body_ty);
            self.bind_route_local_by_name(&route.body, "body", ty);
        }

        let declared_ret = self.type_ref_to_type(&route.response.ty);
        let prev_ret = std::mem::replace(&mut self.current_return, Some(declared_ret));
        let prev_in_agent = std::mem::replace(&mut self.in_agent_body, true);
        let prev_saw_yield = std::mem::replace(&mut self.saw_yield, false);

        self.check_block(&route.body);

        self.current_return = prev_ret;
        self.in_agent_body = prev_in_agent;
        self.saw_yield = prev_saw_yield;

        let _ = server;
    }

    fn bind_route_local_by_name(&mut self, block: &Block, name: &str, ty: Type) {
        let mut spans = Vec::new();
        collect_ident_spans_by_name_in_block(block, name, &mut spans);
        for span in spans {
            if let Some(Binding::Local(local_id)) = self.bindings.get(&span).cloned() {
                self.local_types.insert(local_id, ty.clone());
            }
        }
    }

    fn check_extern_c_signature(&mut self, a: &AgentDecl) {
        for param in &a.params {
            let ty = self.type_ref_to_type(&param.ty);
            if !extern_c_param_type_supported(&ty) {
                self.errors.push(TypeError::new(
                    TypeErrorKind::NonScalarInExternC {
                        agent: a.name.name.clone(),
                        offender_type: ty.display_name(),
                        position: format!("parameter `{}`", param.name.name),
                    },
                    param.span,
                ));
                continue;
            }
            match infer_extern_param_ownership(&ty) {
                Ok(inferred) => {
                    if let Some(declared) = param.ownership.as_ref() {
                        if !ownership_matches(declared, &inferred) {
                            self.errors.push(TypeError::new(
                                TypeErrorKind::ExternOwnershipMismatch {
                                    agent: a.name.name.clone(),
                                    position: format!("parameter `{}`", param.name.name),
                                    declared: ownership_label_declared(declared),
                                    inferred: ownership_label_inferred(&inferred),
                                    reason: inferred.reason.clone(),
                                },
                                param.span,
                            ));
                        }
                    }
                }
                Err(reason) => {
                    if param.ownership.is_none() {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::AmbiguousExternOwnership {
                                agent: a.name.name.clone(),
                                position: format!("parameter `{}`", param.name.name),
                            },
                            param.span,
                        ));
                    } else {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::ExternOwnershipMismatch {
                                agent: a.name.name.clone(),
                                position: format!("parameter `{}`", param.name.name),
                                declared: ownership_label_declared(
                                    param.ownership.as_ref().unwrap(),
                                ),
                                inferred: "ambiguous".into(),
                                reason,
                            },
                            param.span,
                        ));
                    }
                }
            }
        }
        let ret = self.type_ref_to_type(&a.return_ty);
        if !extern_c_return_type_supported(&ret) {
            self.errors.push(TypeError::new(
                TypeErrorKind::NonScalarInExternC {
                    agent: a.name.name.clone(),
                    offender_type: ret.display_name(),
                    position: "return type".into(),
                },
                a.return_ty.span(),
            ));
            return;
        }
        match infer_extern_return_ownership(&ret) {
            Ok(inferred) => {
                if let Some(declared) = a.return_ownership.as_ref() {
                    if !ownership_matches(declared, &inferred) {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::ExternOwnershipMismatch {
                                agent: a.name.name.clone(),
                                position: "return type".into(),
                                declared: ownership_label_declared(declared),
                                inferred: ownership_label_inferred(&inferred),
                                reason: inferred.reason.clone(),
                            },
                            a.return_ty.span(),
                        ));
                    }
                }
            }
            Err(reason) => {
                if a.return_ownership.is_none() {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::AmbiguousExternOwnership {
                            agent: a.name.name.clone(),
                            position: "return type".into(),
                        },
                        a.return_ty.span(),
                    ));
                } else {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::ExternOwnershipMismatch {
                            agent: a.name.name.clone(),
                            position: "return type".into(),
                            declared: ownership_label_declared(
                                a.return_ownership.as_ref().unwrap(),
                            ),
                            inferred: "ambiguous".into(),
                            reason,
                        },
                        a.return_ty.span(),
                    ));
                }
            }
        }
    }

    /// Walk `body` looking for calls to functions the determinism
    /// catalog flags as nondeterministic. Emits one
    /// `NonReplayableCall` error per offending call site. Safe to
    /// call on any body — a catalog-empty walk is a no-op.
    fn check_replayable_body(&mut self, agent_name: &str, body: &Block) {
        let mut violations = Vec::new();
        collect_replayability_violations_in_block(body, &mut violations);
        for violation in violations {
            self.errors.push(TypeError::with_guarantee(
                TypeErrorKind::NonReplayableCall {
                    agent: agent_name.to_string(),
                    call: violation.call_name,
                    source_label: violation.source.label().to_string(),
                },
                violation.span,
                "replay.deterministic_pure_path",
            ));
        }
    }

    /// Walk `body` enforcing the `@deterministic` contract: no
    /// LLM prompt calls, no tool calls, no approve statements,
    /// no catalog-registered nondeterminism, and every called
    /// agent must itself be `@deterministic`. Needs resolver
    /// access (to classify call targets by `DeclKind`) so this
    /// lives on `Checker` rather than as a free helper like the
    /// replayability walk.
    fn check_deterministic_body(&mut self, agent_name: &str, body: &Block) {
        self.walk_deterministic_block(agent_name, body);
    }

    fn walk_deterministic_block(&mut self, agent: &str, block: &Block) {
        for stmt in &block.stmts {
            self.walk_deterministic_stmt(agent, stmt);
        }
    }

    fn walk_deterministic_stmt(&mut self, agent: &str, stmt: &Stmt) {
        match stmt {
            Stmt::Let { value, .. } => self.walk_deterministic_expr(agent, value),
            Stmt::Return { value, .. } => {
                if let Some(expr) = value {
                    self.walk_deterministic_expr(agent, expr);
                }
            }
            Stmt::Yield { value, .. } => self.walk_deterministic_expr(agent, value),
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                self.walk_deterministic_expr(agent, cond);
                self.walk_deterministic_block(agent, then_block);
                if let Some(eb) = else_block {
                    self.walk_deterministic_block(agent, eb);
                }
            }
            Stmt::For { iter, body, .. } => {
                self.walk_deterministic_expr(agent, iter);
                self.walk_deterministic_block(agent, body);
            }
            Stmt::Expr { expr, .. } => self.walk_deterministic_expr(agent, expr),
            Stmt::Approve { action, span } => {
                // Approve is an LLM-layer concern; a pure function
                // cannot gate on user approval.
                self.errors.push(TypeError::with_guarantee(
                    TypeErrorKind::NonDeterministicCall {
                        agent: agent.to_string(),
                        call: callee_name(action).unwrap_or_else(|| "<approve target>".into()),
                        call_kind: "approve".into(),
                    },
                    *span,
                    "replay.deterministic_pure_path",
                ));
                self.walk_deterministic_expr(agent, action);
            }
        }
    }

    fn walk_deterministic_expr(&mut self, agent: &str, expr: &Expr) {
        match expr {
            Expr::Call { callee, args, span } => {
                self.classify_deterministic_call(agent, callee, *span);
                self.walk_deterministic_expr(agent, callee);
                for arg in args {
                    self.walk_deterministic_expr(agent, arg);
                }
            }
            Expr::FieldAccess { target, .. } | Expr::TryPropagate { inner: target, .. } => {
                self.walk_deterministic_expr(agent, target);
            }
            Expr::Index { target, index, .. } => {
                self.walk_deterministic_expr(agent, target);
                self.walk_deterministic_expr(agent, index);
            }
            Expr::BinOp { left, right, .. } => {
                self.walk_deterministic_expr(agent, left);
                self.walk_deterministic_expr(agent, right);
            }
            Expr::UnOp { operand, .. } => self.walk_deterministic_expr(agent, operand),
            Expr::List { items, .. } => {
                for item in items {
                    self.walk_deterministic_expr(agent, item);
                }
            }
            Expr::TryRetry { body, .. } => self.walk_deterministic_expr(agent, body),
            Expr::Replay {
                trace,
                arms,
                else_body,
                ..
            } => {
                // Walk subexpressions so determinism violations
                // nested inside a replay arm still surface. The
                // `replay` expression itself is treated as pure
                // substrate today — full classification lands with
                // the checker slice (21-inv-E-3).
                self.walk_deterministic_expr(agent, trace);
                for arm in arms {
                    self.walk_deterministic_expr(agent, &arm.body);
                }
                self.walk_deterministic_expr(agent, else_body);
            }
            Expr::Literal { .. } | Expr::Ident { .. } => {}
        }
    }

    /// Classify a call target inside a `@deterministic` body and
    /// emit a `NonDeterministicCall` error if the target fails
    /// the contract. Unresolved or dynamic callees (subscripts,
    /// chained calls) are passed over — they cannot be statically
    /// classified, so the conservative choice is to let the
    /// existing call-check machinery handle them.
    fn classify_deterministic_call(&mut self, agent: &str, callee: &Expr, span: Span) {
        let name = match callee_name(callee) {
            Some(name) => name,
            None => return,
        };

        // Catalog-registered nondeterminism (clocks, PRNGs, etc.)
        // fails `@deterministic` for the same reason it fails
        // `@replayable` — but the error message is stricter.
        if let Some(source) = classify_call_target(&name) {
            self.errors.push(TypeError::with_guarantee(
                TypeErrorKind::NonDeterministicCall {
                    agent: agent.to_string(),
                    call: name.clone(),
                    call_kind: source.label().to_string(),
                },
                span,
                "replay.deterministic_pure_path",
            ));
            return;
        }

        // Resolved decl lookup: if the callee is a bare
        // identifier that binds to a tool / prompt / non-
        // `@deterministic` agent, flag it. Method-call form
        // `x.foo()` is handled by the type checker's method
        // machinery and is deliberately out of scope here for
        // v1 — the catalog + ident-call coverage is enough to
        // enforce the contract on realistic programs; a
        // follow-up slice can extend to method dispatch if
        // users start writing `@deterministic` bodies that
        // route tool calls through receivers.
        let ident_span = match callee {
            Expr::Ident { span, .. } => Some(*span),
            _ => None,
        };
        let binding = ident_span.and_then(|s| self.bindings.get(&s).cloned());
        if let Some(Binding::Decl(def_id)) = binding {
            let entry = self.symbols.get(def_id);
            let call_kind = match entry.kind {
                DeclKind::Tool => Some("tool"),
                DeclKind::Prompt => Some("prompt"),
                DeclKind::Agent => {
                    let callee_agent = self.agents_by_id.get(&def_id).copied();
                    let is_det = callee_agent
                        .map(|a| AgentAttribute::is_deterministic(&a.attributes))
                        .unwrap_or(false);
                    if is_det {
                        None
                    } else {
                        Some("non-`@deterministic` agent")
                    }
                }
                _ => None,
            };
            if let Some(kind) = call_kind {
                self.errors.push(TypeError::with_guarantee(
                    TypeErrorKind::NonDeterministicCall {
                        agent: agent.to_string(),
                        call: name,
                        call_kind: kind.to_string(),
                    },
                    span,
                    "replay.deterministic_pure_path",
                ));
            }
        } else if let Some(Binding::BuiltIn(BuiltIn::Ask | BuiltIn::Choose)) = binding {
            self.errors.push(TypeError::with_guarantee(
                TypeErrorKind::NonDeterministicCall {
                    agent: agent.to_string(),
                    call: name,
                    call_kind: "human".to_string(),
                },
                span,
                "replay.deterministic_pure_path",
            ));
        }
    }

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

fn extern_c_param_type_supported(ty: &Type) -> bool {
    matches!(ty, Type::Int | Type::Float | Type::Bool | Type::String)
}

fn extern_c_return_type_supported(ty: &Type) -> bool {
    match ty {
        Type::Int | Type::Float | Type::Bool | Type::String | Type::Nothing => true,
        Type::Grounded(inner) => matches!(
            &**inner,
            Type::Int | Type::Float | Type::Bool | Type::String
        ),
        _ => false,
    }
}

#[derive(Debug, Clone)]
struct InferredOwnership {
    mode: OwnershipMode,
    lifetime: Option<String>,
    reason: String,
}

fn infer_extern_param_ownership(ty: &Type) -> Result<InferredOwnership, String> {
    match ty {
        Type::String | Type::TraceId => Ok(InferredOwnership {
            mode: OwnershipMode::Borrowed,
            lifetime: Some("call".to_string()),
            reason: "string-like extern parameters are passed as borrowed call-frame inputs".into(),
        }),
        Type::Int | Type::Float | Type::Bool => Ok(InferredOwnership {
            mode: OwnershipMode::Owned,
            lifetime: None,
            reason: "scalar copy parameters transfer no lifetime obligations back to the caller"
                .into(),
        }),
        other => Err(format!(
            "the compiler cannot infer a stable ownership mode for extern parameter type `{}`",
            other.display_name()
        )),
    }
}

fn infer_extern_return_ownership(ty: &Type) -> Result<InferredOwnership, String> {
    match ty {
        Type::Int | Type::Float | Type::Bool | Type::Nothing | Type::String | Type::TraceId => {
            Ok(InferredOwnership {
                mode: OwnershipMode::Owned,
                lifetime: None,
                reason: "extern return values cross the boundary as owned results".into(),
            })
        }
        Type::Grounded(inner)
            if matches!(
                &**inner,
                Type::Int | Type::Float | Type::Bool | Type::String | Type::TraceId
            ) =>
        {
            Ok(InferredOwnership {
                mode: OwnershipMode::Owned,
                lifetime: None,
                reason: "grounded handles must be returned as owned lifecycle objects".into(),
            })
        }
        other => Err(format!(
            "the compiler cannot infer a stable ownership mode for extern return type `{}`",
            other.display_name()
        )),
    }
}

fn ownership_matches(declared: &OwnershipAnnotation, inferred: &InferredOwnership) -> bool {
    if declared.mode != inferred.mode {
        return false;
    }
    let declared_lifetime = declared.lifetime.as_deref().unwrap_or_else(|| {
        if matches!(declared.mode, OwnershipMode::Borrowed) {
            "call"
        } else {
            ""
        }
    });
    let inferred_lifetime = inferred.lifetime.as_deref().unwrap_or_else(|| {
        if matches!(inferred.mode, OwnershipMode::Borrowed) {
            "call"
        } else {
            ""
        }
    });
    declared_lifetime == inferred_lifetime
}

fn ownership_label_declared(annotation: &OwnershipAnnotation) -> String {
    ownership_label(annotation.mode, annotation.lifetime.as_deref())
}

fn ownership_label_inferred(annotation: &InferredOwnership) -> String {
    ownership_label(annotation.mode, annotation.lifetime.as_deref())
}

fn ownership_label(mode: OwnershipMode, lifetime: Option<&str>) -> String {
    match mode {
        OwnershipMode::Owned => "@owned".into(),
        OwnershipMode::Borrowed => match lifetime {
            Some("call") | None => "@borrowed".into(),
            Some(name) => format!("@borrowed<'{name}>"),
        },
        OwnershipMode::Shared => "@shared".into(),
        OwnershipMode::Static => "@static".into(),
    }
}

// ----------------------------------------------------------------
// Replayability walk helpers (Phase 21 slice 21-inv-A)
// ----------------------------------------------------------------
//
// These walk an agent body and collect `ReplayabilityViolation`
// entries — one per call site that resolves to a nondeterministic
// builtin the trace schema cannot capture. Free functions (not
// methods) so the walk doesn't need `Checker` state; the checker
// pushes the resulting violations into its own error vec.

/// One replayability violation — a call in a `@replayable` body
/// that resolves to a nondeterministic builtin.
struct ReplayabilityViolation {
    call_name: String,
    source: NondeterminismSource,
    span: Span,
}

fn collect_replayability_violations_in_block(block: &Block, out: &mut Vec<ReplayabilityViolation>) {
    for stmt in &block.stmts {
        collect_replayability_violations_in_stmt(stmt, out);
    }
}

fn collect_replayability_violations_in_stmt(stmt: &Stmt, out: &mut Vec<ReplayabilityViolation>) {
    match stmt {
        Stmt::Let { value, .. } => {
            collect_replayability_violations_in_expr(value, out);
        }
        Stmt::Return { value, .. } => {
            if let Some(expr) = value {
                collect_replayability_violations_in_expr(expr, out);
            }
        }
        Stmt::Yield { value, .. } => {
            collect_replayability_violations_in_expr(value, out);
        }
        Stmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            collect_replayability_violations_in_expr(cond, out);
            collect_replayability_violations_in_block(then_block, out);
            if let Some(eb) = else_block {
                collect_replayability_violations_in_block(eb, out);
            }
        }
        Stmt::For { iter, body, .. } => {
            collect_replayability_violations_in_expr(iter, out);
            collect_replayability_violations_in_block(body, out);
        }
        Stmt::Expr { expr, .. } => {
            collect_replayability_violations_in_expr(expr, out);
        }
        Stmt::Approve { action, .. } => {
            collect_replayability_violations_in_expr(action, out);
        }
    }
}

fn collect_replayability_violations_in_expr(expr: &Expr, out: &mut Vec<ReplayabilityViolation>) {
    match expr {
        Expr::Call { callee, args, span } => {
            if let Some(name) = callee_name(callee) {
                if let Some(source) = classify_call_target(&name) {
                    out.push(ReplayabilityViolation {
                        call_name: name,
                        source,
                        span: *span,
                    });
                }
            }
            collect_replayability_violations_in_expr(callee, out);
            for arg in args {
                collect_replayability_violations_in_expr(arg, out);
            }
        }
        Expr::FieldAccess { target, .. } | Expr::TryPropagate { inner: target, .. } => {
            collect_replayability_violations_in_expr(target, out);
        }
        Expr::Index { target, index, .. } => {
            collect_replayability_violations_in_expr(target, out);
            collect_replayability_violations_in_expr(index, out);
        }
        Expr::BinOp { left, right, .. } => {
            collect_replayability_violations_in_expr(left, out);
            collect_replayability_violations_in_expr(right, out);
        }
        Expr::UnOp { operand, .. } => {
            collect_replayability_violations_in_expr(operand, out);
        }
        Expr::List { items, .. } => {
            for item in items {
                collect_replayability_violations_in_expr(item, out);
            }
        }
        Expr::TryRetry { body, .. } => {
            collect_replayability_violations_in_expr(body, out);
        }
        Expr::Replay {
            trace,
            arms,
            else_body,
            ..
        } => {
            // Walk subexpressions so a replayability violation
            // nested in a replay arm still surfaces. The replay
            // expression itself is replayable-by-construction; its
            // full contract lands with 21-inv-E-3.
            collect_replayability_violations_in_expr(trace, out);
            for arm in arms {
                collect_replayability_violations_in_expr(&arm.body, out);
            }
            collect_replayability_violations_in_expr(else_body, out);
        }
        Expr::Literal { .. } | Expr::Ident { .. } => {}
    }
}

/// Pull a static callee name out of an expression, if the callee
/// is a bare identifier or dotted path. Dynamic callees (subscript,
/// call-returning-call, etc.) return `None`, which the replayability
/// walk treats as "out of catalog scope" — the checker cannot
/// classify them statically, and runtime paths already route
/// through the recorded dispatch layer.
fn callee_name(callee: &Expr) -> Option<String> {
    match callee {
        Expr::Ident { name, .. } => Some(name.name.clone()),
        Expr::FieldAccess { target, field, .. } => {
            let base = callee_name(target)?;
            Some(format!("{base}.{}", field.name))
        }
        _ => None,
    }
}

fn collect_ident_spans_by_name_in_block(block: &Block, name: &str, spans: &mut Vec<Span>) {
    for stmt in &block.stmts {
        collect_ident_spans_by_name_in_stmt(stmt, name, spans);
    }
}

fn collect_ident_spans_by_name_in_stmt(stmt: &Stmt, name: &str, spans: &mut Vec<Span>) {
    match stmt {
        Stmt::Let { value, .. } => collect_ident_spans_by_name_in_expr(value, name, spans),
        Stmt::Return { value, .. } => {
            if let Some(expr) = value {
                collect_ident_spans_by_name_in_expr(expr, name, spans);
            }
        }
        Stmt::Yield { value, .. } => collect_ident_spans_by_name_in_expr(value, name, spans),
        Stmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            collect_ident_spans_by_name_in_expr(cond, name, spans);
            collect_ident_spans_by_name_in_block(then_block, name, spans);
            if let Some(block) = else_block {
                collect_ident_spans_by_name_in_block(block, name, spans);
            }
        }
        Stmt::For { iter, body, .. } => {
            collect_ident_spans_by_name_in_expr(iter, name, spans);
            collect_ident_spans_by_name_in_block(body, name, spans);
        }
        Stmt::Approve { action, .. } => collect_ident_spans_by_name_in_expr(action, name, spans),
        Stmt::Expr { expr, .. } => collect_ident_spans_by_name_in_expr(expr, name, spans),
    }
}

fn collect_ident_spans_by_name_in_expr(expr: &Expr, name: &str, spans: &mut Vec<Span>) {
    match expr {
        Expr::Ident { name: ident, .. } if ident.name == name => spans.push(ident.span),
        Expr::Ident { .. } | Expr::Literal { .. } => {}
        Expr::Call { callee, args, .. } => {
            collect_ident_spans_by_name_in_expr(callee, name, spans);
            for arg in args {
                collect_ident_spans_by_name_in_expr(arg, name, spans);
            }
        }
        Expr::FieldAccess { target, .. } | Expr::TryPropagate { inner: target, .. } => {
            collect_ident_spans_by_name_in_expr(target, name, spans);
        }
        Expr::Index { target, index, .. } => {
            collect_ident_spans_by_name_in_expr(target, name, spans);
            collect_ident_spans_by_name_in_expr(index, name, spans);
        }
        Expr::BinOp { left, right, .. } => {
            collect_ident_spans_by_name_in_expr(left, name, spans);
            collect_ident_spans_by_name_in_expr(right, name, spans);
        }
        Expr::UnOp { operand, .. } => collect_ident_spans_by_name_in_expr(operand, name, spans),
        Expr::List { items, .. } => {
            for item in items {
                collect_ident_spans_by_name_in_expr(item, name, spans);
            }
        }
        Expr::TryRetry { body, .. } => collect_ident_spans_by_name_in_expr(body, name, spans),
        Expr::Replay {
            trace,
            arms,
            else_body,
            ..
        } => {
            collect_ident_spans_by_name_in_expr(trace, name, spans);
            for arm in arms {
                collect_ident_spans_by_name_in_expr(&arm.body, name, spans);
            }
            collect_ident_spans_by_name_in_expr(else_body, name, spans);
        }
    }
}
