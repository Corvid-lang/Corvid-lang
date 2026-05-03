//! Agent + eval declaration checking.
//!
//! `check_agent` validates an `agent name(params) -> T: body` Ã¢â‚¬â€
//! parameter binding, return-type matching, yield/stream legality.
//! `check_eval` validates an `eval name: body` Ã¢â‚¬â€ including
//! trace-assert (`assert called X before Y`) and statistical
//! confidence modifiers.
//!
//! Extracted from `checker.rs` as part of Phase 20i responsibility
//! decomposition.

use super::Checker;
use crate::errors::{TypeError, TypeErrorKind, TypeWarning, TypeWarningKind};
use crate::types::Type;
use corvid_ast::{
    AgentAttribute, AgentDecl, Block, Expr, HttpMethod, HttpRouteDecl, ServerDecl, Span, Stmt,
};
use corvid_resolve::Binding;
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
        // stronger than `@replayable` Ã¢â‚¬â€ the body must be a pure
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
