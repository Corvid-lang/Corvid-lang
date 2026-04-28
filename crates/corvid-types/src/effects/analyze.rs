//! Call-graph effect analyzer.
//!
//! Walks each agent body, collects the effects used by every tool /
//! prompt / agent it calls, composes those per dimension via the
//! registry's archetype rules, and records any constraint violations.
//! `collect_body_capabilities` does the parallel walk for the
//! `capability` dimension (Max-composed through the call graph to
//! gate prompt dispatch).
//!
//! Extracted from `effects.rs` as part of Phase 20i responsibility
//! decomposition.

use super::compose::capability_max;
use super::{ComposedProfile, ConstraintViolation, EffectRegistry};
use corvid_ast::DimensionValue;
use corvid_resolve::DefId;

// ---- Call-graph effect analyzer ----

/// Per-agent inferred effect profile: the union of all effects used
/// by tools/prompts/agents called in the agent's body.
#[derive(Debug, Clone)]
pub struct AgentEffectSummary {
    pub agent_def_id: DefId,
    pub agent_name: String,
    pub declared_effects: Vec<String>,
    pub inferred_effects: Vec<String>,
    pub composed: ComposedProfile,
    pub violations: Vec<ConstraintViolation>,
}

/// Analyze all agents in the file and produce per-agent effect summaries.
pub fn analyze_effects(
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    registry: &EffectRegistry,
) -> Vec<AgentEffectSummary> {
    let mut summaries = Vec::new();

    for decl in &file.decls {
        let corvid_ast::Decl::Agent(agent) = decl else {
            continue;
        };
        let Some(def_id) = resolved.symbols.lookup_def(&agent.name.name) else {
            continue;
        };

        // Collect all effect names used by calls in this agent's body.
        let mut effect_names: Vec<String> = Vec::new();
        collect_body_effects(&agent.body, file, resolved, registry, &mut effect_names);

        // Deduplicate.
        effect_names.sort();
        effect_names.dedup();

        // Compose the dimensional profile.
        let refs: Vec<&str> = effect_names.iter().map(|s| s.as_str()).collect();
        let mut composed = registry.compose(&refs);

        // Phase 20h: collect capability requirements from every
        // prompt call in the body and Max-compose them into the
        // agent's `capability` dimension. A prompt's `requires:
        // <level>` clause sets the minimum model capability its
        // dispatch needs; the agent-level composed requirement is
        // the strictest capability any call needs.
        let mut capabilities: Vec<String> = Vec::new();
        collect_body_capabilities(&agent.body, file, resolved, &mut capabilities);
        if !capabilities.is_empty() {
            let mut iter = capabilities.into_iter();
            let mut acc = iter.next().unwrap();
            for next in iter {
                acc = capability_max(&acc, &next).to_string();
            }
            composed
                .dimensions
                .insert("capability".into(), DimensionValue::Name(acc));
        }

        // Check constraints.
        let violations = registry.check_constraints(&composed, &agent.constraints);

        // Declared effects from the agent's `uses` clause.
        let declared: Vec<String> = agent
            .effect_row
            .effects
            .iter()
            .map(|e| e.name.name.clone())
            .collect();

        summaries.push(AgentEffectSummary {
            agent_def_id: def_id,
            agent_name: agent.name.name.clone(),
            declared_effects: declared,
            inferred_effects: effect_names,
            composed,
            violations,
        });
    }

    summaries
}

fn collect_body_effects(
    block: &corvid_ast::Block,
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    registry: &EffectRegistry,
    effects: &mut Vec<String>,
) {
    for stmt in &block.stmts {
        collect_stmt_effects(stmt, file, resolved, registry, effects);
    }
}

/// Phase 20h: walk a block and collect the `capability_required`
/// string from every prompt call. Mirrors `collect_body_effects`'s
/// recursion pattern but looks at prompt-level attributes instead of
/// effect-row references.
fn collect_body_capabilities(
    block: &corvid_ast::Block,
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    caps: &mut Vec<String>,
) {
    for stmt in &block.stmts {
        collect_stmt_capabilities(stmt, file, resolved, caps);
    }
}

