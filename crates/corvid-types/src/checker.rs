//! The type checker and effect checker.
//!
//! Walks a parsed, resolved `File` and:
//!   * assigns a `Type` to every expression (side table, keyed by span)
//!   * validates call arities and parameter/return compatibility
//!   * enforces the approve-before-dangerous invariant
//!
//! See `ARCHITECTURE.md` §6 and `FEATURES.md` v0.1.

use crate::errors::{TypeError, TypeErrorKind, TypeWarning, TypeWarningKind};
use crate::types::Type;
use corvid_ast::{
    AgentDecl, BinaryOp, Block, Decl, Effect, EvalAssert, EvalDecl, Expr, ExtendMethodKind, File,
    Ident, Literal, Param, PromptDecl, Span, Stmt, ToolDecl, TypeDecl, TypeRef, UnaryOp,
    WeakEffect, WeakEffectRow,
};
use corvid_resolve::{
    resolver::{MethodEntry, MethodKind},
    Binding, BuiltIn, DeclKind, DefId, LocalId, Resolved, SymbolTable,
};
use std::collections::HashMap;

/// Output of the type checker.
#[derive(Debug, Clone)]
pub struct Checked {
    /// Type assigned to each expression, keyed by the expression's span.
    pub types: HashMap<Span, Type>,
    /// Type assigned to each local binding visible in the checked file.
    pub local_types: HashMap<LocalId, Type>,
    /// All errors found. Reporting continues past each error.
    pub errors: Vec<TypeError>,
    /// Non-fatal diagnostics.
    pub warnings: Vec<TypeWarning>,
}

pub fn typecheck(file: &File, resolved: &Resolved) -> Checked {
    let mut c = Checker::new(file, resolved);
    c.check_file(file);

    let effect_decls: Vec<&corvid_ast::EffectDecl> = file.decls.iter().filter_map(|d| {
        if let Decl::Effect(e) = d { Some(e) } else { None }
    }).collect();
    let owned_decls: Vec<corvid_ast::EffectDecl> = effect_decls.iter().cloned().cloned().collect();
    let registry = crate::effects::EffectRegistry::from_decls(&owned_decls);

    // Dimensional effect analysis: collect effect declarations, build
    // the registry, analyze agents, and report non-cost constraint violations.
    if !effect_decls.is_empty() || file.decls.iter().any(|d| {
        matches!(d, Decl::Agent(a) if !a.constraints.is_empty())
    }) {
        let summaries = crate::effects::analyze_effects(file, resolved, &registry);
        for summary in &summaries {
            for violation in &summary.violations {
                if matches!(violation.dimension.as_str(), "cost" | "tokens" | "latency_ms") {
                    continue;
                }
                c.errors.push(TypeError::new(
                    TypeErrorKind::EffectConstraintViolation {
                        agent: summary.agent_name.clone(),
                        dimension: violation.dimension.clone(),
                        message: violation.to_string(),
                    },
                    violation.span,
                ));
            }
        }
    }

    for decl in &file.decls {
        let Decl::Agent(agent) = decl else { continue };
        let budget_constraints: Vec<_> = agent
            .constraints
            .iter()
            .filter(|constraint| {
                matches!(
                    crate::effects::canonical_dimension_name(&constraint.dimension.name).as_str(),
                    "cost" | "tokens" | "latency_ms"
                )
            })
            .cloned()
            .collect();
        if budget_constraints.is_empty() {
            continue;
        }

        if let Some(estimate) =
            crate::effects::compute_worst_case_cost(file, resolved, &registry, &agent.name.name)
        {
            for warning in estimate.warnings {
                let crate::effects::CostWarningKind::UnboundedLoop { agent, message } = warning.kind;
                c.warnings.push(TypeWarning::new(
                    TypeWarningKind::UnboundedCostAnalysis { agent, message },
                    warning.span,
                ));
            }
            if !estimate.bounded {
                continue;
            }

            for constraint in &budget_constraints {
                let dim = crate::effects::canonical_dimension_name(&constraint.dimension.name);
                let actual = estimate.dimensions.get(&dim).copied().unwrap_or(0.0);
                let Some(limit) = crate::effects::numeric_constraint_value(constraint) else {
                    continue;
                };
                if actual > limit {
                    let path = crate::effects::cost_path_for_dimension(&estimate.tree, &dim);
                    let path_text = if path.is_empty() {
                        "path attribution unavailable".to_string()
                    } else {
                        path.join(" → ")
                    };
                    let message = format!(
                        "{}: {} > {} budget (path: {})",
                        dim,
                        crate::effects::format_numeric_dimension(&dim, actual),
                        crate::effects::format_numeric_dimension(&dim, limit),
                        path_text,
                    );
                    c.errors.push(TypeError::new(
                        TypeErrorKind::EffectConstraintViolation {
                            agent: agent.name.name.clone(),
                            dimension: dim,
                            message,
                        },
                        constraint.span,
                    ));
                }
            }
        }
    }

    // Provenance verification: check that agents returning Grounded<T>
    // actually have a provenance path from a data: grounded source.
    {
        let provenance_violations = crate::effects::check_grounded_returns(file, resolved, &registry);
        for violation in provenance_violations {
            c.errors.push(TypeError::new(
                TypeErrorKind::UngroundedReturn {
                    agent: violation.agent_name,
                    message: violation.message,
                },
                violation.span,
            ));
        }
    }

    Checked {
        types: c.types,
        local_types: c.local_types,
        errors: c.errors,
        warnings: c.warnings,
    }
}

