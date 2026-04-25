//! Lower a typed AST into IR.
//!
//! Every AST construct maps to an IR construct. References are resolved
//! via the resolver's side-table; types come from the checker's side-table.

mod stream;

use crate::imports::{
    build_imported_def_ids, resolve_module_qualified_type_ref, resolve_root_imported_type_ref,
    ImportedDefKey,
};
use crate::types::*;
use corvid_ast::{
    AgentAttribute, AgentDecl, BinaryOp, Block, Decl, Effect, EvalAssert, EvalDecl, Expr,
    ExtendMethodKind, ExternAbi, File, Ident,
    ImportDecl, ImportSource, Literal, Param, PromptDecl, ReplayArm, ReplayPattern, Span, Stmt,
    ToolArgPattern, ToolDecl, TypeDecl, TypeRef, UnaryOp,
};
use corvid_resolve::{
    resolver::MethodEntry, Binding, BuiltIn, DeclKind, DefId, LocalId, ModuleResolution, Resolved,
    ResolvedModule, SymbolTable,
};
use corvid_types::effects::{canonical_dimension_name, numeric_constraint_value, EffectRegistry};
use corvid_types::{Checked, ImportedCallKind, ImportedCallTarget, Type};
use std::collections::HashMap;
use std::path::PathBuf;

/// Entry point: produce an `IrFile` from parsed/resolved/checked sources.
pub fn lower(file: &File, resolved: &Resolved, checked: &Checked) -> IrFile {
    let imported_def_ids = HashMap::new();
    let mut l = Lowerer::new(resolved, checked, None, None, &imported_def_ids);
    l.lower_file(file)
}

/// Lower with cross-file module metadata. This is the file-backed
/// counterpart to [`lower`]: it preserves successful `alias.Type`
/// resolutions as `Type::ImportedStruct` in IR signatures instead
/// of degrading them to `Unknown` after typechecking.
pub fn lower_with_modules(
    file: &File,
    resolved: &Resolved,
    checked: &Checked,
    modules: &ModuleResolution,
    checked_modules: &HashMap<PathBuf, Checked>,
) -> IrFile {
    let imported_def_ids = build_imported_def_ids(resolved, modules);
    let mut l = Lowerer::new(resolved, checked, Some(modules), None, &imported_def_ids);
    let mut ir = l.lower_file(file);

    let mut loaded = modules.all_modules.values().collect::<Vec<_>>();
    loaded.sort_by(|a, b| a.path.cmp(&b.path));
    for module in loaded {
        let Some(module_checked) = checked_modules.get(&module.path) else {
            continue;
        };
        let mut module_lowerer = Lowerer::new(
            module.resolved.as_ref(),
            module_checked,
            Some(modules),
            Some(module),
            &imported_def_ids,
        );
        let module_ir = module_lowerer.lower_file(module.file.as_ref());
        ir.types.extend(module_ir.types);
        ir.tools.extend(module_ir.tools);
        ir.prompts.extend(module_ir.prompts);
        ir.agents.extend(module_ir.agents);
    }

    ir
}

struct Lowerer<'a> {
    symbols: &'a SymbolTable,
    bindings: &'a HashMap<Span, Binding>,
    types: &'a HashMap<Span, Type>,
    /// Per-receiver-type method side-table from the
    /// resolver. `lower_file` walks `Decl::Extend` blocks and looks
    /// up each method's allocated DefId here so the IR emits methods
    /// alongside free decls in the per-kind vectors.
    methods: &'a HashMap<DefId, HashMap<String, MethodEntry>>,
    /// Effect name → confidence gate threshold, populated from
    /// `EffectDecl`s with `trust: autonomous_if_confident(T)` dimension.
    /// Used during tool lowering to set `IrTool.confidence_gate`.
    confidence_gates: HashMap<String, f64>,
    effect_registry: EffectRegistry,
    module_resolution: Option<&'a ModuleResolution>,
    current_module: Option<&'a ResolvedModule>,
    imported_def_ids: &'a HashMap<ImportedDefKey, DefId>,
    imported_calls: &'a HashMap<Span, ImportedCallTarget>,
    wrapping_arithmetic: bool,
}

impl<'a> Lowerer<'a> {
    fn new(
        resolved: &'a Resolved,
        checked: &'a Checked,
        module_resolution: Option<&'a ModuleResolution>,
        current_module: Option<&'a ResolvedModule>,
        imported_def_ids: &'a HashMap<ImportedDefKey, DefId>,
    ) -> Self {
        Self {
            symbols: &resolved.symbols,
            bindings: &resolved.bindings,
            types: &checked.types,
            methods: &resolved.methods,
            confidence_gates: HashMap::new(),
            effect_registry: EffectRegistry::default(),
            module_resolution,
            current_module,
            imported_def_ids,
            imported_calls: &checked.imported_calls,
            wrapping_arithmetic: false,
        }
    }

    fn remap_def_id(&self, def_id: DefId) -> DefId {
        let Some(module) = self.current_module else {
            return def_id;
        };
        self.imported_def_ids
            .get(&ImportedDefKey {
                module_path: module.path.to_string_lossy().into_owned(),
                def_id,
            })
            .copied()
            .unwrap_or(def_id)
    }

    fn remap_imported_target(&self, target: &ImportedCallTarget) -> DefId {
        self.imported_def_ids
            .get(&ImportedDefKey {
                module_path: target.module_path.clone(),
                def_id: target.def_id,
            })
            .copied()
            .unwrap_or(target.def_id)
    }