fn collect_stmt_capabilities(
    stmt: &corvid_ast::Stmt,
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    caps: &mut Vec<String>,
) {
    match stmt {
        corvid_ast::Stmt::Let { value, .. } => {
            collect_expr_capabilities(value, file, resolved, caps);
        }
        corvid_ast::Stmt::Return { value: Some(v), .. } => {
            collect_expr_capabilities(v, file, resolved, caps);
        }
        corvid_ast::Stmt::Return { value: None, .. } => {}
        corvid_ast::Stmt::Yield { value, .. } => {
            collect_expr_capabilities(value, file, resolved, caps);
        }
        corvid_ast::Stmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            collect_expr_capabilities(cond, file, resolved, caps);
            collect_body_capabilities(then_block, file, resolved, caps);
            if let Some(eb) = else_block {
                collect_body_capabilities(eb, file, resolved, caps);
            }
        }
        corvid_ast::Stmt::For { iter, body, .. } => {
            collect_expr_capabilities(iter, file, resolved, caps);
            collect_body_capabilities(body, file, resolved, caps);
        }
        corvid_ast::Stmt::Expr { expr, .. } => {
            collect_expr_capabilities(expr, file, resolved, caps);
        }
        _ => {}
    }
}

fn collect_expr_capabilities(
    expr: &corvid_ast::Expr,
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    caps: &mut Vec<String>,
) {
    match expr {
        corvid_ast::Expr::Call { callee, args, .. } => {
            if let corvid_ast::Expr::Ident { name: ident, .. } = callee.as_ref() {
                if let Some(corvid_resolve::Binding::Decl(def_id)) =
                    resolved.bindings.get(&ident.span)
                {
                    let entry = resolved.symbols.get(*def_id);
                    if matches!(entry.kind, corvid_resolve::DeclKind::Prompt) {
                        if let Some(prompt) = find_prompt(file, &entry.name) {
                            if let Some(req) = &prompt.capability_required {
                                caps.push(req.name.clone());
                            }
                        }
                    }
                    if matches!(entry.kind, corvid_resolve::DeclKind::Agent) {
                        if let Some(agent) = find_agent(file, &entry.name) {
                            collect_body_capabilities(&agent.body, file, resolved, caps);
                        }
                    }
                }
            }
            collect_expr_capabilities(callee, file, resolved, caps);
            for arg in args {
                collect_expr_capabilities(arg, file, resolved, caps);
            }
        }
        corvid_ast::Expr::FieldAccess { target, .. } => {
            collect_expr_capabilities(target, file, resolved, caps);
        }
        corvid_ast::Expr::BinOp { left, right, .. } => {
            collect_expr_capabilities(left, file, resolved, caps);
            collect_expr_capabilities(right, file, resolved, caps);
        }
        corvid_ast::Expr::UnOp { operand, .. } => {
            collect_expr_capabilities(operand, file, resolved, caps);
        }
        _ => {}
    }
}

fn collect_stmt_effects(
    stmt: &corvid_ast::Stmt,
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    registry: &EffectRegistry,
    effects: &mut Vec<String>,
) {
    match stmt {
        corvid_ast::Stmt::Let { value, .. } => {
            collect_expr_effects(value, file, resolved, registry, effects);
        }
        corvid_ast::Stmt::Return { value: Some(v), .. } => {
            collect_expr_effects(v, file, resolved, registry, effects);
        }
        corvid_ast::Stmt::Return { value: None, .. } => {}
        corvid_ast::Stmt::Yield { value, .. } => {
            collect_expr_effects(value, file, resolved, registry, effects);
        }
        corvid_ast::Stmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            collect_expr_effects(cond, file, resolved, registry, effects);
            collect_body_effects(then_block, file, resolved, registry, effects);
            if let Some(eb) = else_block {
                collect_body_effects(eb, file, resolved, registry, effects);
            }
        }
        corvid_ast::Stmt::For { iter, body, .. } => {
            collect_expr_effects(iter, file, resolved, registry, effects);
            collect_body_effects(body, file, resolved, registry, effects);
        }
        corvid_ast::Stmt::Approve { action, .. } => {
            effects.push("approve".into());
            collect_expr_effects(action, file, resolved, registry, effects);
        }
        corvid_ast::Stmt::Expr { expr, .. } => {
            collect_expr_effects(expr, file, resolved, registry, effects);
        }
    }
}