struct Checker<'a> {
    symbols: &'a SymbolTable,
    bindings: &'a HashMap<Span, Binding>,
    types: HashMap<Span, Type>,
    errors: Vec<TypeError>,
    warnings: Vec<TypeWarning>,

    /// Indexed declarations for O(1) lookup by DefId. Methods from
    /// `extend` blocks get inserted here too — a method `extend Order: agent
    /// total(o: Order) -> Int` indexes into `agents_by_id` under the
    /// method's allocated DefId, alongside file-level free agents.
    tools_by_id: HashMap<DefId, &'a ToolDecl>,
    prompts_by_id: HashMap<DefId, &'a PromptDecl>,
    agents_by_id: HashMap<DefId, &'a AgentDecl>,
    types_by_id: HashMap<DefId, &'a TypeDecl>,

    /// Per-receiver-type method side-table from the
    /// resolver. Method calls (`x.foo(args)`) look up `x`'s declared
    /// type then this map to find the method's `DefId`, after which
    /// dispatch reuses the existing tool / prompt / agent call paths.
    methods: &'a HashMap<DefId, HashMap<String, MethodEntry>>,

    /// Type of each local binding, populated as we enter scopes.
    local_types: HashMap<LocalId, Type>,

    /// Declared return type of the currently-checked function-like.
    current_return: Option<Type>,
    in_agent_body: bool,
    saw_yield: bool,

    /// Approvals visible at the current point. Represented as a flat
    /// stack that is truncated back to its parent's length when a block
    /// is exited. This gives block-local effect scoping for free.
    approvals: Vec<Approval>,

    /// Monotonic per-effect epochs used to prove `Weak::upgrade(...)`
    /// stays ahead of invalidating effects.
    effect_frontier: EffectFrontier,

    /// Last-refresh snapshot per weak local.
    weak_refresh: HashMap<LocalId, EffectFrontier>,
}

#[derive(Debug, Clone)]
struct Approval {
    /// The user-written label (e.g. `IssueRefund`).
    label: String,
    /// Number of arguments in the approve.
    arity: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct EffectFrontier {
    tool_call: u64,
    llm: u64,
    approve: u64,
}

impl EffectFrontier {
    fn bumped(mut self, effect: WeakEffect) -> Self {
        match effect {
            WeakEffect::ToolCall => self.tool_call += 1,
            WeakEffect::Llm => self.llm += 1,
            WeakEffect::Approve => self.approve += 1,
        }
        self
    }

    fn merge_max(self, other: Self) -> Self {
        Self {
            tool_call: self.tool_call.max(other.tool_call),
            llm: self.llm.max(other.llm),
            approve: self.approve.max(other.approve),
        }
    }

    fn meet_min(self, other: Self) -> Self {
        Self {
            tool_call: self.tool_call.min(other.tool_call),
            llm: self.llm.min(other.llm),
            approve: self.approve.min(other.approve),
        }
    }

    fn invalidating_effects_since(
        &self,
        refreshed_at: &EffectFrontier,
        row: WeakEffectRow,
    ) -> Vec<String> {
        let mut effects = Vec::new();
        if row.tool_call && self.tool_call != refreshed_at.tool_call {
            effects.push("tool_call".into());
        }
        if row.llm && self.llm != refreshed_at.llm {
            effects.push("llm".into());
        }
        if row.approve && self.approve != refreshed_at.approve {
            effects.push("approve".into());
        }
        effects
    }
}

impl<'a> Checker<'a> {
    fn new(file: &'a File, resolved: &'a Resolved) -> Self {
        let mut tools = HashMap::new();
        let mut prompts = HashMap::new();
        let mut agents = HashMap::new();
        let mut types = HashMap::new();

        for decl in &file.decls {
            match decl {
                Decl::Tool(t) => {
                    if let Some(id) = resolved.symbols.lookup_def(&t.name.name) {
                        tools.insert(id, t);
                    }
                }
                Decl::Prompt(p) => {
                    if let Some(id) = resolved.symbols.lookup_def(&p.name.name) {
                        prompts.insert(id, p);
                    }
                }
                Decl::Agent(a) => {
                    if let Some(id) = resolved.symbols.lookup_def(&a.name.name) {
                        agents.insert(id, a);
                    }
                }
                Decl::Eval(_) => {}
                Decl::Type(t) => {
                    if let Some(id) = resolved.symbols.lookup_def(&t.name.name) {
                        types.insert(id, t);
                    }
                }
                Decl::Import(_) => {}
                Decl::Effect(_) => {}
                Decl::Extend(ext) => {
                    // Index method decls by their allocated DefIds
                    // (from the resolver's
                    // method side-table) into the same per-kind
                    // tables free decls use, so call-resolution can
                    // dispatch uniformly.
                    let Some(type_def_id) =
                        resolved.symbols.lookup_def(&ext.type_name.name)
                    else {
                        continue;
                    };
                    let Some(method_table) = resolved.methods.get(&type_def_id) else {
                        continue;
                    };
                    for method in &ext.methods {
                        let name = method.name().name.as_str();
                        let Some(entry) = method_table.get(name) else {
                            continue;
                        };
                        match &method.kind {
                            ExtendMethodKind::Tool(t) => {
                                tools.insert(entry.def_id, t);
                            }
                            ExtendMethodKind::Prompt(p) => {
                                prompts.insert(entry.def_id, p);
                            }
                            ExtendMethodKind::Agent(a) => {
                                agents.insert(entry.def_id, a);
                            }
                        }
                    }
                }
            }
        }

        Self {
            symbols: &resolved.symbols,
            bindings: &resolved.bindings,
            types: HashMap::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
            tools_by_id: tools,
            prompts_by_id: prompts,
            agents_by_id: agents,
            types_by_id: types,
            methods: &resolved.methods,
            local_types: HashMap::new(),
            current_return: None,
            in_agent_body: false,
            saw_yield: false,
            approvals: Vec::new(),
            effect_frontier: EffectFrontier::default(),
            weak_refresh: HashMap::new(),
        }
    }

    // ------------------------------------------------------------
    // File-level traversal.
    // ------------------------------------------------------------

    fn check_file(&mut self, file: &File) {
        for decl in &file.decls {
            match decl {
                Decl::Agent(a) => self.check_agent(a),
                Decl::Eval(e) => self.check_eval(e),
                Decl::Prompt(p) => self.check_prompt(p),
                Decl::Tool(_) | Decl::Type(_) | Decl::Import(_) | Decl::Effect(_) => {}
                Decl::Extend(ext) => {
                    // Typecheck agent method bodies the same way free
                    // agents are checked.
                    // Tool methods have no body. Prompt methods
                    // have a template (not a code block) — its
                    // typecheck is the same as a free prompt's.
                    for method in &ext.methods {
                        match &method.kind {
                            ExtendMethodKind::Agent(a) => self.check_agent(a),
                            ExtendMethodKind::Prompt(p) => self.check_prompt(p),
                            ExtendMethodKind::Tool(_) => {}
                        }
                    }
                }
            }
        }
    }

