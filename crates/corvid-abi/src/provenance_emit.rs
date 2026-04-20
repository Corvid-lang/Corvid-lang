use crate::schema::AbiProvenanceContract;
use corvid_ast::{AgentDecl, Block, Expr, Stmt};
use corvid_types::Type;

pub fn emit_provenance_contract(agent: &AgentDecl, return_ty: &Type) -> AbiProvenanceContract {
    let returns_grounded = matches!(return_ty, Type::Grounded(_));
    let mut grounded_param_deps = Vec::new();
    if returns_grounded {
        collect_block_dependencies(&agent.body, &mut grounded_param_deps);
        let param_names = agent
            .params
            .iter()
            .map(|param| param.name.name.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        grounded_param_deps.retain(|dep| param_names.contains(dep.as_str()));
        grounded_param_deps.sort();
        grounded_param_deps.dedup();
    }
    AbiProvenanceContract {
        returns_grounded,
        grounded_param_deps,
    }
}

fn collect_block_dependencies(block: &Block, out: &mut Vec<String>) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let { value, .. }
            | Stmt::Yield { value, .. }
            | Stmt::Expr { expr: value, .. }
            | Stmt::Return {
                value: Some(value), ..
            } => collect_expr_dependencies(value, out),
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                collect_expr_dependencies(cond, out);
                collect_block_dependencies(then_block, out);
                if let Some(else_block) = else_block {
                    collect_block_dependencies(else_block, out);
                }
            }
            Stmt::For { iter, body, .. } => {
                collect_expr_dependencies(iter, out);
                collect_block_dependencies(body, out);
            }
            Stmt::Approve { action, .. } => collect_expr_dependencies(action, out),
            Stmt::Return { value: None, .. } => {}
        }
    }
}

fn collect_expr_dependencies(expr: &Expr, out: &mut Vec<String>) {
    match expr {
        Expr::Literal { .. } => {}
        Expr::Ident { name, .. } => out.push(name.name.clone()),
        Expr::Call { callee, args, .. } => {
            collect_expr_dependencies(callee, out);
            for arg in args {
                collect_expr_dependencies(arg, out);
            }
        }
        Expr::FieldAccess { target, .. } => collect_expr_dependencies(target, out),
        Expr::Index { target, index, .. } => {
            collect_expr_dependencies(target, out);
            collect_expr_dependencies(index, out);
        }
        Expr::BinOp { left, right, .. } => {
            collect_expr_dependencies(left, out);
            collect_expr_dependencies(right, out);
        }
        Expr::UnOp { operand, .. } => collect_expr_dependencies(operand, out),
        Expr::List { items, .. } => {
            for item in items {
                collect_expr_dependencies(item, out);
            }
        }
        Expr::TryPropagate { inner, .. } => collect_expr_dependencies(inner, out),
        Expr::TryRetry { body, .. } => collect_expr_dependencies(body, out),
        Expr::Replay {
            trace,
            arms,
            else_body,
            ..
        } => {
            collect_expr_dependencies(trace, out);
            for arm in arms {
                collect_expr_dependencies(&arm.body, out);
            }
            collect_expr_dependencies(else_body, out);
        }
    }
}
