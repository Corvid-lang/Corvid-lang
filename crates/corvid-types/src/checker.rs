//! The type checker and effect checker.
//!
//! Walks a parsed, resolved `File` and:
//!   * assigns a `Type` to every expression (side table, keyed by span)
//!   * validates call arities and parameter/return compatibility
//!   * enforces the approve-before-dangerous invariant
//!
//! See `ARCHITECTURE.md` §6 and `FEATURES.md` v0.1.

use crate::errors::{TypeError, TypeErrorKind};
use crate::types::Type;
use corvid_ast::{
    AgentDecl, BinaryOp, Block, Decl, Effect, Expr, File, Ident, Literal, Param, PromptDecl, Span,
    Stmt, ToolDecl, TypeDecl, TypeRef, UnaryOp,
};
use corvid_resolve::{Binding, BuiltIn, DeclKind, DefId, LocalId, Resolved, SymbolTable};
use std::collections::HashMap;

/// Output of the type checker.
#[derive(Debug, Clone)]
pub struct Checked {
    /// Type assigned to each expression, keyed by the expression's span.
    pub types: HashMap<Span, Type>,
    /// All errors found. Reporting continues past each error.
    pub errors: Vec<TypeError>,
}

pub fn typecheck(file: &File, resolved: &Resolved) -> Checked {
    let mut c = Checker::new(file, resolved);
    c.check_file(file);
    Checked {
        types: c.types,
        errors: c.errors,
    }
}

struct Checker<'a> {
    symbols: &'a SymbolTable,
    bindings: &'a HashMap<Span, Binding>,
    types: HashMap<Span, Type>,
    errors: Vec<TypeError>,

    /// Indexed declarations for O(1) lookup by DefId.
    tools_by_id: HashMap<DefId, &'a ToolDecl>,
    prompts_by_id: HashMap<DefId, &'a PromptDecl>,
    agents_by_id: HashMap<DefId, &'a AgentDecl>,
    types_by_id: HashMap<DefId, &'a TypeDecl>,

    /// Type of each local binding, populated as we enter scopes.
    local_types: HashMap<LocalId, Type>,

    /// Declared return type of the currently-checked function-like.
    current_return: Option<Type>,

    /// Approvals visible at the current point. Represented as a flat
    /// stack that is truncated back to its parent's length when a block
    /// is exited. This gives block-local effect scoping for free.
    approvals: Vec<Approval>,
}