    fn check_agent(&mut self, a: &AgentDecl) {
        // Bind parameter types.
        self.bind_params(&a.params);

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

        self.current_return = prev_ret;
        self.in_agent_body = prev_in_agent;
        self.saw_yield = prev_saw_yield;
        // (Locals leak between agents in our single-scope model; harmless
        //  since each agent binds its params fresh at the start.)
    }

    fn check_prompt(&mut self, p: &PromptDecl) {
        let return_ty = self.type_ref_to_type(&p.return_ty);
        let has_stream_modifiers = p.stream.min_confidence.is_some()
            || p.stream.max_tokens.is_some()
            || p.stream.backpressure.is_some();

        if has_stream_modifiers && !matches!(return_ty, Type::Stream(_) | Type::Unknown) {
            self.errors.push(TypeError::new(
                TypeErrorKind::TypeMismatch {
                    expected: "Stream<T>".into(),
                    got: return_ty.display_name(),
                    context: format!("stream modifiers on prompt `{}`", p.name.name),
                },
                p.span,
            ));
        }

        if let Some(confidence) = p.stream.min_confidence {
            if !(0.0..=1.0).contains(&confidence) {
                self.errors.push(TypeError::new(
                    TypeErrorKind::InvalidConfidence { value: confidence },
                    p.span,
                ));
            }
        }
    }

