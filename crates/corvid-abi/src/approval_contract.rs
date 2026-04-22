use crate::effect_emit::emit_effects_from_effect_names;
use crate::schema::{
    AbiApprovalContract, AbiApprovalLabel, AbiApprovalSite, AbiDeclaredAt, AbiParam,
    AbiSourceSpan,
};
use crate::type_description::emit_type_description;
use corvid_ast::{Block, Decl, Effect, Expr, File, Stmt, ToolDecl};
use corvid_resolve::Resolved;
use corvid_types::EffectRegistry;
use std::collections::{BTreeSet, HashMap};

#[derive(Debug, Clone)]
pub struct ApprovalAnalysis {
    pub contract: AbiApprovalContract,
    pub dangerous_targets: Vec<String>,
}

pub fn analyze_agent_approval_contract(
    file: &File,
    resolved: &Resolved,
    registry: &EffectRegistry,
    agent: &corvid_ast::AgentDecl,
) -> ApprovalAnalysis {
    let tools = tool_map(file);
    let mut labels = Vec::new();
    let mut dangerous_targets = BTreeSet::new();
    collect_contract_from_block(
        &agent.body,
        resolved,
        registry,
        &tools,
        &mut labels,
        &mut dangerous_targets,
    );
    ApprovalAnalysis {
        contract: AbiApprovalContract {
            required: !dangerous_targets.is_empty(),
            labels,
        },
        dangerous_targets: dangerous_targets.into_iter().collect(),
    }
}

pub fn collect_all_approval_sites(
    file: &File,
    resolved: &Resolved,
    registry: &EffectRegistry,
) -> Vec<AbiApprovalSite> {
    let tools = tool_map(file);
    let mut out = Vec::new();
    for decl in &file.decls {
        let Decl::Agent(agent) = decl else {
            continue;
        };
        collect_sites_from_block(
            &agent.body,
            &agent.name.name,
            resolved,
            registry,
            &tools,
            &mut out,
        );
    }
    out
}

fn collect_contract_from_block(
    block: &Block,
    resolved: &Resolved,
    registry: &EffectRegistry,
    tools: &HashMap<String, &ToolDecl>,
    labels: &mut Vec<AbiApprovalLabel>,
    dangerous_targets: &mut BTreeSet<String>,
) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Approve { action, .. } => {
                if let Some((label, args)) = parse_approval_action(action) {
                    if let Some(tool) = tools.get(&label_to_tool_name(&label)) {
                        if tool.effect == Effect::Dangerous {
                            dangerous_targets.insert(tool.name.name.clone());
                        }
                        let effects = emit_effects_from_effect_names(
                            &tool
                                .effect_row
                                .effects
                                .iter()
                                .map(|effect| effect.name.name.clone())
                                .collect::<Vec<_>>(),
                            registry,
                        );
                        labels.push(AbiApprovalLabel {
                            label,
                            args: tool
                                .params
                                .iter()
                                .map(|param| AbiParam {
                                    name: param.name.name.clone(),
                                    ty: emit_type_description(
                                        &crate::emit::resolve_typeref_to_type(&param.ty, resolved),
                                        resolved,
                                    ),
                                })
                                .collect(),
                            cost_at_site: effects.cost.as_ref().map(|cost| cost.projected_usd),
                            reversibility: effects.reversibility.clone(),
                            required_tier: effects.trust_tier.clone(),
                        });
                    } else {
                        labels.push(AbiApprovalLabel {
                            label,
                            args: args
                                .iter()
                                .enumerate()
                                .map(|(idx, _)| AbiParam {
                                    name: format!("arg_{idx}"),
                                    ty: crate::schema::TypeDescription::Scalar {
                                        scalar: crate::schema::ScalarTypeName::String,
                                    },
                                })
                                .collect(),
                            cost_at_site: None,
                            reversibility: None,
                            required_tier: Some("human_required".into()),
                        });
                    }
                }
            }
            Stmt::If {
                then_block,
                else_block,
                ..
            } => {
                collect_contract_from_block(then_block, resolved, registry, tools, labels, dangerous_targets);
                if let Some(else_block) = else_block {
                    collect_contract_from_block(
                        else_block,
                        resolved,
                        registry,
                        tools,
                        labels,
                        dangerous_targets,
                    );
                }
            }
            Stmt::For { body, .. } => {
                collect_contract_from_block(body, resolved, registry, tools, labels, dangerous_targets);
            }
            _ => {}
        }
    }
}

fn collect_sites_from_block(
    block: &Block,
    agent_name: &str,
    resolved: &Resolved,
    registry: &EffectRegistry,
    tools: &HashMap<String, &ToolDecl>,
    out: &mut Vec<AbiApprovalSite>,
) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Approve { action, span } => {
                if let Some((label, _)) = parse_approval_action(action) {
                    let tool_name = label_to_tool_name(&label);
                    let maybe_tool = tools.get(&tool_name);
                    let effects = maybe_tool
                        .map(|tool| {
                            emit_effects_from_effect_names(
                                &tool
                                    .effect_row
                                    .effects
                                    .iter()
                                    .map(|effect| effect.name.name.clone())
                                    .collect::<Vec<_>>(),
                                registry,
                            )
                        })
                        .unwrap_or_default();
                    out.push(AbiApprovalSite {
                        label,
                        declared_at: AbiDeclaredAt {
                            source_span: AbiSourceSpan {
                                start: span.start,
                                end: span.end,
                            },
                        },
                        agent_context: agent_name.to_string(),
                        predicate: Some(serde_json::json!({
                            "kind": "approval_site",
                            "op": "requires_approval",
                            "label": tool_name,
                            "arity": maybe_tool.map(|tool| tool.params.len()).unwrap_or(0),
                        })),
                        dangerous_targets: maybe_tool
                            .filter(|tool| tool.effect == Effect::Dangerous)
                            .map(|tool| vec![tool.name.name.clone()])
                            .unwrap_or_default(),
                        required_tier: effects
                            .trust_tier
                            .clone()
                            .unwrap_or_else(|| "human_required".into()),
                        effects,
                    });
                }
            }
            Stmt::If {
                then_block,
                else_block,
                ..
            } => {
                collect_sites_from_block(then_block, agent_name, resolved, registry, tools, out);
                if let Some(else_block) = else_block {
                    collect_sites_from_block(else_block, agent_name, resolved, registry, tools, out);
                }
            }
            Stmt::For { body, .. } => {
                collect_sites_from_block(body, agent_name, resolved, registry, tools, out);
            }
            _ => {}
        }
    }
}

fn tool_map(file: &File) -> HashMap<String, &ToolDecl> {
    file.decls
        .iter()
        .filter_map(|decl| match decl {
            Decl::Tool(tool) => Some((tool.name.name.clone(), tool)),
            _ => None,
        })
        .collect()
}

fn parse_approval_action(expr: &Expr) -> Option<(String, Vec<Expr>)> {
    match expr {
        Expr::Call { callee, args, .. } => match callee.as_ref() {
            Expr::Ident { name, .. } => Some((name.name.clone(), args.clone())),
            _ => None,
        },
        _ => None,
    }
}

fn label_to_tool_name(label: &str) -> String {
    let mut out = String::new();
    for (idx, ch) in label.chars().enumerate() {
        if ch.is_uppercase() && idx != 0 {
            out.push('_');
        }
        out.extend(ch.to_lowercase());
    }
    out
}
