use crate::approval_contract::{analyze_agent_approval_contract, collect_all_approval_sites};
use crate::effect_emit::{emit_effects_from_composed, emit_effects_from_effect_names};
use crate::provenance_emit::emit_provenance_contract;
use crate::schema::{
    AbiAgent, AbiAttributes, AbiBudget, AbiCostEnvelope, AbiDispatch, AbiField, AbiParam,
    AbiProgressiveStage, AbiPrompt, AbiRouteArm, AbiSourceSpan, AbiTool, AbiTypeDecl, CorvidAbi,
};
use crate::type_description::emit_type_description;
use corvid_ast::{AgentAttribute, Decl, DimensionValue, File, PromptDecl, Span, ToolDecl, TypeRef, WeakEffectRow};
use corvid_ir::{
    IrAgent, IrCallKind, IrExpr, IrExprKind, IrFile, IrPrompt, IrRoutePattern, IrTool,
    IrVoteStrategy,
};
use corvid_resolve::Resolved;
use corvid_types::{analyze_effects, Checked, EffectRegistry, Type};
use std::collections::{BTreeSet, HashMap};

#[derive(Debug, Clone)]
pub struct EmitOptions<'a> {
    pub source_path: &'a str,
    pub compiler_version: &'a str,
    pub generated_at: &'a str,
}