    /// Scan the file's effect declarations for `trust: autonomous_if_confident(T)`
    /// dimension values and populate the confidence_gates table.
    fn populate_confidence_gates(&mut self, file: &File) {
        for decl in &file.decls {
            let Decl::Effect(effect) = decl else { continue };
            for dim in &effect.dimensions {
                if dim.name.name == "trust" {
                    if let corvid_ast::DimensionValue::ConfidenceGated { threshold, .. } = &dim.value {
                        self.confidence_gates.insert(effect.name.name.clone(), *threshold);
                    }
                }
            }
        }
    }

    fn lower_file(&mut self, file: &File) -> IrFile {
        self.populate_confidence_gates(file);
        self.populate_effect_registry(file);
        let mut imports = Vec::new();
        let mut types = Vec::new();
        let mut tools = Vec::new();
        let mut prompts = Vec::new();
        let mut agents = Vec::new();
        let mut evals = Vec::new();

        for decl in &file.decls {
            match decl {
                Decl::Import(i) => imports.push(self.lower_import(i)),
                Decl::Type(t) => types.push(self.lower_type(t)),
                Decl::Tool(t) => tools.push(self.lower_tool(t)),
                Decl::Prompt(p) => prompts.push(self.lower_prompt(p)),
                Decl::Agent(a) => agents.push(self.lower_agent(a)),
                Decl::Eval(e) => evals.push(self.lower_eval(e)),
                Decl::Effect(_) => {}
                Decl::Model(_) => {
                    // Phase 20h slice A: model declarations are a
                    // static catalog with no runtime lowering yet.
                    // Slice B adds IR fields on IrPrompt for
                    // capability requirements; slice C adds the
                    // route table.
                }
                Decl::Extend(ext) => {
                    // Lower each method into the appropriate per-kind
                    // IR vector. Methods get
                    // their `DefId` from the resolver's method side
                    // table (NOT the by-name namespace, since two
                    // types can share method names like `total`).
                    let Some(type_def_id) =
                        self.symbols.lookup_def(&ext.type_name.name)
                    else {
                        continue;
                    };
                    let Some(method_table) = self.methods.get(&type_def_id) else {
                        continue;
                    };
                    for method in &ext.methods {
                        let Some(entry) = method_table.get(&method.name().name) else {
                            continue;
                        };
                        match &method.kind {
                            ExtendMethodKind::Tool(t) => {
                                tools.push(self.lower_tool_with_id(t, entry.def_id));
                            }
                            ExtendMethodKind::Prompt(p) => {
                                prompts.push(self.lower_prompt_with_id(p, entry.def_id));
                            }
                            ExtendMethodKind::Agent(a) => {
                                agents.push(self.lower_agent_with_id(a, entry.def_id));
                            }
                        }
                    }
                }
            }
        }

        IrFile {
            imports,
            types,
            tools,
            prompts,
            agents,
            evals,
        }
    }

    fn lower_import(&self, i: &ImportDecl) -> IrImport {
        let alias_name = i.alias.as_ref().map(|a| a.name.clone());
        let binding_name = alias_name.clone().unwrap_or_else(|| i.module.clone());
        let id = self
            .symbols
            .lookup_def(&binding_name)
            .expect("import binding missing from symbol table");
        let source = match i.source {
            ImportSource::Python => IrImportSource::Python,
            ImportSource::Corvid => IrImportSource::Corvid,
        };
        IrImport {
            id,
            source,
            module: i.module.clone(),
            alias: alias_name,
            span: i.span,
        }
    }

    fn lower_eval(&self, e: &EvalDecl) -> IrEval {
        let id = self
            .symbols
            .lookup_def(&e.name.name)
            .expect("eval missing from symbol table");
        IrEval {
            id: self.remap_def_id(id),
            name: e.name.name.clone(),
            body: self.lower_block(&e.body),
            assertions: e
                .assertions
                .iter()
                .map(|assertion| self.lower_eval_assert(assertion))
                .collect(),
            span: e.span,
        }
    }

    fn lower_eval_assert(&self, assertion: &EvalAssert) -> IrEvalAssert {
        match assertion {
            EvalAssert::Value {
                expr,
                confidence,
                runs,
                span,
            } => IrEvalAssert::Value {
                expr: self.lower_expr(expr),
                confidence: *confidence,
                runs: *runs,
                span: *span,
            },
            EvalAssert::Called { tool, span } => {
                let def_id = match self.bindings.get(&tool.span) {
                    Some(Binding::Decl(def_id)) => *def_id,
                    _ => panic!("eval called assertion missing resolved callable"),
                };
                IrEvalAssert::Called {
                    def_id: self.remap_def_id(def_id),
                    name: tool.name.clone(),
                    span: *span,
                }
            }
            EvalAssert::Approved { label, span } => IrEvalAssert::Approved {
                label: label.name.clone(),
                span: *span,
            },
            EvalAssert::Cost { op, bound, span } => IrEvalAssert::Cost {
                op: *op,
                bound: *bound,
                span: *span,
            },
            EvalAssert::Ordering {
                before,
                after,
                span,
            } => {
                let before_id = match self.bindings.get(&before.span) {
                    Some(Binding::Decl(def_id)) => *def_id,
                    _ => panic!("eval ordering assertion missing resolved `before` callable"),
                };
                let after_id = match self.bindings.get(&after.span) {
                    Some(Binding::Decl(def_id)) => *def_id,
                    _ => panic!("eval ordering assertion missing resolved `after` callable"),
                };
                IrEvalAssert::Ordering {
                    before_id: self.remap_def_id(before_id),
                    before_name: before.name.clone(),
                    after_id: self.remap_def_id(after_id),
                    after_name: after.name.clone(),
                    span: *span,
                }
            }
        }
    }

