//! Two-pass name resolver for Corvid.
//!
//! Pass 1: collect every top-level declaration into the file's symbol
//! table. Duplicates are reported and only the first wins.
//!
//! Pass 2: walk the AST and record a `Binding` for every identifier use.
//! Undefined names are reported; resolution continues.

use crate::errors::{ResolveError, ResolveErrorKind};
use crate::scope::{Binding, DefId, DeclKind, LocalId, LocalScope, SymbolTable};
use corvid_ast::{
    AgentDecl, Block, Decl, EvalAssert, EvalDecl, Expr, ExtendDecl, ExtendMethodKind, File,
    Ident, PromptDecl, ReplayArm, ReplayPattern, Span, Stmt, ToolArgPattern, ToolDecl, TypeDecl,
    TypeRef, Visibility,
};
use std::collections::{HashMap, HashSet};

/// Output of name resolution. The AST itself is not mutated — bindings
/// live in a side table keyed by the span of each identifier use.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub symbols: SymbolTable,
    pub bindings: HashMap<Span, Binding>,
    pub errors: Vec<ResolveError>,
    /// Method side-table — per-receiver-type registry of
    /// methods declared in `extend T:` blocks. Outer key is the type's
    /// `DefId`; inner map is keyed by method name. Methods don't
    /// collide across types (`Point.distance` and `Line.distance`
    /// coexist) but must be unique within a single type.
    pub methods: HashMap<DefId, HashMap<String, MethodEntry>>,
    /// Replay-pattern side-table — for each `ReplayPattern` the
    /// resolver walked, either the `DefId` of the resolved
    /// prompt/tool or `Approve` for approval-label patterns. Keyed
    /// by the pattern's span so downstream passes (checker, IR)
    /// can look up the resolution without re-walking string
    /// literals. See [`ReplayPatternBinding`].
    pub replay_pattern_bindings: HashMap<Span, ReplayPatternBinding>,
}

/// Resolver-side handle for a replay pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayPatternBinding {
    /// `llm("<name>")` resolved to the prompt declaration with this `DefId`.
    Llm(DefId),
    /// `tool("<name>", ...)` resolved to the tool declaration with this `DefId`.
    Tool(DefId),
    /// `approve("<label>")` — labels have no `DefId`; the resolver
    /// only verifies the label is used somewhere in the file.
    Approve,
}

/// One method's resolution metadata. The actual method body lives in
/// the AST under `Decl::Extend(ext).methods[i]`; this entry is the
/// side-table indexable handle.
#[derive(Debug, Clone)]
pub struct MethodEntry {
    /// Fresh `DefId` allocated for this method. Distinct from any
    /// top-level decl's DefId because methods aren't in the file's
    /// by-name namespace (multiple types can share a method name).
    pub def_id: DefId,
    /// Tool / prompt / agent kind, mirroring the `ExtendMethodKind`
    /// at the AST level. Tells the typechecker which dispatch path
    /// to use when rewriting the call.
    pub kind: MethodKind,
    /// Visibility from the extend block. The current implementation stores it; later
    /// (package manager) gives it cross-file enforcement teeth.
    pub visibility: Visibility,
    /// Span of the declaration for diagnostics.
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    Tool,
    Prompt,
    Agent,
}

pub fn resolve(file: &File) -> Resolved {
    let mut r = Resolver::new();
    r.collect_decls(file);
    r.collect_methods(file);
    r.collect_approval_labels(file);
    r.resolve_file(file);
    Resolved {
        symbols: r.symbols,
        bindings: r.bindings,
        errors: r.errors,
        methods: r.methods,
        replay_pattern_bindings: r.replay_pattern_bindings,
    }
}

struct Resolver {
    symbols: SymbolTable,
    bindings: HashMap<Span, Binding>,
    errors: Vec<ResolveError>,
    /// One scope per enclosing function/agent/prompt/tool. v0.1 keeps it
    /// single-level inside a function body (Python-style: all block-local
    /// bindings share the function scope).
    scopes: Vec<LocalScope>,
    next_local_id: u32,
    /// Method side-table. Populated in `collect_methods` after
    /// `collect_decls` has run (so type DefIds are known before we look
    /// up `extend T:` targets).
    methods: HashMap<DefId, HashMap<String, MethodEntry>>,
    /// Replay-pattern side-table — populated as `resolve_expr` walks
    /// `Expr::Replay` blocks.
    replay_pattern_bindings: HashMap<Span, ReplayPatternBinding>,
    /// Set of approval labels that appear at `approve Label(...)`
    /// sites anywhere in the file. Populated by
    /// `collect_approval_labels` after `collect_decls` so
    /// `replay ... when approve("Label") -> ...` can be
    /// presence-checked without walking the file a second time.
    known_approval_labels: HashSet<String>,
}

