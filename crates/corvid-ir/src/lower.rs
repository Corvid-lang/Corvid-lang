//! Lower a typed AST into IR.
//!
//! Every AST construct maps to an IR construct. References are resolved
//! via the resolver's side-table; types come from the checker's side-table.

use crate::types::*;
use corvid_ast::{
    AgentDecl, Block, Decl, Effect, Expr, ExtendMethodKind, File, Ident, ImportDecl,
    ImportSource, Literal, Param, PromptDecl, Span, Stmt, ToolDecl, TypeDecl, TypeRef,
};
use corvid_resolve::{
    resolver::MethodEntry, Binding, BuiltIn, DeclKind, DefId, LocalId, Resolved, SymbolTable,
};
use corvid_types::{Checked, Type};
use std::collections::HashMap;

/// Entry point: produce an `IrFile` from parsed/resolved/checked sources.
pub fn lower(file: &File, resolved: &Resolved, checked: &Checked) -> IrFile {
    let mut l = Lowerer::new(resolved, checked);
    l.lower_file(file)
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
}

impl<'a> Lowerer<'a> {
    fn new(resolved: &'a Resolved, checked: &'a Checked) -> Self {
        Self {
            symbols: &resolved.symbols,
            bindings: &resolved.bindings,
            types: &checked.types,
            methods: &resolved.methods,
        }
    }

    fn lower_file(&mut self, file: &File) -> IrFile {
        let mut imports = Vec::new();
        let mut types = Vec::new();
        let mut tools = Vec::new();
        let mut prompts = Vec::new();
        let mut agents = Vec::new();

        for decl in &file.decls {
            match decl {
                Decl::Import(i) => imports.push(self.lower_import(i)),
                Decl::Type(t) => types.push(self.lower_type(t)),
                Decl::Tool(t) => tools.push(self.lower_tool(t)),
                Decl::Prompt(p) => prompts.push(self.lower_prompt(p)),
                Decl::Agent(a) => agents.push(self.lower_agent(a)),
                Decl::Effect(_) => {}
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
        };
        IrImport {
            id,
            source,
            module: i.module.clone(),
            alias: alias_name,
            span: i.span,
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
            id,
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
        IrTool {
            id,
            name: t.name.name.clone(),
            params: self.lower_params(&t.params),
            return_ty: self.type_ref_to_type(&t.return_ty),
            effect: t.effect,
            effect_names: t.effect_row.effects.iter().map(|e| e.name.name.clone()).collect(),
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
        IrPrompt {
            id,
            name: p.name.name.clone(),
            params: self.lower_params(&p.params),
            return_ty: self.type_ref_to_type(&p.return_ty),
            template: p.template.clone(),
            span: p.span,
        }
    }

    fn lower_agent(&self, a: &AgentDecl) -> IrAgent {
        let id = self
            .symbols
            .lookup_def(&a.name.name)
            .expect("agent missing from symbol table");
        self.lower_agent_with_id(a, id)
    }

    fn lower_agent_with_id(&self, a: &AgentDecl, id: DefId) -> IrAgent {
        IrAgent {
            id,
            name: a.name.name.clone(),
            params: self.lower_params(&a.params),
            return_ty: self.type_ref_to_type(&a.return_ty),
            body: self.lower_block(&a.body),
            span: a.span,
            // Populated by corvid-codegen-cl's ownership pass. `None`
            // at lowering time means "every parameter is
            // Owned" at codegen (matches pre-17b semantics).
            borrow_sig: None,
        }
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
            Expr::BinOp { op, left, right, .. } => IrExprKind::BinOp {
                op: *op,
                left: Box::new(self.lower_expr(left)),
                right: Box::new(self.lower_expr(right)),
            },
            Expr::UnOp { op, operand, .. } => IrExprKind::UnOp {
                op: *op,
                operand: Box::new(self.lower_expr(operand)),
            },
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
        };
        IrExpr {
            kind,
            ty,
            span: e.span(),
        }
    }

    fn lower_ident(&self, id: &Ident) -> IrExprKind {
        match self.bindings.get(&id.span) {
            Some(Binding::Local(local_id)) => IrExprKind::Local {
                local_id: *local_id,
                name: id.name.clone(),
            },
            Some(Binding::Decl(def_id)) => IrExprKind::Decl {
                def_id: *def_id,
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
                Some(Binding::Decl(def_id)) => {
                    let entry = self.symbols.get(*def_id);
                    let kind = match entry.kind {
                        DeclKind::Tool => {
                            // Effect is stored on the AST ToolDecl; we need
                            // it at call sites. We pass `Effect::Safe` as a
                            // stable default and let IrTool carry the truth.
                            // Codegen looks up the IrTool by def_id to route.
                            IrCallKind::Tool {
                                def_id: *def_id,
                                effect: lookup_tool_effect(self.symbols, *def_id),
                            }
                        }
                        DeclKind::Prompt => IrCallKind::Prompt { def_id: *def_id },
                        DeclKind::Agent => IrCallKind::Agent { def_id: *def_id },
                        DeclKind::Type => IrCallKind::StructConstructor { def_id: *def_id },
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
        let kind = match entry.kind {
            corvid_resolve::resolver::MethodKind::Tool => IrCallKind::Tool {
                def_id: entry.def_id,
                // Method-tool effects keep `Safe` as the conservative
                // default; the IR's `IrTool` carries the
                // declared effect once `define_tool` lowers it.
                effect: Effect::Safe,
            },
            corvid_resolve::resolver::MethodKind::Prompt => IrCallKind::Prompt {
                def_id: entry.def_id,
            },
            corvid_resolve::resolver::MethodKind::Agent => IrCallKind::Agent {
                def_id: entry.def_id,
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

    fn type_ref_to_type(&self, tr: &TypeRef) -> Type {
        match tr {
            TypeRef::Named { name, .. } => match name.name.as_str() {
                "Int" => Type::Int,
                "Float" => Type::Float,
                "String" => Type::String,
                "Bool" => Type::Bool,
                "Nothing" => Type::Nothing,
                _ => match self.symbols.lookup_def(&name.name) {
                    Some(id) => Type::Struct(id),
                    None => Type::Unknown,
                },
            },
            TypeRef::Generic { name, args, .. } => match name.name.as_str() {
                "List" if args.len() == 1 => Type::List(Box::new(self.type_ref_to_type(&args[0]))),
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