    fn lower_type(&self, t: &TypeDecl) -> IrType {
        let id = self
            .symbols
            .lookup_def(&t.name.name)
            .expect("type missing from symbol table");
        let fields = t
            .fields
            .iter()
            .map(|f| IrField {
                name: f.name.name.clone(),
                ty: self.type_ref_to_type(&f.ty),
                span: f.span,
            })
            .collect();
        IrType {
            id: self.remap_def_id(id),
            name: t.name.name.clone(),
            fields,
            span: t.span,
        }
    }

    fn lower_tool(&self, t: &ToolDecl) -> IrTool {
        let id = self
            .symbols
            .lookup_def(&t.name.name)
            .expect("tool missing from symbol table");
        self.lower_tool_with_id(t, id)
    }

    /// Lower a tool decl whose DefId was allocated outside
    /// the by-name namespace (i.e. it's a method inside an `extend`
    /// block, looked up via the methods side-table rather than by
    /// name).
    fn lower_tool_with_id(&self, t: &ToolDecl, id: DefId) -> IrTool {
        // If any of the tool's declared effects has `autonomous_if_confident(T)`,
        // carry the strictest threshold as the confidence gate.
        let mut confidence_gate: Option<f64> = None;
        for effect_ref in &t.effect_row.effects {
            if let Some(&threshold) = self.confidence_gates.get(&effect_ref.name.name) {
                confidence_gate = match confidence_gate {
                    Some(current) => Some(current.max(threshold)),
                    None => Some(threshold),
                };
            }
        }

        IrTool {
            id: self.remap_def_id(id),
            name: t.name.name.clone(),
            params: self.lower_params(&t.params),
            return_ty: self.type_ref_to_type(&t.return_ty),
            effect: t.effect,
            effect_names: t.effect_row.effects.iter().map(|e| e.name.name.clone()).collect(),
            confidence_gate,
            span: t.span,
        }
    }

    fn lower_prompt(&self, p: &PromptDecl) -> IrPrompt {
        let id = self
            .symbols
            .lookup_def(&p.name.name)
            .expect("prompt missing from symbol table");
        self.lower_prompt_with_id(p, id)
    }

    fn lower_prompt_with_id(&self, p: &PromptDecl, id: DefId) -> IrPrompt {
        let cites_strictly_param = p.cites_strictly.as_ref().and_then(|param_name| {
            p.params.iter().position(|param| param.name.name == *param_name)
        });
        let effect_names: Vec<String> = p
            .effect_row
            .effects
            .iter()
            .map(|e| e.name.name.clone())
            .collect();
        let effect_refs: Vec<&str> = effect_names.iter().map(|name| name.as_str()).collect();
        let profile = self.effect_registry.compose(&effect_refs);
        let route = p
            .route
            .as_ref()
            .map(|rt| self.lower_route_arms(&rt.arms))
            .unwrap_or_default();
        let progressive = p
            .progressive
            .as_ref()
            .map(|chain| self.lower_progressive_stages(&chain.stages))
            .unwrap_or_default();
        let rollout = p.rollout.as_ref().and_then(|spec| {
            let variant = self.remap_def_id(self.symbols.lookup_def(&spec.variant.name)?);
            let baseline = self.remap_def_id(self.symbols.lookup_def(&spec.baseline.name)?);
            Some(IrRolloutSpec {
                variant_percent: spec.variant_percent,
                variant_def_id: variant,
                variant_name: spec.variant.name.clone(),
                baseline_def_id: baseline,
                baseline_name: spec.baseline.name.clone(),
                span: spec.span,
            })
        });
        let ensemble = p.ensemble.as_ref().map(|spec| {
            let members = spec
                .models
                .iter()
                .filter_map(|model| {
                    let def_id = self.symbols.lookup_def(&model.name)?;
                    Some(IrEnsembleMember {
                        def_id: self.remap_def_id(def_id),
                        name: model.name.clone(),
                        span: model.span,
                    })
                })
                .collect();
            let vote = match spec.vote {
                corvid_ast::VoteStrategy::Majority => IrVoteStrategy::Majority,
            };
            IrEnsembleSpec {
                models: members,
                vote,
                span: spec.span,
            }
        });
        let adversarial = p.adversarial.as_ref().and_then(|spec| {
            let proposer = self.remap_def_id(self.symbols.lookup_def(&spec.proposer.name)?);
            let challenger = self.remap_def_id(self.symbols.lookup_def(&spec.challenger.name)?);
            let adjudicator = self.remap_def_id(self.symbols.lookup_def(&spec.adjudicator.name)?);
            Some(IrAdversarialSpec {
                proposer_def_id: proposer,
                proposer_name: spec.proposer.name.clone(),
                challenger_def_id: challenger,
                challenger_name: spec.challenger.name.clone(),
                adjudicator_def_id: adjudicator,
                adjudicator_name: spec.adjudicator.name.clone(),
                span: spec.span,
            })
        });
        IrPrompt {
            id: self.remap_def_id(id),
            name: p.name.name.clone(),
            params: self.lower_params(&p.params),
            return_ty: self.type_ref_to_type(&p.return_ty),
            template: p.template.clone(),
            effect_names,
            effect_cost: numeric_profile_dimension(&profile, "cost"),
            effect_confidence: confidence_profile_dimension(&profile),
            cites_strictly_param,
            min_confidence: p.stream.min_confidence,
            max_tokens: p.stream.max_tokens,
            backpressure: p.stream.backpressure.clone(),
            escalate_to: p.stream.escalate_to.as_ref().map(|model| model.name.clone()),
            calibrated: p.calibrated,
            cacheable: p.cacheable,
            capability_required: p.capability_required.as_ref().map(|c| c.name.clone()),
            route,
            progressive,
            rollout,
            ensemble,
            adversarial,
            span: p.span,
        }
    }