impl Resolver {
    fn new() -> Self {
        Self {
            symbols: SymbolTable::new(),
            bindings: HashMap::new(),
            errors: Vec::new(),
            scopes: Vec::new(),
            next_local_id: 0,
            methods: HashMap::new(),
            replay_pattern_bindings: HashMap::new(),
            known_approval_labels: HashSet::new(),
        }
    }

    // ---------------- pass 1 ----------------

    fn collect_decls(&mut self, file: &File) {
        for decl in &file.decls {
            let (name, kind, span) = match decl {
                Decl::Import(i) => {
                    // An `import ... as alias` binds `alias`. Without an alias,
                    // we use the module name as the binding name (rough but
                    // serviceable for v0.1).
                    let name = i
                        .alias
                        .as_ref()
                        .map(|a| a.name.clone())
                        .unwrap_or_else(|| i.module.clone());
                    (name, DeclKind::Import, i.span)
                }
                Decl::Type(t) => (t.name.name.clone(), DeclKind::Type, t.span),
                Decl::Tool(t) => (t.name.name.clone(), DeclKind::Tool, t.span),
                Decl::Prompt(p) => (p.name.name.clone(), DeclKind::Prompt, p.span),
                Decl::Agent(a) => (a.name.name.clone(), DeclKind::Agent, a.span),
                Decl::Eval(e) => (e.name.name.clone(), DeclKind::Eval, e.span),
                Decl::Effect(e) => (e.name.name.clone(), DeclKind::Effect, e.span),
                Decl::Model(m) => (m.name.name.clone(), DeclKind::Model, m.span),
                Decl::Extend(_) => {
                    // The parser accepts `extend T:`
                    // blocks; method registration into a per-type
                    // method table lands in the next step. For now the
                    // extend decl contributes no top-level symbols
                    // (methods are scoped to the receiver type, not
                    // to the file's symbol namespace).
                    continue;
                }
            };
            if let Err(first_span) = self.symbols.declare(&name, kind, span) {
                self.errors.push(ResolveError {
                    kind: ResolveErrorKind::DuplicateDecl {
                        name,
                        first_span,
                    },
                    span,
                });
            }
        }
    }

    // ---------------- pass 1.5: collect methods ----------------

    /// Walk every `extend T:` block, validate the target is a known
    /// type, and register each contained method in the per-type method
    /// side-table. Runs after `collect_decls` so type DefIds are
    /// already in the symbol table — we look them up by name.
    fn collect_methods(&mut self, file: &File) {
        for decl in &file.decls {
            let Decl::Extend(ext) = decl else { continue };
            self.collect_one_extend(ext, file);
        }
    }

