//! Post-resolution dependency graph: which declarations reference
//! which other declarations.
//!
//! Built by walking the resolver's `bindings` side-table and the AST
//! to attribute each identifier use to its enclosing declaration.
//! Produces both forward deps (what does X use?) and reverse deps
//! (what uses X?) for the REPL's redefinition cascade.

use crate::scope::{Binding, DefId};
use crate::resolver::Resolved;
use corvid_ast::{Block, Decl, Expr, File, Param, Stmt, TypeRef};
use std::collections::{HashMap, HashSet};

/// Forward + reverse dependency edges between top-level declarations.
#[derive(Debug, Clone, Default)]
pub struct DepGraph {
    pub forward: HashMap<DefId, HashSet<DefId>>,
    pub reverse: HashMap<DefId, HashSet<DefId>>,
}

impl DepGraph {
    pub fn dependents_of(&self, id: DefId) -> HashSet<DefId> {
        self.reverse.get(&id).cloned().unwrap_or_default()
    }

    pub fn transitive_dependents(&self, id: DefId) -> HashSet<DefId> {
        let mut visited = HashSet::new();
        let mut queue = vec![id];
        while let Some(current) = queue.pop() {
            if let Some(deps) = self.reverse.get(&current) {
                for &dep in deps {
                    if visited.insert(dep) {
                        queue.push(dep);
                    }
                }
            }
        }
        visited
    }

    pub fn dependencies_of(&self, id: DefId) -> HashSet<DefId> {
        self.forward.get(&id).cloned().unwrap_or_default()
    }
}

/// Build the dependency graph from a resolved file.
pub fn build_dep_graph(file: &File, resolved: &Resolved) -> DepGraph {
    let mut graph = DepGraph::default();

    for decl in &file.decls {
        let owner_id = match decl_def_id(decl, resolved) {
            Some(id) => id,
            None => continue,
        };

        let mut deps = HashSet::new();
        collect_decl_deps(decl, resolved, &mut deps);
        deps.remove(&owner_id);

        for &dep_id in &deps {
            graph.reverse.entry(dep_id).or_default().insert(owner_id);
        }
        graph.forward.insert(owner_id, deps);
    }

    graph
}

fn decl_def_id(decl: &Decl, resolved: &Resolved) -> Option<DefId> {
    let name = decl_name(decl)?;
    resolved.symbols.lookup_def(name)
}

pub fn decl_name(decl: &Decl) -> Option<&str> {
    match decl {
        Decl::Import(d) => d.alias.as_ref().map(|a| a.name.as_str()),
        Decl::Type(d) => Some(&d.name.name),
        Decl::Tool(d) => Some(&d.name.name),
        Decl::Prompt(d) => Some(&d.name.name),
        Decl::Agent(d) => Some(&d.name.name),
        Decl::Eval(d) => Some(&d.name.name),
        Decl::Extend(_) | Decl::Effect(_) => None,
    }
}

fn collect_decl_deps(decl: &Decl, resolved: &Resolved, deps: &mut HashSet<DefId>) {
    match decl {
        Decl::Agent(agent) => {
            collect_params_deps(&agent.params, resolved, deps);
            collect_typeref_dep(&agent.return_ty, resolved, deps);
            collect_block_deps(&agent.body, resolved, deps);
        }
        Decl::Eval(eval) => {
            collect_block_deps(&eval.body, resolved, deps);
            for assertion in &eval.assertions {
                match assertion {
                    corvid_ast::EvalAssert::Value { expr, .. } => {
                        collect_expr_deps(expr, resolved, deps);
                    }
                    corvid_ast::EvalAssert::Called { tool, .. } => {
                        if let Some(Binding::Decl(id)) = resolved.bindings.get(&tool.span) {
                            deps.insert(*id);
                        }
                    }
                    corvid_ast::EvalAssert::Approved { .. }
                    | corvid_ast::EvalAssert::Cost { .. } => {}
                    corvid_ast::EvalAssert::Ordering { before, after, .. } => {
                        for ident in [before, after] {
                            if let Some(Binding::Decl(id)) = resolved.bindings.get(&ident.span) {
                                deps.insert(*id);
                            }
                        }
                    }
                }
            }
        }
        Decl::Tool(tool) => {
            collect_params_deps(&tool.params, resolved, deps);
            collect_typeref_dep(&tool.return_ty, resolved, deps);
        }
        Decl::Prompt(prompt) => {
            collect_params_deps(&prompt.params, resolved, deps);
            collect_typeref_dep(&prompt.return_ty, resolved, deps);
        }
        Decl::Type(ty) => {
            for field in &ty.fields {
                collect_typeref_dep(&field.ty, resolved, deps);
            }
        }
        Decl::Import(_) | Decl::Extend(_) | Decl::Effect(_) => {}
    }
}