    fn lower_progressive_stages(
        &self,
        stages: &[corvid_ast::ProgressiveStage],
    ) -> Vec<IrProgressiveStage> {
        let mut out = Vec::with_capacity(stages.len());
        for stage in stages {
            let Some(def_id) = self.symbols.lookup_def(&stage.model.name) else {
                continue;
            };
            out.push(IrProgressiveStage {
                model_def_id: self.remap_def_id(def_id),
                model_name: stage.model.name.clone(),
                threshold: stage.threshold,
                span: stage.span,
            });
        }
        out
    }

    fn lower_route_arms(&self, arms: &[corvid_ast::RouteArm]) -> Vec<IrRouteArm> {
        use corvid_ast::RoutePattern;
        let mut out = Vec::with_capacity(arms.len());
        for arm in arms {
            let pattern = match &arm.pattern {
                RoutePattern::Wildcard { .. } => IrRoutePattern::Wildcard,
                RoutePattern::Guard(expr) => IrRoutePattern::Guard(self.lower_expr(expr)),
            };
            // Arms whose model ident didn't resolve to a Decl::Model
            // were already flagged by the checker. At IR time we
            // best-effort resolve again; unresolved arms are skipped
            // so IR doesn't carry broken references.
            let Some(def_id) = self.symbols.lookup_def(&arm.model.name) else {
                continue;
            };
            out.push(IrRouteArm {
                pattern,
                model_def_id: self.remap_def_id(def_id),
                model_name: arm.model.name.clone(),
                span: arm.span,
            });
        }
        out
    }

    fn lower_agent(&mut self, a: &AgentDecl) -> IrAgent {
        let id = self
            .symbols
            .lookup_def(&a.name.name)
            .expect("agent missing from symbol table");
        self.lower_agent_with_id(a, id)
    }

    fn lower_agent_with_id(&mut self, a: &AgentDecl, id: DefId) -> IrAgent {
        let previous_wrapping = self.wrapping_arithmetic;
        self.wrapping_arithmetic = AgentAttribute::is_wrapping(&a.attributes);
        let body = self.lower_block(&a.body);
        self.wrapping_arithmetic = previous_wrapping;
        IrAgent {
            id: self.remap_def_id(id),
            name: a.name.name.clone(),
            extern_abi: a.extern_abi.map(|abi| match abi {
                ExternAbi::C => IrExternAbi::C,
            }),
            params: self.lower_params(&a.params),
            return_ty: self.type_ref_to_type(&a.return_ty),
            cost_budget: agent_cost_budget(a),
            wrapping_arithmetic: AgentAttribute::is_wrapping(&a.attributes),
            body,
            span: a.span,
            // Populated by corvid-codegen-cl's ownership pass. `None`
            // at lowering time means "every parameter is
            // Owned" at codegen (matches pre-17b semantics).
            borrow_sig: None,
        }
    }

    fn populate_effect_registry(&mut self, file: &File) {
        let decls: Vec<_> = file
            .decls
            .iter()
            .filter_map(|decl| match decl {
                Decl::Effect(effect) => Some(effect.clone()),
                _ => None,
            })
            .collect();
        self.effect_registry = EffectRegistry::from_decls(&decls);
    }

    fn lower_params(&self, ps: &[Param]) -> Vec<IrParam> {
        ps.iter()
            .map(|p| {
                let local_id = match self.bindings.get(&p.name.span) {
                    Some(Binding::Local(id)) => *id,
                    _ => LocalId(u32::MAX), // should not happen post-resolve
                };
                IrParam {
                    name: p.name.name.clone(),
                    local_id,
                    ty: self.type_ref_to_type(&p.ty),
                    span: p.span,
                }
            })
            .collect()
    }

    fn lower_block(&self, b: &Block) -> IrBlock {
        IrBlock {
            stmts: b.stmts.iter().map(|s| self.lower_stmt(s)).collect(),
            span: b.span,
        }
    }

