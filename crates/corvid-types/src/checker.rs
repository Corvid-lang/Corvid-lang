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
    FixtureDecl, Ident, Literal, MockDecl, ModelDecl, Param, PromptDecl, Span, Stmt, TestDecl,
    ToolDecl, TypeDecl, TypeRef, UnaryOp, WeakEffect, WeakEffectRow,
};
use corvid_resolve::{
    resolver::{MethodEntry, MethodKind},
    Binding, BuiltIn, DeclKind, DefId, LocalId, ReplayPatternBinding, Resolved, SymbolTable,
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
    /// Qualified calls that resolved across a `.cor` import boundary,
    /// keyed by the `alias.member` callee expression span. IR lowering
    /// consumes this to emit a direct call to the imported declaration
    /// instead of treating the field access as an indirect value.
    pub imported_calls: HashMap<Span, ImportedCallTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedCallTarget {
    pub module_path: String,
    pub def_id: DefId,
    pub name: String,
    pub kind: ImportedCallKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportedCallKind {
    Type,
    Tool,
    Prompt,
    Agent,
}

pub fn typecheck(file: &File, resolved: &Resolved) -> Checked {
    typecheck_with_config(file, resolved, None)
}

/// Typecheck `file` with an explicit cross-file module resolution.
/// Callers that have already loaded + resolved imported `.cor`
/// files via [`corvid_driver::build_module_resolution`] pass the
/// resulting [`ModuleResolution`] here so qualified type
/// references (`alias.TypeName`) can consult it. Callers without
/// imports should use the plain [`typecheck`] or
/// [`typecheck_with_config`] and get the same behavior as before.
pub fn typecheck_with_modules(
    file: &File,
    resolved: &Resolved,
    modules: &corvid_resolve::ModuleResolution,
) -> Checked {
    typecheck_with_everything(file, resolved, None, Some(modules))
}

/// Typecheck `file` with both an explicit `corvid.toml` configuration
/// and cross-file module resolution. Production file-backed compiles
/// use this path so custom dimensions and `import "./path" as alias`
/// semantics compose rather than one disabling the other.
pub fn typecheck_with_config_and_modules(
    file: &File,
    resolved: &Resolved,
    config: Option<&crate::config::CorvidConfig>,
    modules: &corvid_resolve::ModuleResolution,
) -> Checked {
    typecheck_with_everything(file, resolved, config, Some(modules))
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
    typecheck_with_everything(file, resolved, config, None)
}

fn typecheck_with_everything(
    file: &File,
    resolved: &Resolved,
    config: Option<&crate::config::CorvidConfig>,
    modules: Option<&corvid_resolve::ModuleResolution>,
) -> Checked {
    let mut c = Checker::new(file, resolved, modules);
    c.validate_import_use_items(file);
    c.check_file(file);

    let effect_decls: Vec<&corvid_ast::EffectDecl> = file.decls.iter().filter_map(|d| {
        if let Decl::Effect(e) = d { Some(e) } else { None }
    }).collect();
    for effect in &effect_decls {
        c.check_effect_decl_confidence(effect);
    }
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
        imported_calls: c.imported_calls,
    }
}

struct Checker<'a> {
    symbols: &'a SymbolTable,
    bindings: &'a HashMap<Span, Binding>,
    types: HashMap<Span, Type>,
    errors: Vec<TypeError>,
    warnings: Vec<TypeWarning>,
    imported_calls: HashMap<Span, ImportedCallTarget>,

    /// Indexed declarations for O(1) lookup by DefId. Methods from
    /// `extend` blocks get inserted here too — a method `extend Order: agent
    /// total(o: Order) -> Int` indexes into `agents_by_id` under the
    /// method's allocated DefId, alongside file-level free agents.
    tools_by_id: HashMap<DefId, &'a ToolDecl>,
    prompts_by_id: HashMap<DefId, &'a PromptDecl>,
    agents_by_id: HashMap<DefId, &'a AgentDecl>,
    fixtures_by_id: HashMap<DefId, &'a FixtureDecl>,
    types_by_id: HashMap<DefId, &'a TypeDecl>,
    models_by_id: HashMap<DefId, &'a ModelDecl>,

    /// Per-receiver-type method side-table from the
    /// resolver. Method calls (`x.foo(args)`) look up `x`'s declared
    /// type then this map to find the method's `DefId`, after which
    /// dispatch reuses the existing tool / prompt / agent call paths.
    methods: &'a HashMap<DefId, HashMap<String, MethodEntry>>,

    /// Replay-pattern side-table from the resolver. Gives the
    /// `DefId` (for prompt/tool resolutions) or the `Approve`
    /// marker for approval-label patterns, keyed by the pattern's
    /// own span. The checker uses it to compute capture types
    /// without re-resolving string literals.
    replay_pattern_bindings: &'a HashMap<Span, ReplayPatternBinding>,

    /// Cross-file module resolution populated by
    /// `corvid_driver::build_module_resolution`. When `None`, the
    /// checker falls back to single-file semantics and any
    /// `TypeRef::Qualified` yields a `CorvidImportNotYetResolved`
    /// error. When `Some`, qualified references to unknown aliases /
    /// private members / unknown members surface typed errors, and
    /// successful public type exports resolve to `Type::ImportedStruct`.
    module_resolution: Option<&'a corvid_resolve::ModuleResolution>,

    /// Type of each local binding, populated as we enter scopes.
    local_types: HashMap<LocalId, Type>,

    /// Declared return type of the currently-checked function-like.
    current_return: Option<Type>,
    in_agent_body: bool,
    in_test_body: bool,
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
    fn new(
        file: &'a File,
        resolved: &'a Resolved,
        module_resolution: Option<&'a corvid_resolve::ModuleResolution>,
    ) -> Self {
        let mut tools = HashMap::new();
        let mut prompts = HashMap::new();
        let mut agents = HashMap::new();
        let mut fixtures = HashMap::new();
        let mut types = HashMap::new();
        let mut models = HashMap::new();

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
                Decl::Fixture(f) => {
                    if let Some(id) = resolved.symbols.lookup_def(&f.name.name) {
                        fixtures.insert(id, f);
                    }
                }
                Decl::Eval(_) | Decl::Test(_) | Decl::Mock(_) => {}
                Decl::Type(t) => {
                    if let Some(id) = resolved.symbols.lookup_def(&t.name.name) {
                        types.insert(id, t);
                    }
                }
                Decl::Import(_) => {}
                Decl::Effect(_) => {}
                Decl::Model(m) => {
                    if let Some(id) = resolved.symbols.lookup_def(&m.name.name) {
                        models.insert(id, m);
                    }
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
            imported_calls: HashMap::new(),
            tools_by_id: tools,
            prompts_by_id: prompts,
            agents_by_id: agents,
            fixtures_by_id: fixtures,
            types_by_id: types,
            models_by_id: models,
            methods: &resolved.methods,
            replay_pattern_bindings: &resolved.replay_pattern_bindings,
            module_resolution,
            local_types: HashMap::new(),
            current_return: None,
            in_agent_body: false,
            in_test_body: false,
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
                Decl::Test(t) => self.check_test(t),
                Decl::Fixture(f) => self.check_fixture(f),
                Decl::Mock(m) => self.check_mock(m),
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



}

mod call;
mod case;
mod decl;
mod effect_decl;
mod expr;
mod import_call;
mod ops;
mod prompt;
mod stmt;
mod stream;
mod types;

use case::{pascal_case, snake_case};

fn is_weakable_type(ty: &Type) -> bool {
    matches!(
        ty,
        Type::String | Type::Struct(_) | Type::ImportedStruct(_) | Type::List(_)
    )
}
