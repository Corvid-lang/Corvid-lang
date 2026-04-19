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

fn file_top_span(file: &File) -> Span {
    file.span
}

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
    typecheck_with_config(file, resolved, None)
}

/// Typecheck `file`, consuming an optional `corvid.toml` configuration.
/// Custom dimensions declared under `[effect-system.dimensions.*]`
/// are merged into the `EffectRegistry` alongside the built-ins.
/// A malformed `corvid.toml` entry surfaces as an
/// `InvalidCustomDimension` type error at the file's top span.
pub fn typecheck_with_config(
    file: &File,
    resolved: &Resolved,
    config: Option<&crate::config::CorvidConfig>,
) -> Checked {
    let mut c = Checker::new(file, resolved);
    c.check_file(file);

    let effect_decls: Vec<&corvid_ast::EffectDecl> = file.decls.iter().filter_map(|d| {
        if let Decl::Effect(e) = d { Some(e) } else { None }
    }).collect();
    let owned_decls: Vec<corvid_ast::EffectDecl> = effect_decls.iter().cloned().cloned().collect();

    // Validate config-declared dimensions up-front so malformed entries
    // become surfaceable diagnostics instead of being swallowed by the
    // registry builder. The registry itself still silently skips
    // invalid entries — this is the user-facing channel.
    if let Some(cfg) = config {
        if let Err(err) = cfg.into_dimension_schemas() {
            let (dimension, message) = match &err {
                crate::config::DimensionConfigError::ParseError { message, .. } => {
                    (String::new(), message.clone())
                }
                crate::config::DimensionConfigError::UnknownComposition { dimension, .. }
                | crate::config::DimensionConfigError::UnknownType { dimension, .. }
                | crate::config::DimensionConfigError::BadDefault { dimension, .. }
                | crate::config::DimensionConfigError::CollidesWithBuiltin { dimension } => {
                    (dimension.clone(), err.to_string())
                }
            };
            let span = file_top_span(file);
            c.errors.push(TypeError::new(
                TypeErrorKind::InvalidCustomDimension { dimension, message },
                span,
            ));
        }
    }

    let registry = crate::effects::EffectRegistry::from_decls_with_config(&owned_decls, config);

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
                Decl::Model(_) => {
                    // Phase 20h slice A: models are catalog entries
                    // without a body; they contribute no typed
                    // values yet. Slice B wires them into prompt
                    // dispatch.
                }
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
                Decl::Tool(_)
                | Decl::Type(_)
                | Decl::Import(_)
                | Decl::Effect(_)
                | Decl::Model(_) => {}
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
            DeclKind::Import | DeclKind::Eval | DeclKind::Effect | DeclKind::Model => {
                Type::Unknown
            }
        }
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


}

mod call;
mod ops;
mod prompt;
mod stmt;
mod types;

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