    fn lower_stmt(&self, s: &Stmt) -> IrStmt {
        match s {
            Stmt::Let { name, value, span, .. } => {
                let local_id = match self.bindings.get(&name.span) {
                    Some(Binding::Local(id)) => *id,
                    _ => LocalId(u32::MAX),
                };
                let lowered_value = self.lower_expr(value);
                IrStmt::Let {
                    local_id,
                    name: name.name.clone(),
                    ty: lowered_value.ty.clone(),
                    value: lowered_value,
                    span: *span,
                }
            }
            Stmt::Return { value, span } => IrStmt::Return {
                value: value.as_ref().map(|e| self.lower_expr(e)),
                span: *span,
            },
            Stmt::Yield { value, span } => IrStmt::Yield {
                value: self.lower_expr(value),
                span: *span,
            },
            Stmt::If { cond, then_block, else_block, span } => IrStmt::If {
                cond: self.lower_expr(cond),
                then_block: self.lower_block(then_block),
                else_block: else_block.as_ref().map(|b| self.lower_block(b)),
                span: *span,
            },
            Stmt::For { var, iter, body, span } => {
                let var_local = match self.bindings.get(&var.span) {
                    Some(Binding::Local(id)) => *id,
                    _ => LocalId(u32::MAX),
                };
                IrStmt::For {
                    var_local,
                    var_name: var.name.clone(),
                    iter: self.lower_expr(iter),
                    body: self.lower_block(body),
                    span: *span,
                }
            }
            Stmt::Approve { action, span } => {
                let (label, args) = self.extract_approve_action(action);
                IrStmt::Approve {
                    label,
                    args,
                    span: *span,
                }
            }
            Stmt::Expr { expr, span } => {
                // Special-case break/continue/pass (currently encoded as Idents).
                if let Expr::Ident { name, .. } = expr {
                    if let Some(Binding::BuiltIn(b)) = self.bindings.get(&name.span) {
                        match b {
                            BuiltIn::Break => return IrStmt::Break { span: *span },
                            BuiltIn::Continue => return IrStmt::Continue { span: *span },
                            BuiltIn::Pass => return IrStmt::Pass { span: *span },
                            _ => {}
                        }
                    }
                }
                IrStmt::Expr {
                    expr: self.lower_expr(expr),
                    span: *span,
                }
            }
        }
    }

    fn extract_approve_action(&self, action: &Expr) -> (String, Vec<IrExpr>) {
        if let Expr::Call { callee, args, .. } = action {
            if let Expr::Ident { name, .. } = &**callee {
                let lowered_args: Vec<IrExpr> =
                    args.iter().map(|a| self.lower_expr(a)).collect();
                return (name.name.clone(), lowered_args);
            }
        }
        // Non-call or non-ident callee: synthesize a label.
        ("<unknown>".to_string(), Vec::new())
    }

    fn lower_expr(&self, e: &Expr) -> IrExpr {
        let ty = self.types.get(&e.span()).cloned().unwrap_or(Type::Unknown);
        let kind = match e {
            Expr::Literal { value, .. } => IrExprKind::Literal(match value {
                Literal::Int(n) => IrLiteral::Int(*n),
                Literal::Float(f) => IrLiteral::Float(*f),
                Literal::String(s) => IrLiteral::String(s.clone()),
                Literal::Bool(b) => IrLiteral::Bool(*b),
                Literal::Nothing => IrLiteral::Nothing,
            }),
            Expr::Ident { name, .. } => self.lower_ident(name),
            Expr::Call { callee, args, .. } => self.lower_call(callee, args),
            Expr::FieldAccess { target, field, .. } => IrExprKind::FieldAccess {
                target: Box::new(self.lower_expr(target)),
                field: field.name.clone(),
            },
            Expr::Index { target, index, .. } => IrExprKind::Index {
                target: Box::new(self.lower_expr(target)),
                index: Box::new(self.lower_expr(index)),
            },
            Expr::BinOp { op, left, right, span } => {
                let left = Box::new(self.lower_expr(left));
                let right = Box::new(self.lower_expr(right));
                if self.wrapping_arithmetic && is_wrapping_int_binop(*op, self.types.get(span)) {
                    IrExprKind::WrappingBinOp { op: *op, left, right }
                } else {
                    IrExprKind::BinOp { op: *op, left, right }
                }
            }
            Expr::UnOp { op, operand, span } => {
                let operand = Box::new(self.lower_expr(operand));
                if self.wrapping_arithmetic && is_wrapping_int_unop(*op, self.types.get(span)) {
                    IrExprKind::WrappingUnOp { op: *op, operand }
                } else {
                    IrExprKind::UnOp { op: *op, operand }
                }
            }
            Expr::List { items, .. } => IrExprKind::List {
                items: items.iter().map(|i| self.lower_expr(i)).collect(),
            },
            Expr::TryPropagate { inner, .. } => IrExprKind::TryPropagate {
                inner: Box::new(self.lower_expr(inner)),
            },
            Expr::TryRetry {
                body,
                attempts,
                backoff,
                ..
            } => IrExprKind::TryRetry {
                body: Box::new(self.lower_expr(body)),
                attempts: *attempts,
                backoff: *backoff,
            },
            Expr::Replay {
                trace,
                arms,
                else_body,
                ..
            } => IrExprKind::Replay {
                trace: Box::new(self.lower_expr(trace)),
                arms: arms.iter().map(|arm| self.lower_replay_arm(arm)).collect(),
                else_body: Box::new(self.lower_expr(else_body)),
            },
        };
        IrExpr {
            kind,
            ty,
            span: e.span(),
        }
    }

    /// Lower one replay arm. The body is lowered in the same
    /// per-arm scope the resolver opened (21-inv-E-2b), so any
    /// capture the arm binds is already reachable via
    /// `self.bindings` keyed by the capture's span.
    fn lower_replay_arm(&self, arm: &ReplayArm) -> IrReplayArm {
        let pattern = self.lower_replay_pattern(&arm.pattern);
        let capture = arm.capture.as_ref().map(|ident| IrReplayCapture {
            local_id: self.lookup_local(ident.span, "replay `as` capture"),
            name: ident.name.clone(),
            span: ident.span,
        });
        let body = Box::new(self.lower_expr(&arm.body));
        IrReplayArm {
            pattern,
            capture,
            body,
            span: arm.span,
        }
    }

