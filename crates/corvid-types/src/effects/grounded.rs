//! Provenance analyzer for `Grounded<T>`.
//!
//! Verifies that every agent declared to return `Grounded<T>` has
//! a provenance path from a retrieval-tagged source (a tool whose
//! effect row contains `data: grounded`) through its body to the
//! return value. Without that path, the agent's promise that the
//! output is sourced from real data cannot be statically
//! established and the checker emits `UngroundedReturn`.
//!
//! Extracted from `effects.rs` as part of Phase 20i responsibility
//! decomposition.

use super::analyze::{find_agent, find_tool};
use super::EffectRegistry;
use corvid_ast::DimensionValue;

// ---- Provenance analyzer for Grounded<T> ----

/// Result of provenance analysis for one agent.
#[derive(Debug, Clone)]
pub struct ProvenanceResult {
    pub agent_name: String,
    pub return_is_grounded: bool,
    pub grounded_locals: Vec<String>,
    pub ungrounded_return_path: Option<String>,
}

/// Check whether an agent returning `Grounded<T>` has provenance from
/// a `data: grounded` source feeding into its return value. Returns
/// violations for agents that fail the check.
pub fn check_grounded_returns(
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    registry: &EffectRegistry,
) -> Vec<ProvenanceViolation> {
    let mut violations = Vec::new();

    for decl in &file.decls {
        let corvid_ast::Decl::Agent(agent) = decl else {
            continue;
        };

        // Check if the return type is Grounded<T>.
        let return_type_name = format_type_ref(&agent.return_ty);
        if !return_type_name.starts_with("Grounded") {
            continue;
        }

        // Analyze provenance: which locals are grounded?
        let grounded_locals = analyze_agent_provenance(agent, file, resolved, registry);

        // Check if any return statement returns a grounded value.
        let return_is_grounded =
            check_return_grounded(&agent.body, &grounded_locals, file, resolved, registry);

        if !return_is_grounded {
            violations.push(ProvenanceViolation {
                agent_name: agent.name.name.clone(),
                span: agent.return_ty.span(),
                message: format!(
                    "agent `{}` returns `{}` but no provenance path from a `data: grounded` \
                     source feeds into the return value. Call a tool with `uses retrieval` \
                     and pass its result (directly or through a prompt) to the return.",
                    agent.name.name, return_type_name,
                ),
            });
        }
    }

    violations
}

/// A provenance violation: an agent returns Grounded<T> without proof.
#[derive(Debug, Clone)]
pub struct ProvenanceViolation {
    pub agent_name: String,
    pub span: corvid_ast::Span,
    pub message: String,
}

/// Analyze which local variables in an agent body are "grounded" —
/// i.e., their value chain includes at least one `data: grounded` tool.
fn analyze_agent_provenance(
    agent: &corvid_ast::AgentDecl,
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    registry: &EffectRegistry,
) -> std::collections::HashSet<String> {
    let mut grounded: std::collections::HashSet<String> = std::collections::HashSet::new();

    for stmt in &agent.body.stmts {
        analyze_stmt_provenance(stmt, file, resolved, registry, &mut grounded);
    }

    grounded
}

fn analyze_stmt_provenance(
    stmt: &corvid_ast::Stmt,
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    registry: &EffectRegistry,
    grounded: &mut std::collections::HashSet<String>,
) {
    match stmt {
        corvid_ast::Stmt::Let { name, value, .. } => {
            if expr_is_grounded(value, file, resolved, registry, grounded) {
                grounded.insert(name.name.clone());
            }
        }
        corvid_ast::Stmt::Yield { .. } => {}
        corvid_ast::Stmt::If {
            then_block,
            else_block,
            ..
        } => {
            for s in &then_block.stmts {
                analyze_stmt_provenance(s, file, resolved, registry, grounded);
            }
            if let Some(eb) = else_block {
                for s in &eb.stmts {
                    analyze_stmt_provenance(s, file, resolved, registry, grounded);
                }
            }
        }
        corvid_ast::Stmt::For { body, .. } => {
            for s in &body.stmts {
                analyze_stmt_provenance(s, file, resolved, registry, grounded);
            }
        }
        _ => {}
    }
}

