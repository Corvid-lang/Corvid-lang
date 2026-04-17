//! Dimensional effect composition algebra and call-graph effect analyzer.
//!
//! Each effect declaration carries typed dimensions (cost, trust,
//! reversibility, data, latency). When effects compose through a call
//! graph, each dimension composes independently via its declared rule:
//!
//!   cost        → Sum (accumulates along the call path)
//!   trust       → Max (most restrictive wins)
//!   reversible  → LeastReversible (one irreversible call makes the chain irreversible)
//!   data        → Union (all data classifications accumulate)
//!   latency     → Max (slowest call determines the chain's latency class)
//!   confidence  → Min (least confident result determines the chain)
//!
//! The analyzer walks the call graph from each agent, collects the
//! effects of every tool/prompt/agent call in its body, and computes
//! the composed dimensional profile. Constraints on the agent (e.g.,
//! `@budget($1.00)`) are then checked against the composed profile.

use corvid_ast::{CompositionRule, DimensionSchema, DimensionValue, EffectConstraint, EffectDecl};
use corvid_resolve::DefId;
use std::collections::HashMap;

/// Registry of declared effect dimensions and their composition rules.
/// Built from the file's `effect` declarations.
#[derive(Debug, Clone, Default)]
pub struct EffectRegistry {
    /// Effect name → its declared dimensions.
    pub effects: HashMap<String, EffectProfile>,
    /// Dimension name → composition rule + default. Inferred from
    /// all effect declarations (each dimension that appears in any
    /// effect gets a schema entry).
    pub dimensions: HashMap<String, DimensionSchema>,
}

/// The dimensional profile of a single declared effect.
#[derive(Debug, Clone, Default)]
pub struct EffectProfile {
    pub name: String,
    pub dimensions: HashMap<String, DimensionValue>,
}

/// A composed dimensional profile — the result of combining multiple
/// effects through a call graph.
#[derive(Debug, Clone, Default)]
pub struct ComposedProfile {
    pub dimensions: HashMap<String, DimensionValue>,
    pub effect_names: Vec<String>,
}

impl EffectRegistry {
    /// Build the registry from a list of effect declarations.
    pub fn from_decls(decls: &[EffectDecl]) -> Self {
        let mut registry = Self::default();

        // Register built-in dimension schemas with default composition rules.
        registry.register_builtin_dimensions();

        // Built-in `retrieval` effect with `data: grounded` so tools can
        // declare themselves as grounded sources for provenance tracking.
        registry.register_retrieval_effect();

        // Legacy bridge: the `dangerous` keyword on tools maps to a built-in
        // effect with `trust: human_required, reversible: false`. Existing
        // code using `dangerous` gets dimensional tracking for free.
        registry.register_dangerous_effect();

        for decl in decls {
            let mut profile = EffectProfile {
                name: decl.name.name.clone(),
                dimensions: HashMap::new(),
            };

            for dim in &decl.dimensions {
                let dim_name = dim.name.name.clone();
                profile.dimensions.insert(dim_name.clone(), dim.value.clone());

                // Infer schema from the dimension if not already registered.
                if !registry.dimensions.contains_key(&dim_name) {
                    let rule = infer_composition_rule(&dim_name, &dim.value);
                    let default = default_for_rule(rule);
                    registry.dimensions.insert(
                        dim_name,
                        DimensionSchema {
                            name: dim.name.name.clone(),
                            composition: rule,
                            default,
                        },
                    );
                }
            }

            registry.effects.insert(decl.name.name.clone(), profile);
        }

        registry
    }

    fn register_builtin_dimensions(&mut self) {
        self.dimensions.insert(
            "cost".into(),
            DimensionSchema {
                name: "cost".into(),
                composition: CompositionRule::Sum,
                default: DimensionValue::Cost(0.0),
            },
        );
        self.dimensions.insert(
            "trust".into(),
            DimensionSchema {
                name: "trust".into(),
                composition: CompositionRule::Max,
                default: DimensionValue::Name("autonomous".into()),
            },
        );
        self.dimensions.insert(
            "reversible".into(),
            DimensionSchema {
                name: "reversible".into(),
                composition: CompositionRule::LeastReversible,
                default: DimensionValue::Bool(true),
            },
        );
        self.dimensions.insert(
            "data".into(),
            DimensionSchema {
                name: "data".into(),
                composition: CompositionRule::Union,
                default: DimensionValue::Name("none".into()),
            },
        );
        self.dimensions.insert(
            "latency".into(),
            DimensionSchema {
                name: "latency".into(),
                composition: CompositionRule::Max,
                default: DimensionValue::Name("instant".into()),
            },
        );
        self.dimensions.insert(
            "confidence".into(),
            DimensionSchema {
                name: "confidence".into(),
                composition: CompositionRule::Min,
                default: DimensionValue::Number(1.0),
            },
        );
    }

    fn register_retrieval_effect(&mut self) {
        let mut dims = HashMap::new();
        dims.insert("data".into(), DimensionValue::Name("grounded".into()));
        self.effects.insert(
            "retrieval".into(),
            EffectProfile {
                name: "retrieval".into(),
                dimensions: dims,
            },
        );
    }