    fn lower_replay_pattern(&self, pattern: &ReplayPattern) -> IrReplayPattern {
        match pattern {
            ReplayPattern::Llm { prompt, span } => IrReplayPattern::Llm {
                prompt: prompt.clone(),
                span: *span,
            },
            ReplayPattern::Tool { tool, arg, span } => IrReplayPattern::Tool {
                tool: tool.clone(),
                arg: self.lower_replay_tool_arg(arg),
                span: *span,
            },
            ReplayPattern::Approve { label, span } => IrReplayPattern::Approve {
                label: label.clone(),
                span: *span,
            },
        }
    }

    fn lower_replay_tool_arg(&self, arg: &ToolArgPattern) -> IrReplayToolArgPattern {
        match arg {
            ToolArgPattern::Wildcard { .. } => IrReplayToolArgPattern::Wildcard,
            ToolArgPattern::StringLit { value, .. } => {
                IrReplayToolArgPattern::StringLit(value.clone())
            }
            ToolArgPattern::Capture { name, span } => {
                IrReplayToolArgPattern::Capture(IrReplayCapture {
                    local_id: self.lookup_local(*span, "replay tool-arg capture"),
                    name: name.name.clone(),
                    span: *span,
                })
            }
        }
    }

    /// Resolve a capture ident's span to its `LocalId`. The
    /// resolver (E-2b) is the source of truth: every capture span
    /// is registered as `Binding::Local(_)` before the checker and
    /// lowerer ever see it. A missing binding here signals a
    /// resolver bug, not a user error, so we fall back to
    /// `LocalId(u32::MAX)` (the same sentinel `lower_ident` uses
    /// for unresolved names) so codegen doesn't panic — the
    /// resolver's diagnostics will already have been surfaced.
    fn lookup_local(&self, span: Span, context: &str) -> LocalId {
        match self.bindings.get(&span) {
            Some(Binding::Local(local_id)) => *local_id,
            _ => {
                let _ = context; // reserved for future debug-assert
                LocalId(u32::MAX)
            }
        }
    }

    fn lower_ident(&self, id: &Ident) -> IrExprKind {
        match self.bindings.get(&id.span) {
            Some(Binding::Local(local_id)) => IrExprKind::Local {
                local_id: *local_id,
                name: id.name.clone(),
            },
            Some(Binding::Decl(def_id)) => IrExprKind::Decl {
                def_id: self.remap_def_id(*def_id),
                name: id.name.clone(),
            },
            Some(Binding::BuiltIn(BuiltIn::None)) => IrExprKind::OptionNone,
            Some(Binding::BuiltIn(_)) => IrExprKind::Local {
                local_id: LocalId(u32::MAX),
                name: id.name.clone(),
            },
            None => IrExprKind::Local {
                local_id: LocalId(u32::MAX),
                name: id.name.clone(),
            },
        }
    }

