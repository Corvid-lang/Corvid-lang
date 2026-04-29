//! Approval reachability analysis for production entrypoints.
//!
//! This pass is intentionally separate from expression typechecking. The
//! local checker proves a dangerous call has an in-scope approval token; this
//! pass starts from externally reachable surfaces (HTTP routes, schedules, and
//! exported agents) and records the entrypoint path that can reach an
//! unapproved dangerous tool.

use crate::errors::{TypeError, TypeErrorKind};
use corvid_ast::{
    AgentDecl, Block, Decl, Effect, Expr, HttpRouteDecl, ScheduleDecl, Span, Stmt, ToolDecl,
    Visibility,
};
use corvid_resolve::{Binding, DeclKind, DefId, Resolved};
use std::collections::{HashMap, HashSet};

pub(crate) fn check_approval_reachability(
    file: &corvid_ast::File,
    resolved: &Resolved,
) -> Vec<TypeError> {
    let index = ReachabilityIndex::new(file, resolved);
    let mut pass = ReachabilityPass {
        index,
        resolved,
        errors: Vec::new(),
    };
    pass.check_file(file);
    pass.errors
}

struct ReachabilityIndex<'a> {
    tools: HashMap<DefId, &'a ToolDecl>,
    agents: HashMap<DefId, &'a AgentDecl>,
    agents_by_name: HashMap<&'a str, DefId>,
}

impl<'a> ReachabilityIndex<'a> {
    fn new(file: &'a corvid_ast::File, resolved: &Resolved) -> Self {
        let mut tools = HashMap::new();
        let mut agents = HashMap::new();
        let mut agents_by_name = HashMap::new();
        for decl in &file.decls {
            match decl {
                Decl::Tool(tool) => {
                    if let Some(def_id) = resolved.symbols.lookup_def(&tool.name.name) {
                        tools.insert(def_id, tool);
                    }
                }
                Decl::Agent(agent) => {
                    if let Some(def_id) = resolved.symbols.lookup_def(&agent.name.name) {
                        agents.insert(def_id, agent);
                        agents_by_name.insert(agent.name.name.as_str(), def_id);
                    }
                }
                Decl::Extend(ext) => {
                    let Some(type_def_id) = resolved.symbols.lookup_def(&ext.type_name.name) else {
                        continue;
                    };
                    let Some(method_table) = resolved.methods.get(&type_def_id) else {
                        continue;
                    };
                    for method in &ext.methods {
                        let Some(entry) = method_table.get(&method.name().name) else {
                            continue;
                        };
                        match &method.kind {
                            corvid_ast::ExtendMethodKind::Tool(tool) => {
                                tools.insert(entry.def_id, tool);
                            }
                            corvid_ast::ExtendMethodKind::Agent(agent) => {
                                agents.insert(entry.def_id, agent);
                            }
                            corvid_ast::ExtendMethodKind::Prompt(_) => {}
                        }
                    }
                }
                _ => {}
            }
        }
        Self {
            tools,
            agents,
            agents_by_name,
        }
    }
}

#[derive(Clone)]
struct ApprovalToken {
    label: String,
    arity: usize,
}

struct ReachabilityPass<'a> {
    index: ReachabilityIndex<'a>,
    resolved: &'a Resolved,
    errors: Vec<TypeError>,
}

impl<'a> ReachabilityPass<'a> {
    fn check_file(&mut self, file: &'a corvid_ast::File) {
        for decl in &file.decls {
            match decl {
                Decl::Server(server) => {
                    for route in &server.routes {
                        self.check_route(&server.name.name, route);
                    }
                }
                Decl::Schedule(schedule) => self.check_schedule(schedule),
                Decl::Agent(agent)
                    if agent.visibility == Visibility::Public || agent.extern_abi.is_some() =>
                {
                    self.check_agent_entry(agent)
                }
                _ => {}
            }
        }
    }

    fn check_route(&mut self, server: &str, route: &'a HttpRouteDecl) {
        let entrypoint = format!(
            "route {} {} on server `{server}`",
            route.method.as_str(),
            route.path
        );
        self.check_block(&entrypoint, &route.body, Vec::new(), &mut HashSet::new());
    }

    fn check_schedule(&mut self, schedule: &'a ScheduleDecl) {
        let entrypoint = format!("schedule `{}` in zone `{}`", schedule.cron, schedule.zone);
        for arg in &schedule.args {
            self.check_expr(&entrypoint, arg, &[], &mut HashSet::new());
        }
        if let Some(def_id) = self.index.agents_by_name.get(schedule.target.name.as_str()) {
            self.check_agent_body(&entrypoint, *def_id, &mut HashSet::new());
        }
    }

    fn check_agent_entry(&mut self, agent: &'a AgentDecl) {
        let Some(def_id) = self.index.agents_by_name.get(agent.name.name.as_str()) else {
            return;
        };
        let entrypoint = format!("agent `{}`", agent.name.name);
        self.check_agent_body(&entrypoint, *def_id, &mut HashSet::new());
    }

    fn check_agent_body(&mut self, entrypoint: &str, def_id: DefId, visiting: &mut HashSet<DefId>) {
        if !visiting.insert(def_id) {
            return;
        }
        if let Some(agent) = self.index.agents.get(&def_id) {
            self.check_block(entrypoint, &agent.body, Vec::new(), visiting);
        }
        visiting.remove(&def_id);
    }

    fn check_block(
        &mut self,
        entrypoint: &str,
        block: &'a Block,
        mut approvals: Vec<ApprovalToken>,
        visiting: &mut HashSet<DefId>,
    ) {
        for stmt in &block.stmts {
            self.check_stmt(entrypoint, stmt, &mut approvals, visiting);
        }
    }