    fn register_dangerous_effect(&mut self) {
        let mut dims = HashMap::new();
        dims.insert("trust".into(), DimensionValue::Name("human_required".into()));
        dims.insert("reversible".into(), DimensionValue::Bool(false));
        self.effects.insert(
            "dangerous".into(),
            EffectProfile {
                name: "dangerous".into(),
                dimensions: dims,
            },
        );
    }

    /// Look up an effect by name and return its profile.
    pub fn get(&self, name: &str) -> Option<&EffectProfile> {
        self.effects.get(name)
    }

    /// Compose multiple effect names into a single dimensional profile
    /// by applying each dimension's composition rule.
    pub fn compose(&self, effect_names: &[&str]) -> ComposedProfile {
        let mut result = ComposedProfile {
            dimensions: HashMap::new(),
            effect_names: effect_names.iter().map(|s| s.to_string()).collect(),
        };

        // Start with defaults for all known dimensions.
        for (dim_name, schema) in &self.dimensions {
            result.dimensions.insert(dim_name.clone(), schema.default.clone());
        }

        // Layer each effect's dimensions using composition rules.
        for &effect_name in effect_names {
            let Some(profile) = self.effects.get(effect_name) else {
                continue;
            };
            for (dim_name, value) in &profile.dimensions {
                let Some(schema) = self.dimensions.get(dim_name) else {
                    continue;
                };
                let current = result
                    .dimensions
                    .entry(dim_name.clone())
                    .or_insert_with(|| schema.default.clone());
                *current = compose_dimension(schema.composition, current, value);
            }
        }

        result
    }

    /// Check a composed profile against a set of constraints. Returns
    /// a list of violations.
    pub fn check_constraints(
        &self,
        profile: &ComposedProfile,
        constraints: &[EffectConstraint],
    ) -> Vec<ConstraintViolation> {
        let mut violations = Vec::new();

        for constraint in constraints {
            let dim_name = &constraint.dimension.name;
            let canonical_dim = if dim_name == "budget" {
                "cost"
            } else {
                dim_name.as_str()
            };
            let Some(actual) = profile.dimensions.get(canonical_dim) else {
                continue;
            };
            let expected = match (&constraint.value, actual) {
                (Some(expected), _) => Some(expected.clone()),
                // Bare boolean constraints like `@reversible` mean
                // "this dimension must stay true".
                (None, DimensionValue::Bool(_)) => Some(DimensionValue::Bool(true)),
                (None, _) => None,
            };
            if let Some(expected) = expected {
                if !dimension_satisfies(actual, &expected, canonical_dim) {
                    violations.push(ConstraintViolation {
                        dimension: canonical_dim.to_string(),
                        constraint: expected,
                        actual: actual.clone(),
                        span: constraint.span,
                    });
                }
            }
        }

        violations
    }
}

/// A constraint violation found during dimensional checking.
#[derive(Debug, Clone)]
pub struct ConstraintViolation {
    pub dimension: String,
    pub constraint: DimensionValue,
    pub actual: DimensionValue,
    pub span: corvid_ast::Span,
}

impl std::fmt::Display for ConstraintViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "dimension `{}`: constraint requires {}, but composed value is {}",
            self.dimension,
            format_dim_value(&self.constraint),
            format_dim_value(&self.actual),
        )
    }
}

// ---- Composition rules ----

fn compose_dimension(
    rule: CompositionRule,
    current: &DimensionValue,
    incoming: &DimensionValue,
) -> DimensionValue {
    match rule {
        CompositionRule::Sum => match (current, incoming) {
            (DimensionValue::Cost(a), DimensionValue::Cost(b)) => DimensionValue::Cost(a + b),
            (DimensionValue::Number(a), DimensionValue::Number(b)) => {
                DimensionValue::Number(a + b)
            }
            _ => incoming.clone(),
        },
        CompositionRule::Max => match (current, incoming) {
            (DimensionValue::Number(a), DimensionValue::Number(b)) => {
                DimensionValue::Number(a.max(*b))
            }
            (DimensionValue::Cost(a), DimensionValue::Cost(b)) => {
                DimensionValue::Cost(a.max(*b))
            }
            (DimensionValue::Name(a), DimensionValue::Name(b)) => {
                DimensionValue::Name(trust_max(a, b).to_string())
            }
            _ => incoming.clone(),
        },
        CompositionRule::Min => match (current, incoming) {
            (DimensionValue::Number(a), DimensionValue::Number(b)) => {
                DimensionValue::Number(a.min(*b))
            }
            (DimensionValue::Cost(a), DimensionValue::Cost(b)) => {
                DimensionValue::Cost(a.min(*b))
            }
            _ => incoming.clone(),
        },
        CompositionRule::Union => match (current, incoming) {
            (DimensionValue::Name(a), DimensionValue::Name(b)) => {
                if a == "none" {
                    DimensionValue::Name(b.clone())
                } else if b == "none" {
                    DimensionValue::Name(a.clone())
                } else if a.contains(b.as_str()) {
                    DimensionValue::Name(a.clone())
                } else {
                    DimensionValue::Name(format!("{a}, {b}"))
                }
            }
            _ => incoming.clone(),
        },
        CompositionRule::LeastReversible => match (current, incoming) {
            (DimensionValue::Bool(a), DimensionValue::Bool(b)) => {
                DimensionValue::Bool(*a && *b)
            }
            _ => incoming.clone(),
        },
    }
}

