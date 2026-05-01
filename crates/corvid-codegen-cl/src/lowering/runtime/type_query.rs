//! Type-shape predicates and IR walkers used across the runtime
//! lowering passes — `is_refcounted_type`, native-layout
//! classifiers, retain/release call emitters, the `mangle_type_name`
//! symbol synthesiser, and the `collect_*_types` walkers that
//! enumerate every list-element / result / option type used in
//! an IR file (so per-type destructors / trace fns / typeinfo
//! blobs can be emitted exactly once).

use super::*;

pub fn option_uses_wrapper(option_ty: &Type) -> bool {
    match option_ty {
        Type::Option(inner) => {
            is_native_wide_option_type(option_ty) || matches!(&**inner, Type::Option(_))
        }
        _ => false,
    }
}








/// Stable, link-safe string from a Corvid `Type` for use in typeinfo
/// symbol names. `List<List<String>>` → `List_List_String`, etc.
pub fn mangle_type_name(ty: &Type) -> String {
    match ty {
        Type::Int => "Int".into(),
        Type::Float => "Float".into(),
        Type::Bool => "Bool".into(),
        Type::String => "String".into(),
        Type::Nothing => "Nothing".into(),
        Type::List(inner) => format!("List_{}", mangle_type_name(inner)),
        Type::Stream(inner) => format!("Stream_{}", mangle_type_name(inner)),
        Type::Struct(def_id) => format!("Struct_{}", def_id.0),
        Type::ImportedStruct(imported) => {
            format!(
                "ImportedStruct_{}_{}",
                imported.module_path.replace(['\\', '/', ':'], "_"),
                imported.def_id.0
            )
        }
        Type::Function { .. } => "Function".into(),
        // Result<T,E> and Option<T> are compiler-known
        // tagged unions. Their typeinfo emission (and full native
        // codegen) lands in 18d. For 17c we just need the mangler
        // to terminate; the resulting names won't be used at runtime
        // because programs touching these types fail at the
        // `lower_expr` codegen step below before reaching emission.
        Type::Result(ok, err) => {
            format!("Result_{}_{}", mangle_type_name(ok), mangle_type_name(err))
        }
        Type::Option(inner) => format!("Option_{}", mangle_type_name(inner)),
        Type::Grounded(inner) => format!("Grounded_{}", mangle_type_name(inner)),
        Type::Partial(inner) => format!("Partial_{}", mangle_type_name(inner)),
        Type::ResumeToken(inner) => format!("ResumeToken_{}", mangle_type_name(inner)),
        Type::Weak(inner, effects) => {
            if effects.is_any() {
                format!("Weak_{}", mangle_type_name(inner))
            } else {
                let suffix: Vec<&'static str> = effects
                    .effects()
                    .into_iter()
                    .map(|effect| match effect {
                        corvid_ast::WeakEffect::ToolCall => "tool_call",
                        corvid_ast::WeakEffect::Llm => "llm",
                        corvid_ast::WeakEffect::Approve => "approve",
                        corvid_ast::WeakEffect::Human => "human",
                    })
                    .collect();
                format!("Weak_{}_{}", mangle_type_name(inner), suffix.join("_"))
            }
        }
        Type::TraceId => "TraceId".into(),
        Type::RouteParams(_) => "RouteParams".into(),
        Type::Unknown => "Unknown".into(),
    }
}