fn collect_params_deps(params: &[Param], resolved: &Resolved, deps: &mut HashSet<DefId>) {
    for param in params {
        collect_typeref_dep(&param.ty, resolved, deps);
    }
}

fn collect_typeref_dep(ty: &TypeRef, resolved: &Resolved, deps: &mut HashSet<DefId>) {
    match ty {
        TypeRef::Named { name, .. } => {
            if let Some(id) = resolved.symbols.lookup_def(&name.name) {
                deps.insert(id);
            }
        }
        TypeRef::Generic { name, args, .. } => {
            if let Some(id) = resolved.symbols.lookup_def(&name.name) {
                deps.insert(id);
            }
            for arg in args {
                collect_typeref_dep(arg, resolved, deps);
            }
        }
        TypeRef::Weak { inner, .. } => {
            collect_typeref_dep(inner, resolved, deps);
        }
        TypeRef::Function { params, ret, .. } => {
            for p in params {
                collect_typeref_dep(p, resolved, deps);
            }
            collect_typeref_dep(ret, resolved, deps);
        }
    }
}

fn collect_block_deps(block: &Block, resolved: &Resolved, deps: &mut HashSet<DefId>) {
    for stmt in &block.stmts {
        collect_stmt_deps(stmt, resolved, deps);
    }
}

fn collect_stmt_deps(stmt: &Stmt, resolved: &Resolved, deps: &mut HashSet<DefId>) {
    match stmt {
        Stmt::Let { value, .. } => {
            collect_expr_deps(value, resolved, deps);
        }
        Stmt::Return { value: Some(value), .. } => {
            collect_expr_deps(value, resolved, deps);
        }
        Stmt::Return { value: None, .. } => {}
        Stmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            collect_expr_deps(cond, resolved, deps);
            collect_block_deps(then_block, resolved, deps);
            if let Some(eb) = else_block {
                collect_block_deps(eb, resolved, deps);
            }
        }
        Stmt::For { iter, body, .. } => {
            collect_expr_deps(iter, resolved, deps);
            collect_block_deps(body, resolved, deps);
        }
        Stmt::Approve { action, .. } => {
            collect_expr_deps(action, resolved, deps);
        }
        Stmt::Expr { expr, .. } => {
            collect_expr_deps(expr, resolved, deps);
        }
    }
}

fn collect_expr_deps(expr: &Expr, resolved: &Resolved, deps: &mut HashSet<DefId>) {
    match expr {
        Expr::Ident { span, .. } => {
            if let Some(Binding::Decl(id)) = resolved.bindings.get(span) {
                deps.insert(*id);
            }
        }
        Expr::Call { callee, args, .. } => {
            collect_expr_deps(callee, resolved, deps);
            for arg in args {
                collect_expr_deps(arg, resolved, deps);
            }
        }
        Expr::FieldAccess { target, .. } => {
            collect_expr_deps(target, resolved, deps);
        }
        Expr::Index { target, index, .. } => {
            collect_expr_deps(target, resolved, deps);
            collect_expr_deps(index, resolved, deps);
        }
        Expr::BinOp { left, right, .. } => {
            collect_expr_deps(left, resolved, deps);
            collect_expr_deps(right, resolved, deps);
        }
        Expr::UnOp { operand, .. } => {
            collect_expr_deps(operand, resolved, deps);
        }
        Expr::List { items, .. } => {
            for item in items {
                collect_expr_deps(item, resolved, deps);
            }
        }
        Expr::TryPropagate { inner, .. } => {
            collect_expr_deps(inner, resolved, deps);
        }
        Expr::TryRetry { body, .. } => {
            collect_expr_deps(body, resolved, deps);
        }
        Expr::Literal { .. } => {}
    }
}