    fn check_stmt(
        &mut self,
        entrypoint: &str,
        stmt: &'a Stmt,
        approvals: &mut Vec<ApprovalToken>,
        visiting: &mut HashSet<DefId>,
    ) {
        match stmt {
            Stmt::Let { value, .. }
            | Stmt::Yield { value, .. }
            | Stmt::Expr { expr: value, .. } => {
                self.check_expr(entrypoint, value, approvals, visiting);
            }
            Stmt::Return { value, .. } => {
                if let Some(value) = value {
                    self.check_expr(entrypoint, value, approvals, visiting);
                }
            }
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                self.check_expr(entrypoint, cond, approvals, visiting);
                self.check_block(entrypoint, then_block, approvals.clone(), visiting);
                if let Some(else_block) = else_block {
                    self.check_block(entrypoint, else_block, approvals.clone(), visiting);
                }
            }
            Stmt::For { iter, body, .. } => {
                self.check_expr(entrypoint, iter, approvals, visiting);
                self.check_block(entrypoint, body, approvals.clone(), visiting);
            }
            Stmt::Approve { action, .. } => {
                if let Expr::Call { callee, args, .. } = action {
                    if let Expr::Ident { name, .. } = &**callee {
                        approvals.push(ApprovalToken {
                            label: name.name.clone(),
                            arity: args.len(),
                        });
                    }
                    for arg in args {
                        self.check_expr(entrypoint, arg, approvals, visiting);
                    }
                } else {
                    self.check_expr(entrypoint, action, approvals, visiting);
                }
            }
        }
    }

    fn check_expr(
        &mut self,
        entrypoint: &str,
        expr: &'a Expr,
        approvals: &[ApprovalToken],
        visiting: &mut HashSet<DefId>,
    ) {
        match expr {
            Expr::Call { callee, args, span } => {
                self.check_call(entrypoint, callee, args, *span, approvals, visiting);
                self.check_expr(entrypoint, callee, approvals, visiting);
                for arg in args {
                    self.check_expr(entrypoint, arg, approvals, visiting);
                }
            }
            Expr::FieldAccess { target, .. } | Expr::TryPropagate { inner: target, .. } => {
                self.check_expr(entrypoint, target, approvals, visiting);
            }
            Expr::Index { target, index, .. } => {
                self.check_expr(entrypoint, target, approvals, visiting);
                self.check_expr(entrypoint, index, approvals, visiting);
            }
            Expr::BinOp { left, right, .. } => {
                self.check_expr(entrypoint, left, approvals, visiting);
                self.check_expr(entrypoint, right, approvals, visiting);
            }
            Expr::UnOp { operand, .. } => self.check_expr(entrypoint, operand, approvals, visiting),
            Expr::List { items, .. } => {
                for item in items {
                    self.check_expr(entrypoint, item, approvals, visiting);
                }
            }
            Expr::TryRetry { body, .. } => {
                self.check_expr(entrypoint, body, approvals, visiting);
            }
            Expr::Replay {
                trace,
                arms,
                else_body,
                ..
            } => {
                self.check_expr(entrypoint, trace, approvals, visiting);
                for arm in arms {
                    self.check_expr(entrypoint, &arm.body, approvals, visiting);
                }
                self.check_expr(entrypoint, else_body, approvals, visiting);
            }
            Expr::Literal { .. } | Expr::Ident { .. } => {}
        }
    }

    fn check_call(
        &mut self,
        entrypoint: &str,
        callee: &'a Expr,
        args: &'a [Expr],
        span: Span,
        approvals: &[ApprovalToken],
        visiting: &mut HashSet<DefId>,
    ) {
        let Expr::Ident { name, .. } = callee else {
            return;
        };
        let Some(Binding::Decl(def_id)) = self.resolved.bindings.get(&name.span) else {
            return;
        };
        match self.resolved.symbols.get(*def_id).kind {
            DeclKind::Tool => self.check_tool_call(
                entrypoint,
                *def_id,
                name.name.as_str(),
                args,
                span,
                approvals,
            ),
            DeclKind::Agent => self.check_agent_body(entrypoint, *def_id, visiting),
            _ => {}
        }
    }

    fn check_tool_call(
        &mut self,
        entrypoint: &str,
        def_id: DefId,
        tool_name: &str,
        args: &'a [Expr],
        span: Span,
        approvals: &[ApprovalToken],
    ) {
        let Some(tool) = self.index.tools.get(&def_id) else {
            return;
        };
        if !matches!(tool.effect, Effect::Dangerous) {
            return;
        }
        let approved = approvals.iter().any(|approval| {
            snake_case(&approval.label) == tool_name && approval.arity == args.len()
        });
        if approved {
            return;
        }
        self.errors.push(TypeError::with_guarantee(
            TypeErrorKind::ApprovalReachabilityViolation {
                entrypoint: entrypoint.to_string(),
                tool: tool_name.to_string(),
                expected_approve_label: pascal_case(tool_name),
                arity: args.len(),
            },
            span,
            "approval.reachable_entrypoints_require_contract",
        ));
    }
}

fn pascal_case(s: &str) -> String {
    let mut out = String::new();
    let mut cap = true;
    for ch in s.chars() {
        if ch == '_' || ch == '-' {
            cap = true;
            continue;
        }
        if cap {
            out.extend(ch.to_uppercase());
            cap = false;
        } else {
            out.push(ch);
        }
    }
    out
}

fn snake_case(s: &str) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() {
            if i != 0 {
                out.push('_');
            }
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
        } else {
            out.push(ch);
        }
    }
    out
}