/// Walk every `Type::List(_)` the IR mentions (agent sigs,
/// struct fields, tool/prompt sigs, expression types) and produce the
/// set of unique list element types in a dependency-friendly order:
/// element types come before lists that contain them.
///
/// The returned `Vec<Type>` holds the *element* type of each list
/// (not the `List<T>` type itself). Emission iterates this vec
/// creating one `corvid_typeinfo_List_<elem>` per entry.
pub fn collect_list_element_types(ir: &IrFile) -> Vec<Type> {
    use std::collections::BTreeSet;
    let mut seen: BTreeSet<Type> = BTreeSet::new();
    let mut order: Vec<Type> = Vec::new();

    fn visit(ty: &Type, seen: &mut BTreeSet<Type>, order: &mut Vec<Type>) {
        match ty {
            Type::List(inner) => {
                // Recurse first so inner list types get their
                // typeinfo emitted BEFORE the outer list references
                // them via elem_typeinfo relocation.
                visit(inner, seen, order);
                let elem = (**inner).clone();
                if seen.insert(elem.clone()) {
                    order.push(elem);
                }
            }
            Type::Result(ok, err) => {
                visit(ok, seen, order);
                visit(err, seen, order);
            }
            Type::Option(inner)
            | Type::Partial(inner)
            | Type::ResumeToken(inner)
            | Type::Weak(inner, _) => visit(inner, seen, order),
            Type::Function { params, ret, .. } => {
                for param in params {
                    visit(param, seen, order);
                }
                visit(ret, seen, order);
            }
            _ => {}
        }
    }

    for agent in &ir.agents {
        for param in &agent.params {
            visit(&param.ty, &mut seen, &mut order);
        }
        visit(&agent.return_ty, &mut seen, &mut order);
        visit_block_types(&agent.body, &mut seen, &mut order, &visit);
    }
    for ty in &ir.types {
        for field in &ty.fields {
            visit(&field.ty, &mut seen, &mut order);
        }
    }
    for tool in &ir.tools {
        for param in &tool.params {
            visit(&param.ty, &mut seen, &mut order);
        }
        visit(&tool.return_ty, &mut seen, &mut order);
    }
    for prompt in &ir.prompts {
        for param in &prompt.params {
            visit(&param.ty, &mut seen, &mut order);
        }
        visit(&prompt.return_ty, &mut seen, &mut order);
    }

    order
}

pub fn collect_result_types(ir: &IrFile) -> Vec<Type> {
    use std::collections::BTreeSet;
    let mut seen: BTreeSet<Type> = BTreeSet::new();
    let mut order: Vec<Type> = Vec::new();

    fn visit(ty: &Type, seen: &mut BTreeSet<Type>, order: &mut Vec<Type>) {
        match ty {
            Type::Result(ok, err) => {
                visit(ok, seen, order);
                visit(err, seen, order);
                if seen.insert(ty.clone()) {
                    order.push(ty.clone());
                }
            }
            Type::List(inner)
            | Type::Option(inner)
            | Type::Partial(inner)
            | Type::ResumeToken(inner)
            | Type::Weak(inner, _) => {
                visit(inner, seen, order);
            }
            Type::Function { params, ret, .. } => {
                for param in params {
                    visit(param, seen, order);
                }
                visit(ret, seen, order);
            }
            _ => {}
        }
    }

    for agent in &ir.agents {
        for param in &agent.params {
            visit(&param.ty, &mut seen, &mut order);
        }
        visit(&agent.return_ty, &mut seen, &mut order);
        visit_block_types(&agent.body, &mut seen, &mut order, &visit);
    }
    for ty in &ir.types {
        for field in &ty.fields {
            visit(&field.ty, &mut seen, &mut order);
        }
    }
    for tool in &ir.tools {
        for param in &tool.params {
            visit(&param.ty, &mut seen, &mut order);
        }
        visit(&tool.return_ty, &mut seen, &mut order);
    }
    for prompt in &ir.prompts {
        for param in &prompt.params {
            visit(&param.ty, &mut seen, &mut order);
        }
        visit(&prompt.return_ty, &mut seen, &mut order);
    }

    order
}

pub fn collect_option_types(ir: &IrFile) -> Vec<Type> {
    use std::collections::BTreeSet;
    let mut seen: BTreeSet<Type> = BTreeSet::new();
    let mut order: Vec<Type> = Vec::new();

    fn visit(ty: &Type, seen: &mut BTreeSet<Type>, order: &mut Vec<Type>) {
        match ty {
            Type::Option(inner) => {
                visit(inner, seen, order);
                if option_uses_wrapper(ty) && seen.insert(ty.clone()) {
                    order.push(ty.clone());
                }
            }
            Type::Result(ok, err) => {
                visit(ok, seen, order);
                visit(err, seen, order);
            }
            Type::List(inner)
            | Type::Partial(inner)
            | Type::ResumeToken(inner)
            | Type::Weak(inner, _) => visit(inner, seen, order),
            Type::Function { params, ret, .. } => {
                for param in params {
                    visit(param, seen, order);
                }
                visit(ret, seen, order);
            }
            _ => {}
        }
    }

    for agent in &ir.agents {
        for param in &agent.params {
            visit(&param.ty, &mut seen, &mut order);
        }
        visit(&agent.return_ty, &mut seen, &mut order);
        visit_block_types(&agent.body, &mut seen, &mut order, &visit);
    }
    for ty in &ir.types {
        for field in &ty.fields {
            visit(&field.ty, &mut seen, &mut order);
        }
    }
    for tool in &ir.tools {
        for param in &tool.params {
            visit(&param.ty, &mut seen, &mut order);
        }
        visit(&tool.return_ty, &mut seen, &mut order);
    }
    for prompt in &ir.prompts {
        for param in &prompt.params {
            visit(&param.ty, &mut seen, &mut order);
        }
        visit(&prompt.return_ty, &mut seen, &mut order);
    }

    order
}