    fn check_eval(&mut self, e: &EvalDecl) {
        let prev_ret = self.current_return.take();
        let prev_in_agent = std::mem::replace(&mut self.in_agent_body, false);
        let prev_saw_yield = self.saw_yield;
        self.check_block(&e.body);
        for assertion in &e.assertions {
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
                        self.errors.push(TypeError::new(
                            TypeErrorKind::InvalidConfidence { value: *value },
                            *span,
                        ));
                    }
                }
                if matches!(runs, Some(0)) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::InvalidConfidence { value: 0.0 },
                        *span,
                    ));
                }
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

    fn has_known_approval_label(&self, label: &str) -> bool {
        self.tools_by_id
            .values()
            .any(|tool| matches!(tool.effect, Effect::Dangerous) && pascal_case(&tool.name.name) == label)
    }

    fn bind_params(&mut self, params: &[Param]) {
        for p in params {
            if let Some(Binding::Local(local_id)) = self.bindings.get(&p.name.span) {
                let ty = self.type_ref_to_type(&p.ty);
                if matches!(ty, Type::Weak(_, _)) {
                    self.weak_refresh.insert(*local_id, self.effect_frontier);
                } else {
                    self.weak_refresh.remove(local_id);
                }
                self.local_types.insert(*local_id, ty);
            }
        }
    }

    // ------------------------------------------------------------
    // Blocks and statements.
    // ------------------------------------------------------------

    fn check_block(&mut self, b: &Block) {
        // Save approval-stack depth so approvals don't leak out of this block.
        let saved_depth = self.approvals.len();
        for stmt in &b.stmts {
            self.check_stmt(stmt);
        }
        self.approvals.truncate(saved_depth);
    }

    fn check_stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let { name, ty, value, .. } => {
                let explicit_ty = ty.as_ref().map(|t| self.type_ref_to_type(t));
                let value_ty = self.check_expr_as(value, explicit_ty.as_ref());
                let local_ty = match ty {
                    Some(_) => explicit_ty.expect("explicit let type already computed"),
                    None => value_ty.clone(),
                };
                if !value_ty.is_assignable_to(&local_ty) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: local_ty.display_name(),
                            got: value_ty.display_name(),
                            context: format!("assignment to `{}`", name.name),
                        },
                        value.span(),
                    ));
                }
                if let Some(Binding::Local(local_id)) = self.bindings.get(&name.span) {
                    self.update_weak_local_on_assignment(*local_id, value, &local_ty);
                    self.local_types.insert(*local_id, local_ty);
                }
            }
            Stmt::Return { value, span } => {
                let got = match value {
                    Some(e) => {
                        let expected = self.current_return.clone();
                        self.check_expr_as(e, expected.as_ref())
                    }
                    None => Type::Nothing,
                };
                if let Some(expected) = &self.current_return {
                    if !got.is_assignable_to(expected) {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::ReturnTypeMismatch {
                                expected: expected.display_name(),
                                got: got.display_name(),
                            },
                            *span,
                        ));
                    }
                }
            }
            Stmt::If { cond, then_block, else_block, .. } => {
                let cond_ty = self.check_expr(cond);
                if !matches!(cond_ty, Type::Bool | Type::Unknown) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: "Bool".into(),
                            got: cond_ty.display_name(),
                            context: "`if` condition".into(),
                        },
                        cond.span(),
                    ));
                }
                let entry_frontier = self.effect_frontier;
                let entry_weak_refresh = self.weak_refresh.clone();

                self.effect_frontier = entry_frontier;
                self.weak_refresh = entry_weak_refresh.clone();
                self.check_block(then_block);
                let then_frontier = self.effect_frontier;
                let then_refresh = self.weak_refresh.clone();

                let (else_frontier, else_refresh) = if let Some(b) = else_block {
                    self.effect_frontier = entry_frontier;
                    self.weak_refresh = entry_weak_refresh.clone();
                    self.check_block(b);
                    (self.effect_frontier, self.weak_refresh.clone())
                } else {
                    (entry_frontier, entry_weak_refresh.clone())
                };

                self.effect_frontier = then_frontier.merge_max(else_frontier);
                self.weak_refresh = self.merge_weak_refresh(
                    &entry_weak_refresh,
                    &then_refresh,
                    &else_refresh,
                );
            }
            Stmt::Yield { value, span } => {
                let yielded = self.check_expr(value);
                if !self.in_agent_body {
                    self.errors
                        .push(TypeError::new(TypeErrorKind::YieldOutsideAgent, *span));
                    return;
                }
                match self.current_return.as_ref() {
                    Some(Type::Stream(inner)) => {
                        self.saw_yield = true;
                        if !yielded.is_assignable_to(inner) {
                            self.errors.push(TypeError::new(
                                TypeErrorKind::YieldReturnTypeMismatch {
                                    expected: inner.display_name(),
                                    got: yielded.display_name(),
                                },
                                value.span(),
                            ));
                        }
                    }
                    Some(other) => {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::YieldRequiresStreamReturn {
                                declared: other.display_name(),
                            },
                            *span,
                        ));
                    }
                    None => {
                        self.errors
                            .push(TypeError::new(TypeErrorKind::YieldOutsideAgent, *span));
                    }
                }
            }
            Stmt::For { var, iter, body, .. } => {
                let iter_ty = self.check_expr(iter);
                // Derive the loop variable's type from the iterable.
                // Lists iterate their element type; Strings iterate
                // chars (which Corvid currently models as String).
                let var_ty = match &iter_ty {
                    Type::List(elem) => (**elem).clone(),
                    Type::Stream(elem) => (**elem).clone(),
                    Type::String => Type::String,
                    Type::Unknown => Type::Unknown,
                    _other => Type::Unknown,
                };
                if let Some(Binding::Local(local_id)) = self.bindings.get(&var.span) {
                    self.local_types.insert(*local_id, var_ty);
                }
                let entry_frontier = self.effect_frontier;
                let entry_weak_refresh = self.weak_refresh.clone();
                self.check_block(body);
                let body_frontier = self.effect_frontier;
                let body_refresh = self.weak_refresh.clone();
                self.effect_frontier = entry_frontier.merge_max(body_frontier);
                self.weak_refresh = self.merge_weak_refresh(
                    &entry_weak_refresh,
                    &entry_weak_refresh,
                    &body_refresh,
                );
            }
            Stmt::Approve { action, .. } => {
                self.check_approve(action);
                self.bump_effect(WeakEffect::Approve);
            }
            Stmt::Expr { expr, .. } => {
                let _ = self.check_expr(expr);
            }
        }
    }

    fn check_approve(&mut self, action: &Expr) {
        if let Expr::Call { callee, args, .. } = action {
            if let Expr::Ident { name, .. } = &**callee {
                self.approvals.push(Approval {
                    label: name.name.clone(),
                    arity: args.len(),
                });
            }
            // Always typecheck the args themselves for binding validity.
            for arg in args {
                let _ = self.check_expr(arg);
            }
        } else {
            let _ = self.check_expr(action);
        }
    }

    // ------------------------------------------------------------
    // Expressions.
    // ------------------------------------------------------------

    fn check_expr(&mut self, e: &Expr) -> Type {
        self.check_expr_as(e, None)
    }

    fn check_expr_as(&mut self, e: &Expr, expected: Option<&Type>) -> Type {
        let ty = match e {
            Expr::Literal { value, .. } => match value {
                Literal::Int(_) => Type::Int,
                Literal::Float(_) => Type::Float,
                Literal::String(_) => Type::String,
                Literal::Bool(_) => Type::Bool,
                Literal::Nothing => Type::Nothing,
            },
            Expr::Ident { name, .. } => self.type_of_ident(name),
            Expr::Call { callee, args, span } => {
                self.check_call(callee, args, *span, expected)
            }
            Expr::FieldAccess { target, field, span } => self.check_field(target, field, *span),
            Expr::Index { target, index, span } => {
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
            Expr::BinOp { op, left, right, span } => self.check_binop(*op, left, right, *span),
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
                            || matches!(elem_ty, Type::Float)
                                && matches!(item_ty, Type::Int))
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
        };
        self.types.insert(e.span(), ty.clone());
        ty
    }

    fn type_of_ident(&mut self, id: &Ident) -> Type {
        let Some(binding) = self.bindings.get(&id.span) else {
            // Could be the resolver-skipped callee of an approve label —
            // the approve path handles that; in other contexts we give up
            // gracefully to avoid cascading errors.
            return Type::Unknown;
        };
        match binding {
            Binding::Local(lid) => self
                .local_types
                .get(lid)
                .cloned()
                .unwrap_or(Type::Unknown),
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
                | BuiltIn::WeakUpgrade => {
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
    fn type_of_decl(&mut self, id: DefId, ident: &Ident) -> Type {
        let entry = self.symbols.get(id);
        match entry.kind {
            DeclKind::Tool | DeclKind::Prompt | DeclKind::Agent => {
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
            DeclKind::Import | DeclKind::Eval | DeclKind::Effect => Type::Unknown,
        }
    }

    fn check_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        span: Span,
        expected: Option<&Type>,
    ) -> Type {
        // A callee of shape `target.field` is a method call.
        // Lower it: typecheck the receiver, look up the method by
        // (receiver_type_def_id, method_name), validate args (with
        // the receiver implicitly prepended), reuse the appropriate
        // tool / prompt / agent dispatch path.
        if let Expr::FieldAccess { target, field, .. } = callee {
            return self.check_method_call(target, field, args, span);
        }

        // Identify what's being called by looking at the callee's binding.
        let Expr::Ident { name, .. } = callee else {
            // Indirect or chained callee — typecheck args and give up.
            for a in args {
                let _ = self.check_expr(a);
            }
            return Type::Unknown;
        };

        let Some(binding) = self.bindings.get(&name.span) else {
            // Unresolved callee (e.g. approve label encountered outside an
            // approve — shouldn't happen for well-formed code). Typecheck args.
            for a in args {
                let _ = self.check_expr(a);
            }
            return Type::Unknown;
        };

        match binding {
            Binding::Decl(def_id) => {
                let def_id = *def_id;
                let entry = self.symbols.get(def_id);
                match entry.kind {
                    DeclKind::Tool => self.check_tool_call(def_id, &name.name, args, span),
                    DeclKind::Prompt => self.check_prompt_call(def_id, &name.name, args),
                    DeclKind::Agent => self.check_agent_call(def_id, &name.name, args),
                    DeclKind::Import | DeclKind::Eval | DeclKind::Effect => {
                        for a in args {
                            let _ = self.check_expr(a);
                        }
                        Type::Unknown
                    }
                    DeclKind::Type => self.check_struct_constructor(def_id, &name.name, args),
                }
            }
            Binding::BuiltIn(builtin) => {
                self.check_builtin_constructor_call(*builtin, name, args, expected)
            }
            Binding::Local(_) => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::NotCallable {
                        got: "<local value>".into(),
                    },
                    callee.span(),
                ));
                for a in args {
                    let _ = self.check_expr(a);
                }
                Type::Unknown
            }
        }
    }

    fn check_tool_call(
        &mut self,
        def_id: DefId,
        tool_name: &str,
        args: &[Expr],
        span: Span,
    ) -> Type {
        let tool = *self
            .tools_by_id
            .get(&def_id)
            .expect("tool DefId not indexed");

        self.check_args_against_params(tool_name, &tool.params, args);

        // Effect check: dangerous tool must have a prior matching approve.
        if matches!(tool.effect, Effect::Dangerous) {
            let authorized = self
                .approvals
                .iter()
                .any(|a| snake_case(&a.label) == tool_name && a.arity == args.len());
            if !authorized {
                self.errors.push(TypeError::new(
                    TypeErrorKind::UnapprovedDangerousCall {
                        tool: tool_name.to_string(),
                        expected_approve_label: pascal_case(tool_name),
                        arity: args.len(),
                    },
                    span,
                ));
            }
        }

        self.bump_effect(WeakEffect::ToolCall);
        self.type_ref_to_type(&tool.return_ty)
    }

    fn check_prompt_call(
        &mut self,
        def_id: DefId,
        name: &str,
        args: &[Expr],
    ) -> Type {
        let prompt = *self
            .prompts_by_id
            .get(&def_id)
            .expect("prompt DefId not indexed");
        self.check_args_against_params(name, &prompt.params, args);
        self.bump_effect(WeakEffect::Llm);
        self.type_ref_to_type(&prompt.return_ty)
    }

    fn check_agent_call(
        &mut self,
        def_id: DefId,
        name: &str,
        args: &[Expr],
    ) -> Type {
        let agent = *self
            .agents_by_id
            .get(&def_id)
            .expect("agent DefId not indexed");
        self.check_args_against_params(name, &agent.params, args);
        self.bump_effect(WeakEffect::ToolCall);
        self.bump_effect(WeakEffect::Llm);
        self.bump_effect(WeakEffect::Approve);
        self.type_ref_to_type(&agent.return_ty)
    }

    fn check_builtin_constructor_call(
        &mut self,
        builtin: BuiltIn,
        name: &Ident,
        args: &[Expr],
        expected: Option<&Type>,
    ) -> Type {
        match builtin {
            BuiltIn::Ok => {
                if args.len() != 1 {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::ArityMismatch {
                            callee: name.name.clone(),
                            expected: 1,
                            got: args.len(),
                        },
                        name.span,
                    ));
                    for arg in args {
                        let _ = self.check_expr(arg);
                    }
                    return Type::Result(Box::new(Type::Unknown), Box::new(Type::Unknown));
                }
                let ok_ty = self.check_expr(&args[0]);
                let err_ty = match &self.current_return {
                    Some(Type::Result(_, err)) => (**err).clone(),
                    _ => Type::Unknown,
                };
                Type::Result(Box::new(ok_ty), Box::new(err_ty))
            }
            BuiltIn::Err => {
                if args.len() != 1 {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::ArityMismatch {
                            callee: name.name.clone(),
                            expected: 1,
                            got: args.len(),
                        },
                        name.span,
                    ));
                    for arg in args {
                        let _ = self.check_expr(arg);
                    }
                    return Type::Result(Box::new(Type::Unknown), Box::new(Type::Unknown));
                }
                let err_ty = self.check_expr(&args[0]);
                let ok_ty = match &self.current_return {
                    Some(Type::Result(ok, _)) => (**ok).clone(),
                    _ => Type::Unknown,
                };
                Type::Result(Box::new(ok_ty), Box::new(err_ty))
            }
            BuiltIn::Some => {
                if args.len() != 1 {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::ArityMismatch {
                            callee: name.name.clone(),
                            expected: 1,
                            got: args.len(),
                        },
                        name.span,
                    ));
                    for arg in args {
                        let _ = self.check_expr(arg);
                    }
                    return Type::Option(Box::new(Type::Unknown));
                }
                let expected_inner = match expected {
                    Some(Type::Option(inner)) => Some(&**inner),
                    _ => None,
                };
                let inner_ty = self.check_expr_as(&args[0], expected_inner);
                let final_inner_ty = match expected_inner {
                    Some(exp) if inner_ty.is_assignable_to(exp) => exp.clone(),
                    _ => inner_ty,
                };
                Type::Option(Box::new(final_inner_ty))
            }
            BuiltIn::WeakNew => {
                if args.len() != 1 {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::ArityMismatch {
                            callee: name.name.clone(),
                            expected: 1,
                            got: args.len(),
                        },
                        name.span,
                    ));
                    for arg in args {
                        let _ = self.check_expr(arg);
                    }
                    return Type::Weak(Box::new(Type::Unknown), WeakEffectRow::any());
                }
                let target_ty = self.check_expr(&args[0]);
                if !is_weakable_type(&target_ty) && !matches!(target_ty, Type::Unknown) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::InvalidWeakNewTarget {
                            got: target_ty.display_name(),
                        },
                        args[0].span(),
                    ));
                }
                let row = match expected {
                    Some(Type::Weak(_, row)) => *row,
                    _ => WeakEffectRow::any(),
                };
                Type::Weak(Box::new(target_ty), row)
            }
            BuiltIn::WeakUpgrade => {
                if args.len() != 1 {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::ArityMismatch {
                            callee: name.name.clone(),
                            expected: 1,
                            got: args.len(),
                        },
                        name.span,
                    ));
                    for arg in args {
                        let _ = self.check_expr(arg);
                    }
                    return Type::Option(Box::new(Type::Unknown));
                }
                let weak_ty = self.check_expr(&args[0]);
                let refreshed_at = self.refresh_frontier_for_expr(&args[0], &weak_ty);
                match weak_ty {
                    Type::Weak(inner, row) => {
                        let invalidating = self
                            .effect_frontier
                            .invalidating_effects_since(&refreshed_at, row);
                        if !invalidating.is_empty() {
                            self.errors.push(TypeError::new(
                                TypeErrorKind::WeakUpgradeAcrossEffects {
                                    effects: invalidating,
                                },
                                args[0].span(),
                            ));
                        } else {
                            self.refresh_after_upgrade(&args[0]);
                        }
                        Type::Option(inner)
                    }
                    Type::Unknown => Type::Option(Box::new(Type::Unknown)),
                    other => {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::InvalidWeakUpgradeTarget {
                                got: other.display_name(),
                            },
                            args[0].span(),
                        ));
                        Type::Option(Box::new(Type::Unknown))
                    }
                }
            }
            BuiltIn::None => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::NotCallable {
                        got: "Option".into(),
                    },
                    name.span,
                ));
                for arg in args {
                    let _ = self.check_expr(arg);
                }
                Type::Unknown
            }
            _ => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::NotCallable {
                        got: name.name.clone(),
                    },
                    name.span,
                ));
                for arg in args {
                    let _ = self.check_expr(arg);
                }
                Type::Unknown
            }
        }
    }

    /// `target.method(args)` rewritten to a regular
    /// function call with the receiver as the first argument. The
    /// receiver's type is looked up in the methods side-table to
    /// pick the matching method DefId; from there we reuse the
    /// existing tool / prompt / agent dispatch.
    ///
    /// Errors:
    ///   - receiver isn't a struct (no methods on built-ins yet).
    ///   - method name doesn't exist on the type.
    ///   - arity mismatch (argv vs declared params, accounting for
    ///     receiver-as-first-param).
    fn check_method_call(
        &mut self,
        target: &Expr,
        method_name: &Ident,
        args: &[Expr],
        span: Span,
    ) -> Type {
        // 1. Typecheck the receiver and require a struct type.
        let recv_ty = self.check_expr(target);
        let recv_def_id = match recv_ty {
            Type::Struct(id) => id,
            other => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::NotCallable {
                        got: format!(
                            "method `{}` on receiver of type `{}` — methods currently work only on user-declared struct types. Built-in receiver methods are not implemented yet.",
                            method_name.name,
                            other.display_name()
                        ),
                    },
                    target.span(),
                ));
                // Still typecheck remaining args for diagnostics.
                for a in args {
                    let _ = self.check_expr(a);
                }
                return Type::Unknown;
            }
        };

        // 2. Look up the method.
        let method = match self
            .methods
            .get(&recv_def_id)
            .and_then(|m| m.get(&method_name.name))
        {
            Some(m) => m.clone(),
            None => {
                let type_name = self.symbols.get(recv_def_id).name.clone();
                self.errors.push(TypeError::new(
                    TypeErrorKind::NotCallable {
                        got: format!(
                            "no method `{}` on type `{type_name}`",
                            method_name.name
                        ),
                    },
                    method_name.span,
                ));
                for a in args {
                    let _ = self.check_expr(a);
                }
                return Type::Unknown;
            }
        };

        // 3. Build the effective argument list: receiver prepended.
        //    Then dispatch by method kind, reusing the existing
        //    free-call paths.
        let mut effective_args: Vec<Expr> = Vec::with_capacity(args.len() + 1);
        effective_args.push(target.clone());
        effective_args.extend_from_slice(args);

        match method.kind {
            MethodKind::Tool => self.check_tool_call(
                method.def_id,
                &method_name.name,
                &effective_args,
                span,
            ),
            MethodKind::Prompt => {
                self.check_prompt_call(method.def_id, &method_name.name, &effective_args)
            }
            MethodKind::Agent => {
                self.check_agent_call(method.def_id, &method_name.name, &effective_args)
            }
        }
    }

    /// `TypeName(field0, field1, ...)` — construct a struct. Field
    /// values must be assignable to each field's declared type.
    /// Returns `Struct(def_id)`.
    fn check_struct_constructor(&mut self, def_id: DefId, name: &str, args: &[Expr]) -> Type {
        let ty_decl = *self
            .types_by_id
            .get(&def_id)
            .expect("type DefId not indexed");

        if args.len() != ty_decl.fields.len() {
            self.errors.push(TypeError::new(
                TypeErrorKind::ArityMismatch {
                    callee: name.to_string(),
                    expected: ty_decl.fields.len(),
                    got: args.len(),
                },
                args.first().map(|a| a.span()).unwrap_or(ty_decl.span),
            ));
        }
        for (i, arg) in args.iter().enumerate() {
            if let Some(field) = ty_decl.fields.get(i) {
                let field_ty = self.type_ref_to_type(&field.ty);
                let arg_ty = self.check_expr_as(arg, Some(&field_ty));
                if !arg_ty.is_assignable_to(&field_ty) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: field_ty.display_name(),
                            got: arg_ty.display_name(),
                            context: format!("field `{}` of `{name}`", field.name.name),
                        },
                        arg.span(),
                    ));
                }
            } else {
                let _ = self.check_expr(arg);
            }
        }
        Type::Struct(def_id)
    }

    fn check_args_against_params(
        &mut self,
        callee_name: &str,
        params: &[Param],
        args: &[Expr],
    ) {
        if params.len() != args.len() {
            self.errors.push(TypeError::new(
                TypeErrorKind::ArityMismatch {
                    callee: callee_name.to_string(),
                    expected: params.len(),
                    got: args.len(),
                },
                args.first()
                    .map(|a| a.span())
                    .unwrap_or(Span::new(0, 0)),
            ));
        }
        for (i, arg) in args.iter().enumerate() {
            if let Some(param) = params.get(i) {
                let param_ty = self.type_ref_to_type(&param.ty);
                let arg_ty = self.check_expr_as(arg, Some(&param_ty));
                if !arg_ty.is_assignable_to(&param_ty) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: param_ty.display_name(),
                            got: arg_ty.display_name(),
                            context: format!(
                                "argument {} to `{callee_name}`",
                                i + 1
                            ),
                        },
                        arg.span(),
                    ));
                }
            } else {
                let _ = self.check_expr(arg);
            }
        }
    }

    fn bump_effect(&mut self, effect: WeakEffect) {
        self.effect_frontier = self.effect_frontier.bumped(effect);
    }

    fn update_weak_local_on_assignment(
        &mut self,
        local_id: LocalId,
        value: &Expr,
        local_ty: &Type,
    ) {
        match local_ty {
            Type::Weak(_, _) => {
                let refreshed = self.refresh_frontier_for_expr(value, local_ty);
                self.weak_refresh.insert(local_id, refreshed);
            }
            _ => {
                self.weak_refresh.remove(&local_id);
            }
        }
    }

    fn refresh_frontier_for_expr(&self, expr: &Expr, ty: &Type) -> EffectFrontier {
        match expr {
            Expr::Ident { name, .. } => match self.bindings.get(&name.span) {
                Some(Binding::Local(local_id)) if matches!(ty, Type::Weak(_, _)) => self
                    .weak_refresh
                    .get(local_id)
                    .copied()
                    .unwrap_or(self.effect_frontier),
                _ => self.effect_frontier,
            },
            _ => self.effect_frontier,
        }
    }

    fn refresh_after_upgrade(&mut self, expr: &Expr) {
        if let Expr::Ident { name, .. } = expr {
            if let Some(Binding::Local(local_id)) = self.bindings.get(&name.span) {
                self.weak_refresh.insert(*local_id, self.effect_frontier);
            }
        }
    }

    fn merge_weak_refresh(
        &self,
        entry: &HashMap<LocalId, EffectFrontier>,
        left: &HashMap<LocalId, EffectFrontier>,
        right: &HashMap<LocalId, EffectFrontier>,
    ) -> HashMap<LocalId, EffectFrontier> {
        let mut merged = HashMap::new();
        for (local_id, ty) in &self.local_types {
            if !matches!(ty, Type::Weak(_, _)) {
                continue;
            }
            let entry_state = entry.get(local_id).copied().unwrap_or_default();
            let left_state = left.get(local_id).copied().unwrap_or(entry_state);
            let right_state = right.get(local_id).copied().unwrap_or(entry_state);
            merged.insert(*local_id, left_state.meet_min(right_state));
        }
        merged
    }

    fn check_field(&mut self, target: &Expr, field: &Ident, span: Span) -> Type {
        let target_ty = self.check_expr(target);
        match &target_ty {
            Type::Struct(def_id) => {
                let type_decl = *self
                    .types_by_id
                    .get(def_id)
                    .expect("struct DefId not indexed");
                if let Some(f) = type_decl
                    .fields
                    .iter()
                    .find(|f| f.name.name == field.name)
                {
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

    fn check_binop(&mut self, op: BinaryOp, l: &Expr, r: &Expr, _span: Span) -> Type {
        let lt = self.check_expr(l);
        let rt = self.check_expr(r);
        use BinaryOp::*;
        match op {
            // `+` is overloaded: numeric addition OR string concatenation.
            Add => match (&lt, &rt) {
                (Type::Int, Type::Int) => Type::Int,
                (Type::Float, Type::Float)
                | (Type::Int, Type::Float)
                | (Type::Float, Type::Int) => Type::Float,
                (Type::String, Type::String) => Type::String,
                (Type::Unknown, _) | (_, Type::Unknown) => Type::Unknown,
                (a, b) => {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: "Int, Float, or two Strings".into(),
                            got: format!("{} and {}", a.display_name(), b.display_name()),
                            context: "`+` operator".into(),
                        },
                        l.span().merge(r.span()),
                    ));
                    Type::Unknown
                }
            },
            Sub | Mul | Div | Mod => match (&lt, &rt) {
                (Type::Int, Type::Int) => Type::Int,
                (Type::Float, Type::Float)
                | (Type::Int, Type::Float)
                | (Type::Float, Type::Int) => Type::Float,
                (Type::Unknown, _) | (_, Type::Unknown) => Type::Unknown,
                (a, b) => {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: "Int or Float".into(),
                            got: format!("{} and {}", a.display_name(), b.display_name()),
                            context: "arithmetic operator".into(),
                        },
                        l.span().merge(r.span()),
                    ));
                    Type::Unknown
                }
            },
            Eq | NotEq | Lt | LtEq | Gt | GtEq => {
                if !lt.is_assignable_to(&rt) && !rt.is_assignable_to(&lt) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: lt.display_name(),
                            got: rt.display_name(),
                            context: "comparison".into(),
                        },
                        l.span().merge(r.span()),
                    ));
                }
                Type::Bool
            }
            And | Or => {
                if !matches!(lt, Type::Bool | Type::Unknown) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: "Bool".into(),
                            got: lt.display_name(),
                            context: "logical operator".into(),
                        },
                        l.span(),
                    ));
                }
                if !matches!(rt, Type::Bool | Type::Unknown) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: "Bool".into(),
                            got: rt.display_name(),
                            context: "logical operator".into(),
                        },
                        r.span(),
                    ));
                }
                Type::Bool
            }
        }
    }

    fn check_unop(&mut self, op: UnaryOp, operand: &Expr) -> Type {
        let t = self.check_expr(operand);
        match op {
            UnaryOp::Neg => match t {
                Type::Int => Type::Int,
                Type::Float => Type::Float,
                Type::Unknown => Type::Unknown,
                other => {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: "Int or Float".into(),
                            got: other.display_name(),
                            context: "unary `-`".into(),
                        },
                        operand.span(),
                    ));
                    Type::Unknown
                }
            },
            UnaryOp::Not => match t {
                Type::Bool | Type::Unknown => Type::Bool,
                other => {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeMismatch {
                            expected: "Bool".into(),
                            got: other.display_name(),
                            context: "unary `not`".into(),
                        },
                        operand.span(),
                    ));
                    Type::Bool
                }
            },
        }
    }

    fn check_try_propagate(&mut self, inner: &Expr, span: Span) -> Type {
        let inner_ty = self.check_expr(inner);
        match inner_ty {
            Type::Result(ok, err) => {
                self.ensure_try_return_context(
                    &Type::Result(Box::new(Type::Unknown), err.clone()),
                    span,
                );
                (*ok).clone()
            }
            Type::Option(inner) => {
                self.ensure_try_return_context(&Type::Option(Box::new(Type::Unknown)), span);
                (*inner).clone()
            }
            Type::Unknown => Type::Unknown,
            other => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::InvalidTryPropagate {
                        got: other.display_name(),
                    },
                    span,
                ));
                Type::Unknown
            }
        }
    }

    fn check_try_retry(&mut self, body: &Expr, span: Span) -> Type {
        let body_ty = self.check_expr(body);
        match body_ty {
            Type::Result(_, _) | Type::Option(_) | Type::Stream(_) | Type::Unknown => body_ty,
            other => {
                self.errors.push(TypeError::new(
                    TypeErrorKind::InvalidRetryTarget {
                        got: other.display_name(),
                    },
                    span,
                ));
                Type::Unknown
            }
        }
    }

    fn ensure_try_return_context(&mut self, required: &Type, span: Span) {
        match &self.current_return {
            Some(current) if required.is_assignable_to(current) => {}
            Some(current) => self.errors.push(TypeError::new(
                TypeErrorKind::TryPropagateReturnMismatch {
                    expected: required.display_name(),
                    got: current.display_name(),
                },
                span,
            )),
            None => self.errors.push(TypeError::new(
                TypeErrorKind::TryPropagateReturnMismatch {
                    expected: required.display_name(),
                    got: "no enclosing return type".into(),
                },
                span,
            )),
        }
    }

    // ------------------------------------------------------------
    // Type-reference resolution (TypeRef → Type).
    // ------------------------------------------------------------

    fn type_ref_to_type(&mut self, tr: &TypeRef) -> Type {
        match tr {
            TypeRef::Named { name, .. } => self.named_type_to_type(&name.name),
            TypeRef::Generic { name, args, span } => match name.name.as_str() {
                "List" => {
                    if args.len() != 1 {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::GenericArityMismatch {
                                name: name.name.clone(),
                                expected: 1,
                                got: args.len(),
                            },
                            *span,
                        ));
                        return Type::Unknown;
                    }
                    Type::List(Box::new(self.type_ref_to_type(&args[0])))
                }
                "Stream" => {
                    if args.len() != 1 {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::GenericArityMismatch {
                                name: name.name.clone(),
                                expected: 1,
                                got: args.len(),
                            },
                            *span,
                        ));
                        return Type::Unknown;
                    }
                    Type::Stream(Box::new(self.type_ref_to_type(&args[0])))
                }
                "Option" => {
                    if args.len() != 1 {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::GenericArityMismatch {
                                name: name.name.clone(),
                                expected: 1,
                                got: args.len(),
                            },
                            *span,
                        ));
                        return Type::Unknown;
                    }
                    Type::Option(Box::new(self.type_ref_to_type(&args[0])))
                }
                "Result" => {
                    if args.len() != 2 {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::GenericArityMismatch {
                                name: name.name.clone(),
                                expected: 2,
                                got: args.len(),
                            },
                            *span,
                        ));
                        return Type::Unknown;
                    }
                    Type::Result(
                        Box::new(self.type_ref_to_type(&args[0])),
                        Box::new(self.type_ref_to_type(&args[1])),
                    )
                }
                "Grounded" => {
                    if args.len() != 1 {
                        self.errors.push(TypeError::new(
                            TypeErrorKind::GenericArityMismatch {
                                name: name.name.clone(),
                                expected: 1,
                                got: args.len(),
                            },
                            *span,
                        ));
                        return Type::Unknown;
                    }
                    Type::Grounded(Box::new(self.type_ref_to_type(&args[0])))
                }
                _ => Type::Unknown,
            },
            TypeRef::Weak { inner, effects, span } => {
                let inner_ty = self.type_ref_to_type(inner);
                if !is_weakable_type(&inner_ty) && !matches!(inner_ty, Type::Unknown) {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::InvalidWeakTargetType {
                            got: inner_ty.display_name(),
                        },
                        *span,
                    ));
                    return Type::Weak(Box::new(Type::Unknown), effects.unwrap_or_else(WeakEffectRow::any));
                }
                Type::Weak(
                    Box::new(inner_ty),
                    effects.unwrap_or_else(WeakEffectRow::any),
                )
            }
            TypeRef::Function { .. } => Type::Unknown,
        }
    }

    fn named_type_to_type(&self, name: &str) -> Type {
        match name {
            "Int" => Type::Int,
            "Float" => Type::Float,
            "String" => Type::String,
            "Bool" => Type::Bool,
            "Nothing" => Type::Nothing,
            _ => match self.symbols.lookup_def(name) {
                Some(id) => Type::Struct(id),
                None => Type::Unknown,
            },
        }
    }
}

fn is_weakable_type(ty: &Type) -> bool {
    matches!(ty, Type::String | Type::Struct(_) | Type::List(_))
}

// ------------------------------------------------------------
// String-case helpers for approve-label matching.
// ------------------------------------------------------------

fn pascal_case(snake: &str) -> String {
    let mut out = String::new();
    let mut cap_next = true;
    for c in snake.chars() {
        if c == '_' {
            cap_next = true;
            continue;
        }
        if cap_next {
            out.extend(c.to_uppercase());
            cap_next = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn snake_case(pascal: &str) -> String {
    let mut out = String::new();
    for (i, c) in pascal.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod case_tests {
    use super::*;

    #[test]
    fn snake_and_pascal_are_inverses() {
        assert_eq!(pascal_case("issue_refund"), "IssueRefund");
        assert_eq!(snake_case("IssueRefund"), "issue_refund");
        assert_eq!(pascal_case("send_email"), "SendEmail");
        assert_eq!(snake_case("SendEmail"), "send_email");
    }
}
