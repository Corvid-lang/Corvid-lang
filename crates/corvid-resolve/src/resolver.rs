//! Two-pass name resolver for Corvid.
//!
//! Pass 1: collect every top-level declaration into the file's symbol
//! table. Duplicates are reported and only the first wins.
//!
//! Pass 2: walk the AST and record a `Binding` for every identifier use.
//! Undefined names are reported; resolution continues.

use crate::errors::{ResolveError, ResolveErrorKind};
use crate::scope::{Binding, DeclKind, LocalId, LocalScope, SymbolTable};
use corvid_ast::{
    AgentDecl, Block, Decl, Expr, File, Ident, PromptDecl, Span, Stmt, ToolDecl, TypeDecl, TypeRef,
};
use std::collections::HashMap;

/// Output of name resolution. The AST itself is not mutated — bindings
/// live in a side table keyed by the span of each identifier use.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub symbols: SymbolTable,
    pub bindings: HashMap<Span, Binding>,
    pub errors: Vec<ResolveError>,
}

pub fn resolve(file: &File) -> Resolved {
    let mut r = Resolver::new();
    r.collect_decls(file);
    r.resolve_file(file);
    Resolved {
        symbols: r.symbols,
        bindings: r.bindings,
        errors: r.errors,
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
}

impl Resolver {
    fn new() -> Self {
        Self {
            symbols: SymbolTable::new(),
            bindings: HashMap::new(),
            errors: Vec::new(),
            scopes: Vec::new(),
            next_local_id: 0,
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

    // ---------------- pass 2 ----------------

    fn resolve_file(&mut self, file: &File) {
        for decl in &file.decls {
            match decl {
                Decl::Import(_) => {}
                Decl::Type(t) => self.resolve_type_decl(t),
                Decl::Tool(t) => self.resolve_tool_decl(t),
                Decl::Prompt(p) => self.resolve_prompt_decl(p),
                Decl::Agent(a) => self.resolve_agent_decl(a),
            }
        }
    }

    fn resolve_type_decl(&mut self, t: &TypeDecl) {
        for field in &t.fields {
            self.resolve_type_ref(&field.ty);
        }
    }

    fn resolve_tool_decl(&mut self, t: &ToolDecl) {
        for p in &t.params {
            self.resolve_type_ref(&p.ty);
        }
        self.resolve_type_ref(&t.return_ty);
        // Tools have no body. Nothing more to resolve.
    }

    fn resolve_prompt_decl(&mut self, p: &PromptDecl) {
        // Resolve param and return types. The template is a plain string;
        // interpolations are a Phase-5+ concern.
        self.push_scope();
        for param in &p.params {
            self.resolve_type_ref(&param.ty);
            let id = self.fresh_local();
            self.current_scope_mut().insert(&param.name.name, id);
            self.bindings.insert(param.name.span, Binding::Local(id));
        }
        self.resolve_type_ref(&p.return_ty);
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
        self.resolve_block(&a.body);
        self.pop_scope();
    }

    fn resolve_type_ref(&mut self, ty: &TypeRef) {
        match ty {
            TypeRef::Named { name, .. } => self.resolve_ident(name),
            TypeRef::Generic { name, args, .. } => {
                self.resolve_ident(name);
                for arg in args {
                    self.resolve_type_ref(arg);
                }
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