/// Walk an `IrBlock` and visit every expression's `ty` through the
/// caller's closure. Catches list literals and other list-producing
/// expressions that don't surface in signatures.
fn visit_block_types(
    block: &IrBlock,
    seen: &mut std::collections::BTreeSet<Type>,
    order: &mut Vec<Type>,
    visit: &dyn Fn(&Type, &mut std::collections::BTreeSet<Type>, &mut Vec<Type>),
) {
    for stmt in &block.stmts {
        match stmt {
            IrStmt::Let { value, ty, .. } => {
                visit(ty, seen, order);
                visit_expr_types(value, seen, order, visit);
            }
            IrStmt::Expr { expr, .. } => visit_expr_types(expr, seen, order, visit),
            IrStmt::Yield { value, .. } => visit_expr_types(value, seen, order, visit),
            IrStmt::Return { value: Some(e), .. } => visit_expr_types(e, seen, order, visit),
            IrStmt::Return { value: None, .. } => {}
            IrStmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                visit_expr_types(cond, seen, order, visit);
                visit_block_types(then_block, seen, order, visit);
                if let Some(eb) = else_block {
                    visit_block_types(eb, seen, order, visit);
                }
            }
            IrStmt::For { iter, body, .. } => {
                visit_expr_types(iter, seen, order, visit);
                visit_block_types(body, seen, order, visit);
            }
            IrStmt::Approve { args, .. } => {
                for a in args {
                    visit_expr_types(a, seen, order, visit);
                }
            }
            IrStmt::Break { .. } | IrStmt::Continue { .. } | IrStmt::Pass { .. } => {}
            IrStmt::Dup { .. } | IrStmt::Drop { .. } => {}
        }
    }
}

fn visit_expr_types(
    e: &IrExpr,
    seen: &mut std::collections::BTreeSet<Type>,
    order: &mut Vec<Type>,
    visit: &dyn Fn(&Type, &mut std::collections::BTreeSet<Type>, &mut Vec<Type>),
) {
    visit(&e.ty, seen, order);
    match &e.kind {
        IrExprKind::Literal(_) | IrExprKind::Local { .. } | IrExprKind::Decl { .. } => {}
        IrExprKind::BinOp { left, right, .. } | IrExprKind::WrappingBinOp { left, right, .. } => {
            visit_expr_types(left, seen, order, visit);
            visit_expr_types(right, seen, order, visit);
        }
        IrExprKind::UnOp { operand, .. } | IrExprKind::WrappingUnOp { operand, .. } => {
            visit_expr_types(operand, seen, order, visit);
        }
        IrExprKind::Call { args, .. } => {
            for a in args {
                visit_expr_types(a, seen, order, visit);
            }
        }
        IrExprKind::FieldAccess { target, .. } => {
            visit_expr_types(target, seen, order, visit);
        }
        IrExprKind::UnwrapGrounded { value } => {
            visit_expr_types(value, seen, order, visit);
        }
        IrExprKind::Index { target, index } => {
            visit_expr_types(target, seen, order, visit);
            visit_expr_types(index, seen, order, visit);
        }
        IrExprKind::List { items } => {
            for el in items {
                visit_expr_types(el, seen, order, visit);
            }
        }
        // Result/Option/?/try-retry IR variants. The
        // visit_expr_types pass collects list-element types for
        // typeinfo emission. Result/Option don't appear in list-
        // element positions in 17c (their codegen lands in 18d),
        // but we still recurse into their sub-expressions so any
        // List<T> *nested* inside them is still seen.
        IrExprKind::WeakNew { strong: inner }
        | IrExprKind::WeakUpgrade { weak: inner }
        | IrExprKind::StreamSplitBy { stream: inner, .. }
        | IrExprKind::StreamMerge { groups: inner, .. }
        | IrExprKind::StreamOrderedBy { stream: inner, .. }
        | IrExprKind::StreamResumeToken { stream: inner }
        | IrExprKind::ResumeStream { token: inner, .. }
        | IrExprKind::ResultOk { inner }
        | IrExprKind::ResultErr { inner }
        | IrExprKind::OptionSome { inner }
        | IrExprKind::Ask { prompt: inner, .. }
        | IrExprKind::Choose { options: inner }
        | IrExprKind::TryPropagate { inner } => {
            visit_expr_types(inner, seen, order, visit);
        }
        IrExprKind::OptionNone => {}
        IrExprKind::TryRetry { body, .. } => {
            visit_expr_types(body, seen, order, visit);
        }
        IrExprKind::Replay {
            trace,
            arms,
            else_body,
        } => {
            visit_expr_types(trace, seen, order, visit);
            for arm in arms {
                visit_expr_types(&arm.body, seen, order, visit);
            }
            visit_expr_types(else_body, seen, order, visit);
        }
    }
}