    fn collect_one_extend(&mut self, ext: &ExtendDecl, file: &File) {
        // 1. Resolve the target type by name. Must exist + be a Type.
        let type_def_id = match self.symbols.lookup_def(&ext.type_name.name) {
            Some(id) => {
                let entry = self.symbols.get(id);
                if entry.kind != DeclKind::Type {
                    self.errors.push(ResolveError {
                        kind: ResolveErrorKind::ExtendTargetNotAType(ext.type_name.name.clone()),
                        span: ext.type_name.span,
                    });
                    return;
                }
                id
            }
            None => {
                self.errors.push(ResolveError {
                    kind: ResolveErrorKind::ExtendTargetNotAType(ext.type_name.name.clone()),
                    span: ext.type_name.span,
                });
                return;
            }
        };

        // 2. Build a quick set of field names on this type so we can
        //    catch method/field collisions cheaply. Source of truth
        //    is the AST `TypeDecl` we find by walking `file.decls`
        //    once. v0.1 has no per-type field index in the resolver;
        //    This stays cheap rather than building one for one
        //    use site.
        let type_decl = file.decls.iter().find_map(|d| match d {
            Decl::Type(t) if t.name.name == ext.type_name.name => Some(t),
            _ => None,
        });
        let field_spans: HashMap<&str, Span> = type_decl
            .map(|t| {
                t.fields
                    .iter()
                    .map(|f| (f.name.name.as_str(), f.span))
                    .collect()
            })
            .unwrap_or_default();

        // 3. Walk the methods. Each one allocates a fresh DefId and
        //    lands in the per-type method-name table.
        let entry_table = self.methods.entry(type_def_id).or_default();
        for method in &ext.methods {
            let name = method.name().name.clone();
            let span = method.span();

            // Collision: method vs field on same type.
            if let Some(&field_span) = field_spans.get(name.as_str()) {
                self.errors.push(ResolveError {
                    kind: ResolveErrorKind::MethodFieldCollision {
                        type_name: ext.type_name.name.clone(),
                        method_name: name,
                        field_span,
                    },
                    span,
                });
                continue;
            }

            // Collision: duplicate method on same type.
            if let Some(existing) = entry_table.get(&name) {
                self.errors.push(ResolveError {
                    kind: ResolveErrorKind::DuplicateMethod {
                        type_name: ext.type_name.name.clone(),
                        method_name: name,
                        first_span: existing.span,
                    },
                    span,
                });
                continue;
            }

            let kind = match &method.kind {
                ExtendMethodKind::Tool(_) => MethodKind::Tool,
                ExtendMethodKind::Prompt(_) => MethodKind::Prompt,
                ExtendMethodKind::Agent(_) => MethodKind::Agent,
            };
            // Allocate a DefId scoped to the method side-table —
            // intentionally NOT in the file's by-name namespace
            // (multiple types can share method names).
            let decl_kind = match kind {
                MethodKind::Tool => DeclKind::Tool,
                MethodKind::Prompt => DeclKind::Prompt,
                MethodKind::Agent => DeclKind::Agent,
            };
            let def_id = self.symbols.allocate_def(&name, decl_kind, span);
            entry_table.insert(
                name,
                MethodEntry {
                    def_id,
                    kind,
                    visibility: method.visibility.clone(),
                    span,
                },
            );
        }
    }

    // ---------------- pass 2 ----------------

    fn resolve_file(&mut self, file: &File) {
        for decl in &file.decls {
            match decl {
                Decl::Import(_) => {}
                Decl::Type(t) => self.resolve_type_decl(t),
                Decl::Tool(t) => self.resolve_tool_decl(t),
                Decl::Prompt(p) => self.resolve_prompt_decl(p),
                Decl::Agent(a) => self.resolve_agent_decl(a),
                Decl::Eval(e) => self.resolve_eval_decl(e),
                Decl::Effect(e) => self.resolve_effect_decl(e),
                Decl::Model(_) => {
                    // Phase 20h slice A: model decls register a name
                    // in the symbol table and carry a key-value field
                    // map. The fields don't reference anything in
                    // scope (values are literals — strings, numbers,
                    // cost literals, bools, or names treated as
                    // enum-ish tags), so resolution is a no-op beyond
                    // the name registration done in `collect_decls`.
                    // Slice B wires the fields into capability /
                    // dimension validation; slice C wires `route:`
                    // clauses that reference model names.
                }
                Decl::Extend(ext) => {
                    // Resolve each method body
                    // the same way free agents/prompts/tools are
                    // resolved. Method bodies see the same scoping
                    // rules — there's no implicit `self` (the
                    // receiver is just the explicit first parameter,
                    // bound like any other param).
                    for method in &ext.methods {
                        match &method.kind {
                            ExtendMethodKind::Agent(a) => self.resolve_agent_decl(a),
                            ExtendMethodKind::Prompt(p) => self.resolve_prompt_decl(p),
                            ExtendMethodKind::Tool(t) => self.resolve_tool_decl(t),
                        }
                    }
                }
            }
        }
    }

    fn resolve_type_decl(&mut self, t: &TypeDecl) {
        for field in &t.fields {
            self.resolve_type_ref(&field.ty);
        }
    }

    fn resolve_effect_decl(&mut self, _e: &corvid_ast::EffectDecl) {
        // Dimension values are literals — no identifier resolution needed.
        // Future: if dimension values can reference types or other effects,
        // resolve those here.
    }