/// Determine if an expression produces a grounded value.
fn expr_is_grounded(
    expr: &corvid_ast::Expr,
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    registry: &EffectRegistry,
    grounded: &std::collections::HashSet<String>,
) -> bool {
    match expr {
        corvid_ast::Expr::Call { callee, args, .. } => {
            // Check if callee is a tool/prompt/agent with grounded effects.
            if let corvid_ast::Expr::Ident { span, .. } = &**callee {
                if let Some(corvid_resolve::Binding::Decl(def_id)) = resolved.bindings.get(span) {
                    let entry = resolved.symbols.get(*def_id);
                    match entry.kind {
                        corvid_resolve::DeclKind::Tool => {
                            if let Some(tool) = find_tool(file, &entry.name) {
                                // Check if the tool has a grounded effect.
                                if tool_is_grounded(tool, registry) {
                                    return true;
                                }
                            }
                        }
                        corvid_resolve::DeclKind::Prompt => {
                            // A prompt is grounded if ANY of its args are grounded.
                            // This is the key provenance flow: grounded input → grounded output.
                            for arg in args {
                                if expr_is_grounded(arg, file, resolved, registry, grounded) {
                                    return true;
                                }
                            }
                        }
                        corvid_resolve::DeclKind::Agent => {
                            // Check if the called agent returns Grounded<T>.
                            if let Some(agent) = find_agent(file, &entry.name) {
                                let ret_name = format_type_ref(&agent.return_ty);
                                if ret_name.starts_with("Grounded") {
                                    return true;
                                }
                                // Also check if the agent has grounded effects.
                                for eff in &agent.effect_row.effects {
                                    if let Some(profile) = registry.get(&eff.name.name) {
                                        if profile.dimensions.get("data")
                                            == Some(&DimensionValue::Name("grounded".into()))
                                        {
                                            return true;
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            false
        }
        corvid_ast::Expr::Ident { name, .. } => {
            // A local variable is grounded if it was previously assigned a grounded value.
            grounded.contains(&name.name)
        }
        corvid_ast::Expr::FieldAccess { target, .. } => {
            // Field access on a grounded struct is grounded.
            expr_is_grounded(target, file, resolved, registry, grounded)
        }
        _ => false,
    }
}

fn tool_is_grounded(tool: &corvid_ast::ToolDecl, registry: &EffectRegistry) -> bool {
    for eff in &tool.effect_row.effects {
        if let Some(profile) = registry.get(&eff.name.name) {
            if profile.dimensions.get("data") == Some(&DimensionValue::Name("grounded".into())) {
                return true;
            }
        }
        // Built-in: "retrieval" effect has data: grounded.
        if eff.name.name == "retrieval" {
            return true;
        }
    }
    false
}

fn check_return_grounded(
    block: &corvid_ast::Block,
    grounded: &std::collections::HashSet<String>,
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    registry: &EffectRegistry,
) -> bool {
    for stmt in &block.stmts {
        match stmt {
            corvid_ast::Stmt::Return {
                value: Some(expr), ..
            } => {
                if expr_is_grounded(expr, file, resolved, registry, grounded) {
                    return true;
                }
            }
            corvid_ast::Stmt::Yield { .. } => {}
            corvid_ast::Stmt::If {
                then_block,
                else_block,
                ..
            } => {
                let then_grounded =
                    check_return_grounded(then_block, grounded, file, resolved, registry);
                let else_grounded = else_block.as_ref().map_or(false, |eb| {
                    check_return_grounded(eb, grounded, file, resolved, registry)
                });
                if then_grounded || else_grounded {
                    return true;
                }
            }
            corvid_ast::Stmt::For { body, .. } => {
                if check_return_grounded(body, grounded, file, resolved, registry) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn format_type_ref(ty: &corvid_ast::TypeRef) -> String {
    match ty {
        corvid_ast::TypeRef::Named { name, .. } => name.name.clone(),
        corvid_ast::TypeRef::Qualified { alias, name, .. } => {
            format!("{}.{}", alias.name, name.name)
        }
        corvid_ast::TypeRef::Generic { name, args, .. } => {
            let inner: Vec<String> = args.iter().map(format_type_ref).collect();
            format!("{}<{}>", name.name, inner.join(", "))
        }
        corvid_ast::TypeRef::Weak { inner, .. } => format!("Weak<{}>", format_type_ref(inner)),
        corvid_ast::TypeRef::Function { params, ret, .. } => {
            let ps: Vec<String> = params.iter().map(format_type_ref).collect();
            format!("({}) -> {}", ps.join(", "), format_type_ref(ret))
        }
    }
}