    fn lower_call(&self, callee: &Expr, args: &[Expr]) -> IrExprKind {
        // `target.method(args)` rewrites to a regular call
        // with the receiver prepended. Method DefId comes from the
        // resolver's per-type method side-table; the caller's type
        // is read from the type checker's per-expression side table.
        if let Expr::Call { .. } = callee {
            // (no-op: shouldn't happen — Call's callee is an Expr,
            // never another Call directly. Keeps the match arm
            // catchall narrower below.)
        }
        if let Expr::FieldAccess { target, field, .. } = callee {
            if let Some(rewrite) = self.try_imported_call(callee, field, args) {
                return rewrite;
            }
            if let Some(rewrite) = self.try_grounded_builtin_call(target, field, args) {
                return rewrite;
            }
            if let Some(rewrite) = stream::try_stream_builtin_call(self, target, field, args) {
                return rewrite;
            }
            if let Some(rewrite) = self.try_method_call(target, field, args) {
                return rewrite;
            }
        }

        let (kind, callee_name) = match callee {
            Expr::Ident { name, .. } => match self.bindings.get(&name.span) {
                Some(Binding::BuiltIn(BuiltIn::Ok)) => {
                    let inner = args
                        .first()
                        .map(|arg| self.lower_expr(arg))
                        .unwrap_or_else(|| IrExpr {
                            kind: IrExprKind::OptionNone,
                            ty: Type::Unknown,
                            span: name.span,
                        });
                    return IrExprKind::ResultOk {
                        inner: Box::new(inner),
                    };
                }
                Some(Binding::BuiltIn(BuiltIn::Err)) => {
                    let inner = args
                        .first()
                        .map(|arg| self.lower_expr(arg))
                        .unwrap_or_else(|| IrExpr {
                            kind: IrExprKind::OptionNone,
                            ty: Type::Unknown,
                            span: name.span,
                        });
                    return IrExprKind::ResultErr {
                        inner: Box::new(inner),
                    };
                }
                Some(Binding::BuiltIn(BuiltIn::Some)) => {
                    let inner = args
                        .first()
                        .map(|arg| self.lower_expr(arg))
                        .unwrap_or_else(|| IrExpr {
                            kind: IrExprKind::OptionNone,
                            ty: Type::Unknown,
                            span: name.span,
                        });
                    return IrExprKind::OptionSome {
                        inner: Box::new(inner),
                    };
                }
                Some(Binding::BuiltIn(BuiltIn::None)) => return IrExprKind::OptionNone,
                Some(Binding::BuiltIn(BuiltIn::WeakNew)) => {
                    let strong = args
                        .first()
                        .map(|arg| self.lower_expr(arg))
                        .unwrap_or_else(|| IrExpr {
                            kind: IrExprKind::Literal(IrLiteral::Nothing),
                            ty: Type::Unknown,
                            span: name.span,
                        });
                    return IrExprKind::WeakNew {
                        strong: Box::new(strong),
                    };
                }
                Some(Binding::BuiltIn(BuiltIn::WeakUpgrade)) => {
                    let weak = args
                        .first()
                        .map(|arg| self.lower_expr(arg))
                        .unwrap_or_else(|| IrExpr {
                            kind: IrExprKind::Literal(IrLiteral::Nothing),
                            ty: Type::Unknown,
                            span: name.span,
                        });
                    return IrExprKind::WeakUpgrade {
                        weak: Box::new(weak),
                    };
                }
                Some(Binding::BuiltIn(BuiltIn::StreamMerge)) => {
                    return stream::lower_merge_call(self, name.span, args);
                }
                Some(Binding::BuiltIn(BuiltIn::StreamResumeToken)) => {
                    let stream = args
                        .first()
                        .map(|arg| self.lower_expr(arg))
                        .unwrap_or_else(|| IrExpr {
                            kind: IrExprKind::Literal(IrLiteral::Nothing),
                            ty: Type::Unknown,
                            span: name.span,
                        });
                    return IrExprKind::StreamResumeToken {
                        stream: Box::new(stream),
                    };
                }
                Some(Binding::BuiltIn(BuiltIn::Resume)) => {
                    if let Some(Expr::Ident { name: prompt_name, .. }) = args.first() {
                        if let Some(Binding::Decl(def_id)) = self.bindings.get(&prompt_name.span) {
                            if self.symbols.get(*def_id).kind == DeclKind::Prompt {
                                let token = args
                                    .get(1)
                                    .map(|arg| self.lower_expr(arg))
                                    .unwrap_or_else(|| IrExpr {
                                        kind: IrExprKind::Literal(IrLiteral::Nothing),
                                        ty: Type::Unknown,
                                        span: name.span,
                                    });
                                return IrExprKind::ResumeStream {
                                    prompt_def_id: self.remap_def_id(*def_id),
                                    prompt_name: prompt_name.name.clone(),
                                    token: Box::new(token),
                                };
                            }
                        }
                    }
                    (IrCallKind::Unknown, name.name.clone())
                }
                Some(Binding::Decl(def_id)) => {
                    let entry = self.symbols.get(*def_id);
                    let lowered_def_id = self.remap_def_id(*def_id);
                    let kind = match entry.kind {
                        DeclKind::Tool => {
                            // Effect is stored on the AST ToolDecl; we need
                            // it at call sites. We pass `Effect::Safe` as a
                            // stable default and let IrTool carry the truth.
                            // Codegen looks up the IrTool by def_id to route.
                            IrCallKind::Tool {
                                def_id: lowered_def_id,
                                effect: lookup_tool_effect(self.symbols, lowered_def_id),
                            }
                        }
                        DeclKind::Prompt => IrCallKind::Prompt {
                            def_id: lowered_def_id,
                        },
                        DeclKind::Agent => IrCallKind::Agent {
                            def_id: lowered_def_id,
                        },
                        DeclKind::Type => IrCallKind::StructConstructor {
                            def_id: lowered_def_id,
                        },
                        _ => IrCallKind::Unknown,
                    };
                    (kind, name.name.clone())
                }
                _ => (IrCallKind::Unknown, name.name.clone()),
            },
            _ => (IrCallKind::Unknown, "<indirect>".to_string()),
        };
        IrExprKind::Call {
            kind,
            callee_name,
            args: args.iter().map(|a| self.lower_expr(a)).collect(),
        }
    }

    /// Detect and lower a `target.method(args)` call. Returns
    /// `Some(IrExprKind::Call { ... })` with the receiver prepended
    /// when `target`'s type matches a registered method. Returns
    /// `None` when the call doesn't resolve to a method (caller
    /// falls back to the regular field-access-of-a-fn path, which
    /// produces `IrCallKind::Unknown` and lets later validation error).
    fn try_method_call(
        &self,
        target: &Expr,
        field: &Ident,
        args: &[Expr],
    ) -> Option<IrExprKind> {
        // Receiver type lives on the type-checker's side-table.
        let recv_ty = self.types.get(&target.span())?;
        let recv_def_id = match recv_ty {
            Type::Struct(id) => *id,
            _ => return None,
        };
        let entry = self.methods.get(&recv_def_id)?.get(&field.name)?;
        let def_id = self.remap_def_id(entry.def_id);
        let kind = match entry.kind {
            corvid_resolve::resolver::MethodKind::Tool => IrCallKind::Tool {
                def_id,
                // Method-tool effects keep `Safe` as the conservative
                // default; the IR's `IrTool` carries the
                // declared effect once `define_tool` lowers it.
                effect: Effect::Safe,
            },
            corvid_resolve::resolver::MethodKind::Prompt => IrCallKind::Prompt {
                def_id,
            },
            corvid_resolve::resolver::MethodKind::Agent => IrCallKind::Agent {
                def_id,
            },
        };
        // Receiver becomes the first argument.
        let mut lowered_args: Vec<IrExpr> = Vec::with_capacity(args.len() + 1);
        lowered_args.push(self.lower_expr(target));
        lowered_args.extend(args.iter().map(|a| self.lower_expr(a)));
        Some(IrExprKind::Call {
            kind,
            callee_name: field.name.clone(),
            args: lowered_args,
        })
    }