    fn resolve_effect_row(&mut self, row: &corvid_ast::EffectRow) {
        for effect_ref in &row.effects {
            match self.symbols.lookup(&effect_ref.name.name) {
                Some(Binding::Decl(id)) => {
                    let entry = self.symbols.get(id);
                    if entry.kind != DeclKind::Effect {
                        self.errors.push(ResolveError {
                            kind: ResolveErrorKind::UndefinedName(
                                effect_ref.name.name.clone(),
                            ),
                            span: effect_ref.span,
                        });
                    } else {
                        self.bindings.insert(effect_ref.name.span, Binding::Decl(id));
                    }
                }
                _ => {
                    self.errors.push(ResolveError {
                        kind: ResolveErrorKind::UndefinedName(
                            effect_ref.name.name.clone(),
                        ),
                        span: effect_ref.span,
                    });
                }
            }
        }
    }

    fn resolve_tool_decl(&mut self, t: &ToolDecl) {
        for p in &t.params {
            self.resolve_type_ref(&p.ty);
        }
        self.resolve_type_ref(&t.return_ty);
        self.resolve_effect_row(&t.effect_row);
    }

    fn resolve_prompt_decl(&mut self, p: &PromptDecl) {
        self.push_scope();
        for param in &p.params {
            self.resolve_type_ref(&param.ty);
            let id = self.fresh_local();
            self.current_scope_mut().insert(&param.name.name, id);
            self.bindings.insert(param.name.span, Binding::Local(id));
        }
        self.resolve_type_ref(&p.return_ty);
        self.resolve_effect_row(&p.effect_row);
        if let Some(model) = &p.stream.escalate_to {
            if let Some(def_id) = self.symbols.lookup_def(&model.name) {
                self.bindings.insert(model.span, Binding::Decl(def_id));
            } else {
                self.errors.push(ResolveError {
                    kind: ResolveErrorKind::UndefinedName(model.name.clone()),
                    span: model.span,
                });
            }
        }
        // Phase 20h slice C: resolve each `route:` arm in the prompt's
        // parameter scope. Guard expressions can reference the prompt's
        // params and any declaration in the file. Model idents must
        // bind to a `model` declaration; non-model bindings are left
        // for the type checker to report.
        if let Some(route) = &p.route {
            for arm in &route.arms {
                if let corvid_ast::RoutePattern::Guard(expr) = &arm.pattern {
                    self.resolve_expr(expr);
                }
                if let Some(def_id) = self.symbols.lookup_def(&arm.model.name) {
                    self.bindings
                        .insert(arm.model.span, Binding::Decl(def_id));
                } else {
                    self.errors.push(ResolveError {
                        kind: ResolveErrorKind::UndefinedName(arm.model.name.clone()),
                        span: arm.model.span,
                    });
                }
            }
        }
        // Phase 20h slice E: resolve each `progressive:` stage's
        // model ident. Thresholds are numeric literals (no resolution
        // needed). Unknown model names produce `UndefinedName`.
        if let Some(chain) = &p.progressive {
            for stage in &chain.stages {
                if let Some(def_id) = self.symbols.lookup_def(&stage.model.name) {
                    self.bindings
                        .insert(stage.model.span, Binding::Decl(def_id));
                } else {
                    self.errors.push(ResolveError {
                        kind: ResolveErrorKind::UndefinedName(stage.model.name.clone()),
                        span: stage.model.span,
                    });
                }
            }
        }
        // Phase 20h slice I: resolve the `rollout` variant + baseline.
        if let Some(spec) = &p.rollout {
            for ident in [&spec.variant, &spec.baseline] {
                if let Some(def_id) = self.symbols.lookup_def(&ident.name) {
                    self.bindings.insert(ident.span, Binding::Decl(def_id));
                } else {
                    self.errors.push(ResolveError {
                        kind: ResolveErrorKind::UndefinedName(ident.name.clone()),
                        span: ident.span,
                    });
                }
            }
        }
        // Phase 20h slice F: resolve each `ensemble [...]` model ident.
        if let Some(spec) = &p.ensemble {
            for model in &spec.models {
                if let Some(def_id) = self.symbols.lookup_def(&model.name) {
                    self.bindings.insert(model.span, Binding::Decl(def_id));
                } else {
                    self.errors.push(ResolveError {
                        kind: ResolveErrorKind::UndefinedName(model.name.clone()),
                        span: model.span,
                    });
                }
            }
        }
        // Phase 20h slice G: resolve the three `adversarial:` stages.
        if let Some(spec) = &p.adversarial {
            for ident in [&spec.proposer, &spec.challenger, &spec.adjudicator] {
                if let Some(def_id) = self.symbols.lookup_def(&ident.name) {
                    self.bindings.insert(ident.span, Binding::Decl(def_id));
                } else {
                    self.errors.push(ResolveError {
                        kind: ResolveErrorKind::UndefinedName(ident.name.clone()),
                        span: ident.span,
                    });
                }
            }
        }
        self.pop_scope();
    }