#[derive(Debug, Clone)]
struct Approval {
    /// The user-written label (e.g. `IssueRefund`).
    label: String,
    /// Number of arguments in the approve.
    arity: usize,
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
                Decl::Type(t) => {
                    if let Some(id) = resolved.symbols.lookup_def(&t.name.name) {
                        types.insert(id, t);
                    }
                }
                Decl::Import(_) => {}
            }
        }

        Self {
            symbols: &resolved.symbols,
            bindings: &resolved.bindings,
            types: HashMap::new(),
            errors: Vec::new(),
            tools_by_id: tools,
            prompts_by_id: prompts,
            agents_by_id: agents,
            types_by_id: types,
            local_types: HashMap::new(),
            current_return: None,
            approvals: Vec::new(),
        }
    }

    // ------------------------------------------------------------
    // File-level traversal.
    // ------------------------------------------------------------

    fn check_file(&mut self, file: &File) {
        for decl in &file.decls {
            match decl {
                Decl::Agent(a) => self.check_agent(a),
                Decl::Prompt(_) | Decl::Tool(_) | Decl::Type(_) | Decl::Import(_) => {
                    // No body to check. Signatures are already well-formed
                    // by the parser, and the resolver has vetted their names.
                }
            }
        }
    }

    fn check_agent(&mut self, a: &AgentDecl) {
        // Bind parameter types.
        self.bind_params(&a.params);

        let declared_ret = self.type_ref_to_type(&a.return_ty);
        let prev_ret = std::mem::replace(&mut self.current_return, Some(declared_ret.clone()));

        self.check_block(&a.body);

        self.current_return = prev_ret;
        // (Locals leak between agents in our single-scope model; harmless
        //  since each agent binds its params fresh at the start.)
    }

    fn bind_params(&mut self, params: &[Param]) {
        for p in params {
            if let Some(Binding::Local(local_id)) = self.bindings.get(&p.name.span) {
                let ty = self.type_ref_to_type(&p.ty);
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
                let value_ty = self.check_expr(value);
                let local_ty = match ty {
                    Some(t) => self.type_ref_to_type(t),
                    None => value_ty.clone(),
                };
                if let Some(Binding::Local(local_id)) = self.bindings.get(&name.span) {
                    self.local_types.insert(*local_id, local_ty);
                }
            }
            Stmt::Return { value, span } => {
                let got = match value {
                    Some(e) => self.check_expr(e),
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
                self.check_block(then_block);
                if let Some(b) = else_block {
                    self.check_block(b);
                }
            }
            Stmt::For { var, iter, body, .. } => {
                let _iter_ty = self.check_expr(iter);
                // For loop variable: leave as Unknown in v0.1 (no full
                // inference of iterable element types yet).
                if let Some(Binding::Local(local_id)) = self.bindings.get(&var.span) {
                    self.local_types.insert(*local_id, Type::Unknown);
                }
                self.check_block(body);
            }
            Stmt::Approve { action, .. } => {
                self.check_approve(action);
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
        let ty = match e {
            Expr::Literal { value, .. } => match value {
                Literal::Int(_) => Type::Int,
                Literal::Float(_) => Type::Float,
                Literal::String(_) => Type::String,
                Literal::Bool(_) => Type::Bool,
                Literal::Nothing => Type::Nothing,
            },
            Expr::Ident { name, .. } => self.type_of_ident(name),
            Expr::Call { callee, args, span } => self.check_call(callee, args, *span),
            Expr::FieldAccess { target, field, span } => self.check_field(target, field, *span),
            Expr::Index { target, index, .. } => {
                let _ = self.check_expr(target);
                let _ = self.check_expr(index);
                Type::Unknown // list/map element typing deferred
            }
            Expr::BinOp { op, left, right, span } => self.check_binop(*op, left, right, *span),
            Expr::UnOp { op, operand, .. } => self.check_unop(*op, operand),
            Expr::List { items, .. } => {
                for item in items {
                    let _ = self.check_expr(item);
                }
                Type::Unknown // homogeneity check deferred
            }
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
                | BuiltIn::Nothing => {
                    self.errors.push(TypeError::new(
                        TypeErrorKind::TypeAsValue {
                            name: id.name.clone(),
                        },
                        id.span,
                    ));
                    Type::Unknown
                }
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
            DeclKind::Import => Type::Unknown,
        }
    }

    fn check_call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> Type {
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

        let Binding::Decl(def_id) = binding else {
            self.errors.push(TypeError::new(
                TypeErrorKind::NotCallable {
                    got: "<local value>".into(),
                },
                callee.span(),
            ));
            return Type::Unknown;
        };

        let def_id = *def_id;
        let entry = self.symbols.get(def_id);

        match entry.kind {
            DeclKind::Tool => self.check_tool_call(def_id, &name.name, args, span),
            DeclKind::Prompt => self.check_prompt_call(def_id, &name.name, args),
            DeclKind::Agent => self.check_agent_call(def_id, &name.name, args),
            DeclKind::Import => {
                for a in args {
                    let _ = self.check_expr(a);
                }
                Type::Unknown
            }
            DeclKind::Type => {
                // Type used as constructor — out of scope for v0.1.
                self.errors.push(TypeError::new(
                    TypeErrorKind::TypeAsValue {
                        name: name.name.clone(),
                    },
                    name.span,
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
        self.type_ref_to_type(&agent.return_ty)
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
            let arg_ty = self.check_expr(arg);
            if let Some(param) = params.get(i) {
                let param_ty = self.type_ref_to_type(&param.ty);
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

    // ------------------------------------------------------------
    // Type-reference resolution (TypeRef → Type).
    // ------------------------------------------------------------

    fn type_ref_to_type(&self, tr: &TypeRef) -> Type {
        match tr {
            TypeRef::Named { name, .. } => self.named_type_to_type(&name.name),
            TypeRef::Generic { .. } | TypeRef::Function { .. } => Type::Unknown,
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