    fn try_grounded_builtin_call(
        &self,
        target: &Expr,
        field: &Ident,
        args: &[Expr],
    ) -> Option<IrExprKind> {
        if field.name != "unwrap_discarding_sources" || !args.is_empty() {
            return None;
        }
        match self.types.get(&target.span())? {
            Type::Grounded(_) => Some(IrExprKind::UnwrapGrounded {
                value: Box::new(self.lower_expr(target)),
            }),
            _ => None,
        }
    }

    fn try_imported_call(
        &self,
        callee: &Expr,
        field: &Ident,
        args: &[Expr],
    ) -> Option<IrExprKind> {
        let target = self.imported_calls.get(&callee.span())?;
        let def_id = self.remap_imported_target(target);
        let kind = match target.kind {
            ImportedCallKind::Type => IrCallKind::StructConstructor { def_id },
            ImportedCallKind::Tool => IrCallKind::Tool {
                def_id,
                effect: Effect::Safe,
            },
            ImportedCallKind::Prompt => IrCallKind::Prompt { def_id },
            ImportedCallKind::Agent => IrCallKind::Agent { def_id },
        };
        Some(IrExprKind::Call {
            kind,
            callee_name: field.name.clone(),
            args: args.iter().map(|arg| self.lower_expr(arg)).collect(),
        })
    }

    fn type_ref_to_type(&self, tr: &TypeRef) -> Type {
        match tr {
            TypeRef::Named { name, .. } => match name.name.as_str() {
                "Int" => Type::Int,
                "Float" => Type::Float,
                "String" => Type::String,
                "Bool" => Type::Bool,
                "Nothing" => Type::Nothing,
                _ => match self.symbols.lookup_def(&name.name) {
                    Some(id) => Type::Struct(self.remap_def_id(id)),
                    None => Type::Unknown,
                },
            },
            TypeRef::Qualified { alias, name, .. } => match self.current_module {
                Some(module) => self
                    .module_resolution
                    .and_then(|resolution| {
                        resolve_module_qualified_type_ref(
                            resolution,
                            module,
                            self.imported_def_ids,
                            &alias.name,
                            &name.name,
                        )
                    })
                    .unwrap_or(Type::Unknown),
                None => self
                    .module_resolution
                    .and_then(|resolution| {
                        resolve_root_imported_type_ref(resolution, &alias.name, &name.name)
                    })
                    .unwrap_or(Type::Unknown),
            },
            TypeRef::Generic { name, args, .. } => match name.name.as_str() {
                "List" if args.len() == 1 => Type::List(Box::new(self.type_ref_to_type(&args[0]))),
                "Stream" if args.len() == 1 => {
                    Type::Stream(Box::new(self.type_ref_to_type(&args[0])))
                }
                "Grounded" if args.len() == 1 => {
                    Type::Grounded(Box::new(self.type_ref_to_type(&args[0])))
                }
                "Partial" if args.len() == 1 => {
                    Type::Partial(Box::new(self.type_ref_to_type(&args[0])))
                }
                "ResumeToken" if args.len() == 1 => {
                    Type::ResumeToken(Box::new(self.type_ref_to_type(&args[0])))
                }
                "Option" if args.len() == 1 => {
                    Type::Option(Box::new(self.type_ref_to_type(&args[0])))
                }
                "Result" if args.len() == 2 => Type::Result(
                    Box::new(self.type_ref_to_type(&args[0])),
                    Box::new(self.type_ref_to_type(&args[1])),
                ),
                _ => Type::Unknown,
            },
            TypeRef::Weak { inner, effects, .. } => Type::Weak(
                Box::new(self.type_ref_to_type(inner)),
                effects.unwrap_or_else(corvid_ast::WeakEffectRow::any),
            ),
            TypeRef::Function { .. } => Type::Unknown,
        }
    }
}

fn numeric_profile_dimension(
    profile: &corvid_types::effects::ComposedProfile,
    dim: &str,
) -> f64 {
    match profile.dimensions.get(dim) {
        Some(corvid_ast::DimensionValue::Cost(value)) => *value,
        Some(corvid_ast::DimensionValue::Number(value)) => *value,
        _ => 0.0,
    }
}

fn confidence_profile_dimension(profile: &corvid_types::effects::ComposedProfile) -> f64 {
    match profile.dimensions.get("confidence") {
        Some(corvid_ast::DimensionValue::Number(value)) => *value,
        _ => 1.0,
    }
}

fn agent_cost_budget(agent: &AgentDecl) -> Option<f64> {
    agent
        .constraints
        .iter()
        .filter(|constraint| canonical_dimension_name(&constraint.dimension.name) == "cost")
        .filter_map(numeric_constraint_value)
        .reduce(f64::min)
}

fn is_wrapping_int_binop(op: BinaryOp, ty: Option<&Type>) -> bool {
    matches!(op, BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul) && matches!(ty, Some(Type::Int))
}

fn is_wrapping_int_unop(op: UnaryOp, ty: Option<&Type>) -> bool {
    matches!(op, UnaryOp::Neg) && matches!(ty, Some(Type::Int))
}

/// Retrieve a tool's declared effect by its `DefId`.
///
/// Note: the `SymbolTable` only stores `DeclEntry`, not the full decl.
/// We don't have access to the AST here without plumbing it in, so we
/// conservatively return `Safe`. The IR also records effect on `IrTool`
/// itself, so codegen should prefer that. A refactor to flow effects
/// through the symbol table can happen when it becomes a hot path.
fn lookup_tool_effect(_symbols: &SymbolTable, _def_id: DefId) -> Effect {
    Effect::Safe
}