/// Trust level ordering: autonomous < supervisor_required < human_required.
fn trust_max<'a>(a: &'a str, b: &'a str) -> &'a str {
    let rank = |s: &str| -> u8 {
        match s {
            "autonomous" => 0,
            "supervisor_required" => 1,
            "human_required" => 2,
            _ => 3,
        }
    };
    if rank(a) >= rank(b) { a } else { b }
}

fn dimension_satisfies(actual: &DimensionValue, constraint: &DimensionValue, dim_name: &str) -> bool {
    match (actual, constraint) {
        (DimensionValue::Cost(actual_cost), DimensionValue::Cost(budget)) => {
            actual_cost <= budget
        }
        (DimensionValue::Bool(actual_rev), DimensionValue::Bool(required_rev)) => {
            // If constraint requires reversible (true), actual must be true.
            !required_rev || *actual_rev
        }
        (DimensionValue::Name(actual_name), DimensionValue::Name(required_name)) => {
            if dim_name == "trust" {
                trust_rank(actual_name) <= trust_rank(required_name)
            } else {
                actual_name == required_name
            }
        }
        (DimensionValue::Number(actual_num), DimensionValue::Number(limit)) => {
            actual_num <= limit
        }
        _ => true,
    }
}

fn trust_rank(s: &str) -> u8 {
    match s {
        "autonomous" => 0,
        "supervisor_required" => 1,
        "human_required" => 2,
        _ => 3,
    }
}

fn infer_composition_rule(name: &str, _value: &DimensionValue) -> CompositionRule {
    match name {
        "cost" => CompositionRule::Sum,
        "trust" => CompositionRule::Max,
        "reversible" => CompositionRule::LeastReversible,
        "data" => CompositionRule::Union,
        "latency" => CompositionRule::Max,
        "confidence" => CompositionRule::Min,
        _ => CompositionRule::Max,
    }
}

fn default_for_rule(rule: CompositionRule) -> DimensionValue {
    match rule {
        CompositionRule::Sum => DimensionValue::Cost(0.0),
        CompositionRule::Max => DimensionValue::Name("none".into()),
        CompositionRule::Min => DimensionValue::Number(1.0),
        CompositionRule::Union => DimensionValue::Name("none".into()),
        CompositionRule::LeastReversible => DimensionValue::Bool(true),
    }
}

fn format_dim_value(v: &DimensionValue) -> String {
    match v {
        DimensionValue::Bool(b) => b.to_string(),
        DimensionValue::Name(n) => n.clone(),
        DimensionValue::Cost(c) => format!("${c:.4}"),
        DimensionValue::Number(n) => format!("{n}"),
    }
}

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
        let composed = registry.compose(&refs);

        // Check constraints.
        let violations = registry.check_constraints(&composed, &agent.constraints);

        // Declared effects from the agent's `uses` clause.
        let declared: Vec<String> = agent.effect_row.effects.iter()
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
        corvid_ast::Stmt::If { cond, then_block, else_block, .. } => {
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

fn find_tool<'a>(file: &'a corvid_ast::File, name: &str) -> Option<&'a corvid_ast::ToolDecl> {
    file.decls.iter().find_map(|d| match d {
        corvid_ast::Decl::Tool(t) if t.name.name == name => Some(t),
        _ => None,
    })
}

fn find_prompt<'a>(file: &'a corvid_ast::File, name: &str) -> Option<&'a corvid_ast::PromptDecl> {
    file.decls.iter().find_map(|d| match d {
        corvid_ast::Decl::Prompt(p) if p.name.name == name => Some(p),
        _ => None,
    })
}

fn find_agent<'a>(file: &'a corvid_ast::File, name: &str) -> Option<&'a corvid_ast::AgentDecl> {
    file.decls.iter().find_map(|d| match d {
        corvid_ast::Decl::Agent(a) if a.name.name == name => Some(a),
        _ => None,
    })
}

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
        let return_is_grounded = check_return_grounded(
            &agent.body,
            &grounded_locals,
            file,
            resolved,
            registry,
        );

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
        corvid_ast::Stmt::If { then_block, else_block, .. } => {
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
            if profile.dimensions.get("data")
                == Some(&DimensionValue::Name("grounded".into()))
            {
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
            corvid_ast::Stmt::Return { value: Some(expr), .. } => {
                if expr_is_grounded(expr, file, resolved, registry, grounded) {
                    return true;
                }
            }
            corvid_ast::Stmt::If { then_block, else_block, .. } => {
                let then_grounded = check_return_grounded(then_block, grounded, file, resolved, registry);
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
