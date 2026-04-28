//! Thin REPL session wrapper around one-shot type checking.

use crate::checker::{typecheck, Checked};
use crate::types::Type;
use corvid_ast::{AgentDecl, Expr, File, Ident, Literal, Param, Span, Stmt, TypeRef, Visibility};
use corvid_resolve::{Resolved, SymbolTable};

const REPL_AGENT_NAME: &str = "__repl_turn__";
pub const REPL_RESULT_NAME: &str = "__repl_value";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplLocal {
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone)]
pub struct ReplTurnBuild {
    pub agent: AgentDecl,
    pub result_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CheckedTurn {
    pub checked: Checked,
    pub build: ReplTurnBuild,
}

#[derive(Debug, Clone, Default)]
pub struct ReplSession {
    locals: Vec<ReplLocal>,
}

impl ReplSession {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn locals(&self) -> &[ReplLocal] {
        &self.locals
    }

    pub fn typecheck_decl_turn(&self, file: &File, resolved: &Resolved) -> Checked {
        typecheck(file, resolved)
    }

    pub fn commit_locals(&mut self, locals: Vec<ReplLocal>) {
        self.locals = locals;
    }

    pub fn build_stmt_turn(&self, stmt: Stmt, symbols: &SymbolTable) -> ReplTurnBuild {
        let mut stmts = vec![stmt];
        stmts.push(Stmt::Return {
            value: Some(Expr::Literal {
                value: Literal::Nothing,
                span: Span::new(0, 0),
            }),
            span: Span::new(0, 0),
        });
        ReplTurnBuild {
            agent: synthetic_agent(self.locals(), stmts, symbols),
            result_name: None,
        }
    }

    pub fn build_expr_turn(&self, expr: Expr, symbols: &SymbolTable) -> ReplTurnBuild {
        let result_ident = Ident::new(REPL_RESULT_NAME, Span::new(0, 0));
        let stmts = vec![
            Stmt::Let {
                name: result_ident,
                ty: None,
                value: expr,
                span: Span::new(0, 0),
            },
            Stmt::Return {
                value: Some(Expr::Literal {
                    value: Literal::Nothing,
                    span: Span::new(0, 0),
                }),
                span: Span::new(0, 0),
            },
        ];
        ReplTurnBuild {
            agent: synthetic_agent(self.locals(), stmts, symbols),
            result_name: Some(REPL_RESULT_NAME.to_string()),
        }
    }

    pub fn typecheck_turn(
        &self,
        file: &File,
        resolved: &Resolved,
        build: ReplTurnBuild,
    ) -> CheckedTurn {
        let checked = typecheck(file, resolved);
        CheckedTurn { checked, build }
    }
}

fn synthetic_agent(locals: &[ReplLocal], stmts: Vec<Stmt>, symbols: &SymbolTable) -> AgentDecl {
    let span = Span::new(0, 0);
    let params = locals
        .iter()
        .map(|local| Param {
            name: Ident::new(local.name.clone(), span),
            ty: type_to_type_ref(&local.ty, symbols),
            ownership: None,
            span,
        })
        .collect();
    AgentDecl {
        name: Ident::new(REPL_AGENT_NAME, span),
        extern_abi: None,
        params,
        return_ty: TypeRef::Named {
            name: Ident::new("Nothing", span),
            span,
        },
        return_ownership: None,
        body: corvid_ast::Block { stmts, span },
        effect_row: Default::default(),
        constraints: Vec::new(),
        attributes: Vec::new(),
        visibility: Visibility::Private,
        span,
    }
}

fn type_to_type_ref(ty: &Type, symbols: &SymbolTable) -> TypeRef {
    let span = Span::new(0, 0);
    match ty {
        Type::Int => named_type("Int", span),
        Type::Float => named_type("Float", span),
        Type::String => named_type("String", span),
        Type::Bool => named_type("Bool", span),
        Type::Nothing => named_type("Nothing", span),
        Type::Struct(id) => {
            let name = symbols.get(*id).name.clone();
            named_type(name, span)
        }
        Type::ImportedStruct(imported) => named_type(imported.name.clone(), span),
        Type::List(inner) => generic_type("List", vec![type_to_type_ref(inner, symbols)], span),
        Type::Stream(inner) => generic_type("Stream", vec![type_to_type_ref(inner, symbols)], span),
        Type::Result(ok, err) => generic_type(
            "Result",
            vec![
                type_to_type_ref(ok, symbols),
                type_to_type_ref(err, symbols),
            ],
            span,
        ),
        Type::Option(inner) => generic_type("Option", vec![type_to_type_ref(inner, symbols)], span),
        Type::Weak(inner, effects) => TypeRef::Weak {
            inner: Box::new(type_to_type_ref(inner, symbols)),
            effects: if effects.is_any() {
                None
            } else {
                Some(*effects)
            },
            span,
        },
        Type::Grounded(inner) => {
            generic_type("Grounded", vec![type_to_type_ref(inner, symbols)], span)
        }
        Type::Partial(inner) => {
            generic_type("Partial", vec![type_to_type_ref(inner, symbols)], span)
        }
        Type::ResumeToken(inner) => {
            generic_type("ResumeToken", vec![type_to_type_ref(inner, symbols)], span)
        }
        Type::TraceId => named_type("TraceId", span),
        Type::Function { .. } | Type::Unknown => named_type("Nothing", span),
    }
}

fn named_type(name: impl Into<String>, span: Span) -> TypeRef {
    TypeRef::Named {
        name: Ident::new(name, span),
        span,
    }
}

fn generic_type(name: &str, args: Vec<TypeRef>, span: Span) -> TypeRef {
    TypeRef::Generic {
        name: Ident::new(name, span),
        args,
        span,
    }
}