pub fn emit_abi(
    file: &File,
    resolved: &Resolved,
    _checked: &Checked,
    ir: &IrFile,
    registry: &EffectRegistry,
    opts: &EmitOptions<'_>,
) -> CorvidAbi {
    let summaries = analyze_effects(file, resolved, registry)
        .into_iter()
        .map(|summary| (summary.agent_def_id, summary))
        .collect::<HashMap<_, _>>();
    let prompt_map = ir.prompts.iter().map(|prompt| (prompt.id, prompt)).collect::<HashMap<_, _>>();
    let agent_map = ir.agents.iter().map(|agent| (agent.id, agent)).collect::<HashMap<_, _>>();
    let ast_prompts = file
        .decls
        .iter()
        .filter_map(|decl| match decl {
            Decl::Prompt(prompt) => Some((prompt.name.name.clone(), prompt)),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    let tool_map = file
        .decls
        .iter()
        .filter_map(|decl| match decl {
            Decl::Tool(tool) => Some((tool.name.name.clone(), tool)),
            _ => None,
        })
        .collect::<HashMap<_, _>>();

    let exported_agent_ids = collect_exported_agent_closure(ir, &agent_map);

    let agents = ir
        .agents
        .iter()
        .filter(|agent| exported_agent_ids.contains(&agent.id))
        .map(|agent| emit_agent(agent, file, resolved, registry, &summaries, &prompt_map))
        .collect();

    let prompts = ir
        .prompts
        .iter()
        .map(|prompt| emit_prompt(prompt, resolved, registry, ast_prompts.get(&prompt.name).copied()))
        .collect();

    let tools = ir
        .tools
        .iter()
        .map(|tool| emit_tool(tool, resolved, registry, tool_map.get(&tool.name).copied()))
        .collect();

    let types = ir.types.iter().map(|ty| AbiTypeDecl {
        name: ty.name.clone(),
        kind: "struct".to_string(),
        fields: ty
            .fields
            .iter()
            .map(|field| AbiField {
                name: field.name.clone(),
                r#type: emit_type_description(&field.ty, resolved),
            })
            .collect(),
    }).collect();

    let approval_sites = collect_all_approval_sites(file, resolved, registry);

    CorvidAbi {
        corvid_abi_version: crate::schema::CORVID_ABI_VERSION,
        compiler_version: opts.compiler_version.to_string(),
        source_path: normalize_source_path(opts.source_path),
        generated_at: opts.generated_at.to_string(),
        agents,
        prompts,
        tools,
        types,
        approval_sites,
        extra: Default::default(),
    }
}

pub fn normalize_source_path(path: &str) -> String {
    path.replace('\\', "/")
}

pub(crate) fn resolve_typeref_to_type(ty: &TypeRef, resolved: &Resolved) -> Type {
    match ty {
        TypeRef::Named { name, .. } => match name.name.as_str() {
            "Int" => Type::Int,
            "Float" => Type::Float,
            "String" => Type::String,
            "Bool" => Type::Bool,
            "Nothing" => Type::Nothing,
            "TraceId" => Type::TraceId,
            other => resolved
                .symbols
                .lookup_def(other)
                .map(Type::Struct)
                .unwrap_or(Type::Unknown),
        },
        TypeRef::Generic { name, args, .. } => match name.name.as_str() {
            "List" | "Stream" if args.len() == 1 => Type::List(Box::new(resolve_typeref_to_type(&args[0], resolved))),
            "Option" if args.len() == 1 => Type::Option(Box::new(resolve_typeref_to_type(&args[0], resolved))),
            "Grounded" if args.len() == 1 => Type::Grounded(Box::new(resolve_typeref_to_type(&args[0], resolved))),
            "Result" if args.len() == 2 => Type::Result(
                Box::new(resolve_typeref_to_type(&args[0], resolved)),
                Box::new(resolve_typeref_to_type(&args[1], resolved)),
            ),
            _ => Type::Unknown,
        },
        TypeRef::Weak { inner, effects, .. } => {
            Type::Weak(Box::new(resolve_typeref_to_type(inner, resolved)), effects.unwrap_or_else(WeakEffectRow::any))
        }
        TypeRef::Function { params, ret, .. } => Type::Function {
            params: params.iter().map(|param| resolve_typeref_to_type(param, resolved)).collect(),
            ret: Box::new(resolve_typeref_to_type(ret, resolved)),
            effect: corvid_ast::Effect::Safe,
        },
    }
}

fn emit_agent(
    agent: &IrAgent,
    file: &File,
    resolved: &Resolved,
    registry: &EffectRegistry,
    summaries: &HashMap<corvid_resolve::DefId, corvid_types::AgentEffectSummary>,
    prompt_map: &HashMap<corvid_resolve::DefId, &IrPrompt>,
) -> AbiAgent {
    let ast_agent = file.decls.iter().find_map(|decl| match decl {
        Decl::Agent(ast_agent) if ast_agent.name.name == agent.name => Some(ast_agent),
        _ => None,
    }).expect("agent present in AST");
    let summary = summaries.get(&agent.id);
    let approval = analyze_agent_approval_contract(file, resolved, registry, ast_agent);
    let effects = summary
        .map(|summary| emit_effects_from_composed(&summary.composed))
        .unwrap_or_default();
    let required_capability = summary
        .and_then(|summary| match summary.composed.dimensions.get("capability") {
            Some(DimensionValue::Name(value)) => Some(value.clone()),
            _ => None,
        });
    let declared_return_ty = resolve_typeref_to_type(&ast_agent.return_ty, resolved);
    AbiAgent {
        name: agent.name.clone(),
        symbol: agent.name.clone(),
        source_span: source_span(agent.span),
        params: ast_agent
            .params
            .iter()
            .map(|param| AbiParam {
                name: param.name.name.clone(),
                ty: emit_type_description(&resolve_typeref_to_type(&param.ty, resolved), resolved),
            })
            .collect(),
        return_type: emit_type_description(&declared_return_ty, resolved),
        effects,
        attributes: AbiAttributes {
            replayable: AgentAttribute::is_replayable(&ast_agent.attributes),
            deterministic: AgentAttribute::is_deterministic(&ast_agent.attributes),
            dangerous: approval.contract.required,
            pub_extern_c: ast_agent.extern_abi.is_some(),
        },
        budget: extract_budget(&ast_agent.constraints),
        required_capability,
        dispatch: infer_agent_dispatch(agent, prompt_map),
        approval_contract: approval.contract,
        provenance: emit_provenance_contract(ast_agent, &declared_return_ty),
    }
}

fn emit_prompt(
    prompt: &IrPrompt,
    resolved: &Resolved,
    registry: &EffectRegistry,
    ast_prompt: Option<&PromptDecl>,
) -> AbiPrompt {
    AbiPrompt {
        name: prompt.name.clone(),
        source_span: source_span(prompt.span),
        params: ast_prompt
            .map(|prompt| {
                prompt
                    .params
                    .iter()
                    .map(|param| AbiParam {
                        name: param.name.name.clone(),
                        ty: emit_type_description(
                            &resolve_typeref_to_type(&param.ty, resolved),
                            resolved,
                        ),
                    })
                    .collect()
            })
            .unwrap_or_else(|| {
                prompt
                    .params
                    .iter()
                    .map(|param| AbiParam {
                        name: param.name.clone(),
                        ty: emit_type_description(&param.ty, resolved),
                    })
                    .collect()
            }),
        return_type: ast_prompt
            .map(|prompt| emit_type_description(&resolve_typeref_to_type(&prompt.return_ty, resolved), resolved))
            .unwrap_or_else(|| emit_type_description(&prompt.return_ty, resolved)),
        effects: emit_effects_from_effect_names(&prompt.effect_names, registry),
        required_capability: prompt.capability_required.clone(),
        dispatch: emit_prompt_dispatch(prompt),
        cost_envelope: Some(AbiCostEnvelope {
            min_usd: prompt.effect_cost,
            typical_usd: prompt.effect_cost,
            max_usd: prompt.effect_cost,
        }),
        confidence_floor: if prompt.effect_confidence > 0.0 {
            Some(prompt.effect_confidence)
        } else {
            None
        },
        cited_params: ast_prompt
            .and_then(|prompt| prompt.cites_strictly.clone())
            .into_iter()
            .collect(),
    }
}

fn emit_tool(
    tool: &IrTool,
    resolved: &Resolved,
    registry: &EffectRegistry,
    ast_tool: Option<&ToolDecl>,
) -> AbiTool {
    AbiTool {
        name: tool.name.clone(),
        symbol: format!("corvid_tool_{}", tool.name),
        params: ast_tool
            .map(|tool_decl| {
                tool_decl
                    .params
                    .iter()
                    .map(|param| AbiParam {
                        name: param.name.name.clone(),
                        ty: emit_type_description(
                            &resolve_typeref_to_type(&param.ty, resolved),
                            resolved,
                        ),
                    })
                    .collect()
            })
            .unwrap_or_else(|| {
                tool
                    .params
                    .iter()
                    .map(|param| AbiParam {
                        name: param.name.clone(),
                        ty: emit_type_description(&param.ty, resolved),
                    })
                    .collect()
            }),
        return_type: ast_tool
            .map(|tool_decl| emit_type_description(&resolve_typeref_to_type(&tool_decl.return_ty, resolved), resolved))
            .unwrap_or_else(|| emit_type_description(&tool.return_ty, resolved)),
        effects: emit_effects_from_effect_names(&tool.effect_names, registry),
        dangerous: ast_tool.map(|tool| tool.effect == corvid_ast::Effect::Dangerous).unwrap_or(false),
    }
}

fn emit_prompt_dispatch(prompt: &IrPrompt) -> Option<AbiDispatch> {
    if !prompt.progressive.is_empty() {
        return Some(AbiDispatch::Progressive {
            stages: prompt.progressive.iter().map(|stage| AbiProgressiveStage {
                model_requires: stage.model_name.clone(),
                escalate_below_confidence: stage.threshold,
            }).collect(),
        });
    }
    if let Some(rollout) = &prompt.rollout {
        return Some(AbiDispatch::Rollout {
            variant: rollout.variant_name.clone(),
            baseline: rollout.baseline_name.clone(),
            variant_percent: rollout.variant_percent,
        });
    }
    if let Some(ensemble) = &prompt.ensemble {
        return Some(AbiDispatch::Ensemble {
            models: ensemble.models.iter().map(|model| model.name.clone()).collect(),
            vote_strategy: match ensemble.vote {
                IrVoteStrategy::Majority => "majority".into(),
            },
        });
    }
    if let Some(adversarial) = &prompt.adversarial {
        return Some(AbiDispatch::Adversarial {
            propose: adversarial.proposer_name.clone(),
            challenge: adversarial.challenger_name.clone(),
            adjudicate: adversarial.adjudicator_name.clone(),
        });
    }
    if !prompt.route.is_empty() {
        return Some(AbiDispatch::Route {
            route_arms: prompt.route.iter().map(|arm| AbiRouteArm {
                model: arm.model_name.clone(),
                matcher: match &arm.pattern {
                    IrRoutePattern::Wildcard => "_".into(),
                    IrRoutePattern::Guard(_) => "guard".into(),
                },
            }).collect(),
        });
    }
    None
}

fn infer_agent_dispatch(
    agent: &IrAgent,
    prompt_map: &HashMap<corvid_resolve::DefId, &IrPrompt>,
) -> Option<AbiDispatch> {
    let mut found = None;
    walk_ir_block_for_prompt(&agent.body, prompt_map, &mut found);
    found
}

fn collect_exported_agent_closure(
    ir: &IrFile,
    agent_map: &HashMap<corvid_resolve::DefId, &IrAgent>,
) -> BTreeSet<corvid_resolve::DefId> {
    let mut out = BTreeSet::new();
    let mut stack = ir
        .agents
        .iter()
        .filter(|agent| agent.extern_abi.is_some())
        .map(|agent| agent.id)
        .collect::<Vec<_>>();

    while let Some(agent_id) = stack.pop() {
        if !out.insert(agent_id) {
            continue;
        }
        let Some(agent) = agent_map.get(&agent_id) else {
            continue;
        };
        collect_called_agents_from_block(&agent.body, &mut stack);
    }

    out
}

fn collect_called_agents_from_block(
    block: &corvid_ir::IrBlock,
    stack: &mut Vec<corvid_resolve::DefId>,
) {
    for stmt in &block.stmts {
        match stmt {
            corvid_ir::IrStmt::Let { value, .. }
            | corvid_ir::IrStmt::Expr { expr: value, .. }
            | corvid_ir::IrStmt::Return { value: Some(value), .. }
            | corvid_ir::IrStmt::Yield { value, .. } => collect_called_agents_from_expr(value, stack),
            corvid_ir::IrStmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                collect_called_agents_from_expr(cond, stack);
                collect_called_agents_from_block(then_block, stack);
                if let Some(else_block) = else_block {
                    collect_called_agents_from_block(else_block, stack);
                }
            }
            corvid_ir::IrStmt::For { iter, body, .. } => {
                collect_called_agents_from_expr(iter, stack);
                collect_called_agents_from_block(body, stack);
            }
            _ => {}
        }
    }
}

fn collect_called_agents_from_expr(
    expr: &IrExpr,
    stack: &mut Vec<corvid_resolve::DefId>,
) {
    match &expr.kind {
        IrExprKind::Call { kind, args, .. } => {
            if let IrCallKind::Agent { def_id } = kind {
                stack.push(*def_id);
            }
            for arg in args {
                collect_called_agents_from_expr(arg, stack);
            }
        }
        IrExprKind::FieldAccess { target, .. }
        | IrExprKind::UnOp { operand: target, .. }
        | IrExprKind::WeakNew { strong: target }
        | IrExprKind::WeakUpgrade { weak: target }
        | IrExprKind::ResultOk { inner: target }
        | IrExprKind::ResultErr { inner: target }
        | IrExprKind::OptionSome { inner: target }
        | IrExprKind::TryPropagate { inner: target } => collect_called_agents_from_expr(target, stack),
        IrExprKind::Index { target, index } | IrExprKind::BinOp { left: target, right: index, .. } => {
            collect_called_agents_from_expr(target, stack);
            collect_called_agents_from_expr(index, stack);
        }
        IrExprKind::List { items } => {
            for item in items {
                collect_called_agents_from_expr(item, stack);
            }
        }
        IrExprKind::TryRetry { body, .. } => collect_called_agents_from_expr(body, stack),
        IrExprKind::Replay { trace, arms, else_body } => {
            collect_called_agents_from_expr(trace, stack);
            for arm in arms {
                collect_called_agents_from_expr(&arm.body, stack);
            }
            collect_called_agents_from_expr(else_body, stack);
        }
        IrExprKind::Literal(_)
        | IrExprKind::Local { .. }
        | IrExprKind::Decl { .. }
        | IrExprKind::OptionNone => {}
    }
}

fn walk_ir_block_for_prompt(
    block: &corvid_ir::IrBlock,
    prompt_map: &HashMap<corvid_resolve::DefId, &IrPrompt>,
    found: &mut Option<AbiDispatch>,
) {
    for stmt in &block.stmts {
        match stmt {
            corvid_ir::IrStmt::Let { value, .. }
            | corvid_ir::IrStmt::Expr { expr: value, .. }
            | corvid_ir::IrStmt::Return { value: Some(value), .. }
            | corvid_ir::IrStmt::Yield { value, .. } => walk_ir_expr_for_prompt(value, prompt_map, found),
            corvid_ir::IrStmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                walk_ir_expr_for_prompt(cond, prompt_map, found);
                walk_ir_block_for_prompt(then_block, prompt_map, found);
                if let Some(else_block) = else_block {
                    walk_ir_block_for_prompt(else_block, prompt_map, found);
                }
            }
            corvid_ir::IrStmt::For { iter, body, .. } => {
                walk_ir_expr_for_prompt(iter, prompt_map, found);
                walk_ir_block_for_prompt(body, prompt_map, found);
            }
            _ => {}
        }
        if found.is_some() {
            return;
        }
    }
}