    fn resolve_agent_decl(&mut self, a: &AgentDecl) {
        self.push_scope();
        for param in &a.params {
            self.resolve_type_ref(&param.ty);
            let id = self.fresh_local();
            self.current_scope_mut().insert(&param.name.name, id);
            self.bindings.insert(param.name.span, Binding::Local(id));
        }
        self.resolve_type_ref(&a.return_ty);
        self.resolve_effect_row(&a.effect_row);
        self.resolve_block(&a.body);
        self.pop_scope();
    }

    fn resolve_eval_decl(&mut self, e: &EvalDecl) {
        self.push_scope();
        self.resolve_block(&e.body);
        for assertion in &e.assertions {
            self.resolve_eval_assert(assertion);
        }
        self.pop_scope();
    }

    fn resolve_eval_assert(&mut self, assertion: &EvalAssert) {
        match assertion {
            EvalAssert::Value { expr, .. } => self.resolve_expr(expr),
            EvalAssert::Called { tool, .. } => self.resolve_ident(tool),
            EvalAssert::Approved { .. } => {}
            EvalAssert::Cost { .. } => {}
            EvalAssert::Ordering { before, after, .. } => {
                self.resolve_ident(before);
                self.resolve_ident(after);
            }
        }
    }

    fn resolve_type_ref(&mut self, ty: &TypeRef) {
        match ty {
            TypeRef::Named { name, .. } => self.resolve_ident(name),
            TypeRef::Qualified { alias, .. } => self.resolve_ident(alias),
            TypeRef::Generic { name, args, .. } => {
                self.resolve_ident(name);
                for arg in args {
                    self.resolve_type_ref(arg);
                }
            }
            TypeRef::Weak { inner, .. } => {
                self.resolve_type_ref(inner);
            }
            TypeRef::Function { params, ret, .. } => {
                for p in params {
                    self.resolve_type_ref(p);
                }
                self.resolve_type_ref(ret);
            }
        }
    }

    fn resolve_block(&mut self, b: &Block) {
        for stmt in &b.stmts {
            self.resolve_stmt(stmt);
        }
    }