fn collect_expr_effects(
    expr: &corvid_ast::Expr,
    file: &corvid_ast::File,
    resolved: &corvid_resolve::Resolved,
    registry: &EffectRegistry,
    effects: &mut Vec<String>,
) {
    match expr {
        corvid_ast::Expr::Call { callee, args, .. } => {
            // Check if callee resolves to a tool/prompt/agent with effects.
            if let corvid_ast::Expr::Ident { span, .. } = &**callee {
                if let Some(corvid_resolve::Binding::Decl(def_id)) = resolved.bindings.get(span) {
                    let entry = resolved.symbols.get(*def_id);
                    match entry.kind {
                        corvid_resolve::DeclKind::Tool => {
                            // Find the tool declaration and collect its effect row.
                            if let Some(tool) = find_tool(file, &entry.name) {
                                for eff in &tool.effect_row.effects {
                                    effects.push(eff.name.name.clone());
                                }
                                // Legacy: dangerous → implicit "dangerous" effect
                                if matches!(tool.effect, corvid_ast::Effect::Dangerous) {
                                    effects.push("dangerous".into());
                                }
                            }
                        }
                        corvid_resolve::DeclKind::Prompt => {
                            if let Some(prompt) = find_prompt(file, &entry.name) {
                                for eff in &prompt.effect_row.effects {
                                    effects.push(eff.name.name.clone());
                                }
                            }
                        }
                        corvid_resolve::DeclKind::Agent => {
                            if let Some(agent) = find_agent(file, &entry.name) {
                                // If the agent declares effects, use those.
                                // Otherwise, this would need recursive inference.
                                for eff in &agent.effect_row.effects {
                                    effects.push(eff.name.name.clone());
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if matches!(
                    resolved.bindings.get(span),
                    Some(corvid_resolve::Binding::BuiltIn(
                        corvid_resolve::BuiltIn::Ask | corvid_resolve::BuiltIn::Choose
                    ))
                ) {
                    effects.push("human".into());
                }
            }
            collect_expr_effects(callee, file, resolved, registry, effects);
            for arg in args {
                collect_expr_effects(arg, file, resolved, registry, effects);
            }
        }
        corvid_ast::Expr::FieldAccess { target, .. } => {
            collect_expr_effects(target, file, resolved, registry, effects);
        }
        corvid_ast::Expr::Index { target, index, .. } => {
            collect_expr_effects(target, file, resolved, registry, effects);
            collect_expr_effects(index, file, resolved, registry, effects);
        }
        corvid_ast::Expr::BinOp { left, right, .. } => {
            collect_expr_effects(left, file, resolved, registry, effects);
            collect_expr_effects(right, file, resolved, registry, effects);
        }
        corvid_ast::Expr::UnOp { operand, .. } => {
            collect_expr_effects(operand, file, resolved, registry, effects);
        }
        corvid_ast::Expr::List { items, .. } => {
            for item in items {
                collect_expr_effects(item, file, resolved, registry, effects);
            }
        }
        corvid_ast::Expr::TryPropagate { inner, .. } => {
            collect_expr_effects(inner, file, resolved, registry, effects);
        }
        corvid_ast::Expr::TryRetry { body, .. } => {
            collect_expr_effects(body, file, resolved, registry, effects);
        }
        _ => {}
    }
}

pub(super) fn find_tool<'a>(
    file: &'a corvid_ast::File,
    name: &str,
) -> Option<&'a corvid_ast::ToolDecl> {
    file.decls.iter().find_map(|d| match d {
        corvid_ast::Decl::Tool(t) if t.name.name == name => Some(t),
        _ => None,
    })
}

pub(super) fn find_prompt<'a>(
    file: &'a corvid_ast::File,
    name: &str,
) -> Option<&'a corvid_ast::PromptDecl> {
    file.decls.iter().find_map(|d| match d {
        corvid_ast::Decl::Prompt(p) if p.name.name == name => Some(p),
        _ => None,
    })
}

pub(super) fn find_agent<'a>(
    file: &'a corvid_ast::File,
    name: &str,
) -> Option<&'a corvid_ast::AgentDecl> {
    file.decls.iter().find_map(|d| match d {
        corvid_ast::Decl::Agent(a) if a.name.name == name => Some(a),
        _ => None,
    })
}