/// Helper: emit `corvid_retain(value)` if the value is refcounted
/// (i.e., non-immortal at runtime). Caller decides whether the value
/// needs ownership at this point — the helper just emits the call.
pub fn emit_retain(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    v: ClValue,
) {
    let callee = module.declare_func_in_func(runtime.retain, builder.func);
    builder.ins().call(callee, &[v]);
}

/// Helper: emit `corvid_release(value)`.
pub fn emit_release(
    builder: &mut FunctionBuilder,
    module: &mut ObjectModule,
    runtime: &RuntimeFuncs,
    v: ClValue,
) {
    let callee = module.declare_func_in_func(runtime.release, builder.func);
    builder.ins().call(callee, &[v]);
}

/// Whether a Corvid value of this type lives behind a refcounted heap
/// allocation. When true, the codegen tracks ownership: `retain` on
/// bind, `release` on scope exit, etc.
///
/// Today `String` is refcounted. Future work extends this to `Struct`
/// (12f), `List` (12g) — both will return true here.
pub fn is_refcounted_type(ty: &Type) -> bool {
    match ty {
        Type::String
        | Type::Struct(_)
        | Type::ImportedStruct(_)
        | Type::List(_)
        | Type::Weak(_, _)
        | Type::Result(_, _)
        | Type::Partial(_)
        | Type::ResumeToken(_) => true,
        Type::Option(inner) => is_native_wide_option_type(ty) || is_refcounted_type(inner),
        Type::Grounded(inner) => is_refcounted_type(inner),
        _ => false,
    }
}

pub fn is_native_value_type(ty: &Type) -> bool {
    match ty {
        Type::Int | Type::Bool | Type::Float | Type::String => true,
        Type::Struct(_) | Type::ImportedStruct(_) | Type::List(_) | Type::Weak(_, _) => true,
        Type::Option(_) => is_native_option_type(ty),
        Type::Result(ok, err) => is_native_value_type(ok) && is_native_value_type(err),
        Type::Grounded(inner) => is_native_value_type(inner),
        // TraceId is a string-backed opaque handle at runtime;
        // treat it as a value type for native emission purposes.
        Type::TraceId => true,
        Type::Nothing
        | Type::Function { .. }
        | Type::RouteParams(_)
        | Type::Stream(_)
        | Type::Partial(_)
        | Type::ResumeToken(_)
        | Type::Unknown => false,
    }
}

pub fn is_native_wide_option_type(ty: &Type) -> bool {
    matches!(ty, Type::Option(inner) if matches!(&**inner, Type::Int | Type::Bool | Type::Float))
}

pub fn is_native_option_type(ty: &Type) -> bool {
    match ty {
        Type::Option(inner) => is_refcounted_type(inner) || is_native_wide_option_type(ty),
        _ => false,
    }
}

pub fn is_native_option_expr_type(ty: &Type) -> bool {
    matches!(ty, Type::Option(inner) if matches!(**inner, Type::Unknown))
        || is_native_option_type(ty)
}

pub fn is_native_result_type(ty: &Type) -> bool {
    matches!(ty, Type::Result(ok, err) if is_native_value_type(ok) && is_native_value_type(err))
}