    fn resolve_stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let { name, ty, value, .. } => {
                if let Some(t) = ty {
                    self.resolve_type_ref(t);
                }
                // RHS is resolved *before* the LHS binding exists. This
                // mirrors Python semantics: `x = x + 1` reads the old `x`.
                self.resolve_expr(value);

                // Pythonic assignment: if `name` already exists in the
                // current function's scope, reuse its LocalId. That way
                // `total = total + x` in a loop mutates the same binding
                // across iterations instead of creating new ones.
                let id = match self
                    .scopes
                    .last()
                    .and_then(|s| s.lookup(&name.name))
                {
                    Some(existing) => existing,
                    None => {
                        let fresh = self.fresh_local();
                        self.current_scope_mut().insert(&name.name, fresh);
                        fresh
                    }
                };
                self.bindings.insert(name.span, Binding::Local(id));
            }
            Stmt::Return { value, .. } => {
                if let Some(e) = value {
                    self.resolve_expr(e);
                }
            }
            Stmt::Yield { value, .. } => self.resolve_expr(value),
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                self.resolve_expr(cond);
                self.resolve_block(then_block);
                if let Some(b) = else_block {
                    self.resolve_block(b);
                }
            }
            Stmt::For { var, iter, body, .. } => {
                self.resolve_expr(iter);
                let id = self.fresh_local();
                self.current_scope_mut().insert(&var.name, id);
                self.bindings.insert(var.span, Binding::Local(id));
                self.resolve_block(body);
            }
            Stmt::Approve { action, .. } => self.resolve_approve_action(action),
            Stmt::Expr { expr, .. } => self.resolve_expr(expr),
        }
    }

    /// The action in an `approve Label(args...)` is descriptive — the
    /// top-level callee is a label, not a reference. The arguments are
    /// resolved normally.
    fn resolve_approve_action(&mut self, e: &Expr) {
        if let Expr::Call { args, .. } = e {
            for arg in args {
                self.resolve_expr(arg);
            }
        } else {
            self.resolve_expr(e);
        }
    }

    fn resolve_expr(&mut self, e: &Expr) {
        match e {
            Expr::Literal { .. } => {}
            Expr::Ident { name, .. } => self.resolve_ident(name),
            Expr::Call { callee, args, .. } => {
                self.resolve_expr(callee);
                for arg in args {
                    self.resolve_expr(arg);
                }
            }
            Expr::FieldAccess { target, .. } => {
                // Only the root of a dotted chain needs resolving; the
                // field name itself is validated by the type checker.
                self.resolve_expr(target);
            }
            Expr::Index { target, index, .. } => {
                self.resolve_expr(target);
                self.resolve_expr(index);
            }
            Expr::BinOp { left, right, .. } => {
                self.resolve_expr(left);
                self.resolve_expr(right);
            }
            Expr::UnOp { operand, .. } => self.resolve_expr(operand),
            Expr::List { items, .. } => {
                for item in items {
                    self.resolve_expr(item);
                }
            }
            Expr::TryPropagate { inner, .. } => self.resolve_expr(inner),
            Expr::TryRetry { body, .. } => self.resolve_expr(body),
            Expr::Replay {
                trace,
                arms,
                else_body,
                ..
            } => {
                self.resolve_expr(trace);
                for arm in arms {
                    self.resolve_replay_arm(arm);
                }
                self.resolve_expr(else_body);
            }
        }
    }

    /// Resolve one replay arm: validate the pattern name, open a
    /// fresh scope with any arm-level captures (whole-event `as
    /// <ident>` binding + per-arg tool captures), then walk the
    /// arm body.
    fn resolve_replay_arm(&mut self, arm: &ReplayArm) {
        self.resolve_replay_pattern(&arm.pattern);

        // Arm body has access to its captures only; siblings don't
        // see each other's captures, and the `else` body sees none.
        self.push_scope();

        if let Some(capture) = &arm.capture {
            let id = self.fresh_local();
            self.current_scope_mut().insert(&capture.name, id);
            self.bindings.insert(capture.span, Binding::Local(id));
        }

        // Per-arg tool captures bind alongside the whole-event capture.
        if let ReplayPattern::Tool { arg, .. } = &arm.pattern {
            if let ToolArgPattern::Capture { name, span } = arg {
                let id = self.fresh_local();
                self.current_scope_mut().insert(&name.name, id);
                self.bindings.insert(*span, Binding::Local(id));
            }
        }

        self.resolve_expr(&arm.body);
        self.pop_scope();
    }

    /// Validate a replay pattern's name against the file's symbol
    /// table (for `llm` / `tool`) or the known-approval-labels set
    /// (for `approve`). Emits `UnknownReplay*` or
    /// `ReplayPatternKindMismatch` on failure; records the
    /// resolved binding in the pattern side-table on success.
    fn resolve_replay_pattern(&mut self, pattern: &ReplayPattern) {
        match pattern {
            ReplayPattern::Llm { prompt, span } => {
                match self.symbols.lookup_def(prompt) {
                    Some(def_id) => {
                        let entry = self.symbols.get(def_id);
                        if entry.kind == DeclKind::Prompt {
                            self.replay_pattern_bindings
                                .insert(*span, ReplayPatternBinding::Llm(def_id));
                        } else {
                            self.errors.push(ResolveError {
                                kind: ResolveErrorKind::ReplayPatternKindMismatch {
                                    name: prompt.clone(),
                                    expected_kind: "prompt",
                                    actual_kind: decl_kind_label(entry.kind),
                                },
                                span: *span,
                            });
                        }
                    }
                    None => {
                        self.errors.push(ResolveError {
                            kind: ResolveErrorKind::UnknownReplayPrompt {
                                name: prompt.clone(),
                            },
                            span: *span,
                        });
                    }
                }
            }
            ReplayPattern::Tool { tool, span, .. } => {
                match self.symbols.lookup_def(tool) {
                    Some(def_id) => {
                        let entry = self.symbols.get(def_id);
                        if entry.kind == DeclKind::Tool {
                            self.replay_pattern_bindings
                                .insert(*span, ReplayPatternBinding::Tool(def_id));
                        } else {
                            self.errors.push(ResolveError {
                                kind: ResolveErrorKind::ReplayPatternKindMismatch {
                                    name: tool.clone(),
                                    expected_kind: "tool",
                                    actual_kind: decl_kind_label(entry.kind),
                                },
                                span: *span,
                            });
                        }
                    }
                    None => {
                        self.errors.push(ResolveError {
                            kind: ResolveErrorKind::UnknownReplayTool {
                                name: tool.clone(),
                            },
                            span: *span,
                        });
                    }
                }
            }
            ReplayPattern::Approve { label, span } => {
                if self.known_approval_labels.contains(label) {
                    self.replay_pattern_bindings
                        .insert(*span, ReplayPatternBinding::Approve);
                } else {
                    self.errors.push(ResolveError {
                        kind: ResolveErrorKind::UnknownReplayApproval {
                            label: label.clone(),
                        },
                        span: *span,
                    });
                }
            }
        }
    }

    /// Walk the file once collecting every `approve Label(...)`
    /// site's label name. `approve` is only valid inside agent
    /// bodies today (top-level agents + agents-as-extend-methods);
    /// prompts carry template strings, not executable blocks.
    fn collect_approval_labels(&mut self, file: &File) {
        for decl in &file.decls {
            match decl {
                Decl::Agent(a) => collect_approval_labels_in_block(
                    &a.body,
                    &mut self.known_approval_labels,
                ),
                Decl::Extend(ext) => {
                    for method in &ext.methods {
                        if let ExtendMethodKind::Agent(a) = &method.kind {
                            collect_approval_labels_in_block(
                                &a.body,
                                &mut self.known_approval_labels,
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn resolve_ident(&mut self, id: &Ident) {
        // Walk the scope stack from innermost to outermost looking for a local.
        for scope in self.scopes.iter().rev() {
            if let Some(local) = scope.lookup(&id.name) {
                self.bindings.insert(id.span, Binding::Local(local));
                return;
            }
        }
        // Fall back to the file-level symbol table (includes built-ins).
        if let Some(b) = self.symbols.lookup(&id.name) {
            self.bindings.insert(id.span, b);
            return;
        }
        self.errors.push(ResolveError {
            kind: ResolveErrorKind::UndefinedName(id.name.clone()),
            span: id.span,
        });
    }

    // ---------------- scope helpers ----------------

    fn push_scope(&mut self) {
        self.scopes.push(LocalScope::default());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn current_scope_mut(&mut self) -> &mut LocalScope {
        self.scopes
            .last_mut()
            .expect("no active local scope — push_scope not called")
    }

    fn fresh_local(&mut self) -> LocalId {
        let id = LocalId(self.next_local_id);
        self.next_local_id += 1;
        id
    }
}

/// Human-readable label for a `DeclKind`, used in replay
/// pattern-kind-mismatch diagnostics ("`get_order` is a tool, not
/// a prompt").
fn decl_kind_label(kind: DeclKind) -> &'static str {
    match kind {
        DeclKind::Import => "import",
        DeclKind::Type => "type",
        DeclKind::Tool => "tool",
        DeclKind::Prompt => "prompt",
        DeclKind::Agent => "agent",
        DeclKind::Eval => "eval",
        DeclKind::Effect => "effect",
        DeclKind::Model => "model",
    }
}

/// Recursively walk a block collecting every `approve Label(...)`
/// site's label name into `out`. The label is the callee of a
/// `Call` expression inside a `Stmt::Approve`.
fn collect_approval_labels_in_block(block: &Block, out: &mut HashSet<String>) {
    for stmt in &block.stmts {
        collect_approval_labels_in_stmt(stmt, out);
    }
}

fn collect_approval_labels_in_stmt(stmt: &Stmt, out: &mut HashSet<String>) {
    match stmt {
        Stmt::Approve { action, .. } => {
            if let Expr::Call { callee, .. } = action {
                if let Expr::Ident { name, .. } = callee.as_ref() {
                    out.insert(name.name.clone());
                }
            }
        }
        Stmt::If {
            then_block,
            else_block,
            ..
        } => {
            collect_approval_labels_in_block(then_block, out);
            if let Some(b) = else_block {
                collect_approval_labels_in_block(b, out);
            }
        }
        Stmt::For { body, .. } => {
            collect_approval_labels_in_block(body, out);
        }
        _ => {}
    }
}