fn walk_ir_expr_for_prompt(
    expr: &IrExpr,
    prompt_map: &HashMap<corvid_resolve::DefId, &IrPrompt>,
    found: &mut Option<AbiDispatch>,
) {
    if found.is_some() {
        return;
    }
    match &expr.kind {
        IrExprKind::Call { kind, args, .. } => {
            if let IrCallKind::Prompt { def_id } = kind {
                if let Some(prompt) = prompt_map.get(def_id) {
                    *found = emit_prompt_dispatch(prompt);
                    if found.is_some() {
                        return;
                    }
                }
            }
            for arg in args {
                walk_ir_expr_for_prompt(arg, prompt_map, found);
            }
        }
        IrExprKind::FieldAccess { target, .. }
        | IrExprKind::UnOp { operand: target, .. }
        | IrExprKind::WeakNew { strong: target }
        | IrExprKind::WeakUpgrade { weak: target }
        | IrExprKind::ResultOk { inner: target }
        | IrExprKind::ResultErr { inner: target }
        | IrExprKind::OptionSome { inner: target }
        | IrExprKind::TryPropagate { inner: target } => walk_ir_expr_for_prompt(target, prompt_map, found),
        IrExprKind::Index { target, index } | IrExprKind::BinOp { left: target, right: index, .. } => {
            walk_ir_expr_for_prompt(target, prompt_map, found);
            walk_ir_expr_for_prompt(index, prompt_map, found);
        }
        IrExprKind::List { items } => {
            for item in items {
                walk_ir_expr_for_prompt(item, prompt_map, found);
            }
        }
        IrExprKind::TryRetry { body, .. } => walk_ir_expr_for_prompt(body, prompt_map, found),
        IrExprKind::Replay { trace, arms, else_body } => {
            walk_ir_expr_for_prompt(trace, prompt_map, found);
            for arm in arms {
                walk_ir_expr_for_prompt(&arm.body, prompt_map, found);
            }
            walk_ir_expr_for_prompt(else_body, prompt_map, found);
        }
        IrExprKind::Literal(_)
        | IrExprKind::Local { .. }
        | IrExprKind::Decl { .. }
        | IrExprKind::OptionNone => {}
    }
}

fn extract_budget(constraints: &[corvid_ast::EffectConstraint]) -> Option<AbiBudget> {
    constraints.iter().find_map(|constraint| {
        if constraint.dimension.name == "budget" || constraint.dimension.name == "cost" {
            match &constraint.value {
                Some(DimensionValue::Cost(value)) => Some(AbiBudget { usd_per_call: *value }),
                Some(DimensionValue::Number(value)) => Some(AbiBudget { usd_per_call: *value }),
                _ => None,
            }
        } else {
            None
        }
    })
}

fn source_span(span: Span) -> AbiSourceSpan {
    AbiSourceSpan {
        start: span.start,
        end: span.end,
    }
}
