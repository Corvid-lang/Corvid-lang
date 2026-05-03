use anyhow::{anyhow, bail, Result};
use corvid_ast::{
    AgentDecl, BinaryOp, Block, Decl, Expr, File, Ident, Literal, Span, Stmt, UnaryOp,
};
use corvid_resolve::{build_dep_graph, resolve, Binding, LocalId, Resolved};
use corvid_syntax::{lex, parse_file};
use std::collections::BTreeSet;

pub use crate::render::render_file;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewriteRule {
    AlphaConversion,
    LetExtract,
    LetInline,
    CommutativeSiblingSwap,
    TopLevelReorder,
    IfBranchSwap,
    ConstantFolding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LawRef {
    pub name: &'static str,
    pub rationale: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewriteResult {
    pub rule: RewriteRule,
    pub source: String,
    pub changed: bool,
    pub law: LawRef,
}

pub fn rule_name(rule: RewriteRule) -> &'static str {
    match rule {
        RewriteRule::AlphaConversion => "alpha-conversion",
        RewriteRule::LetExtract => "let-extract",
        RewriteRule::LetInline => "let-inline",
        RewriteRule::CommutativeSiblingSwap => "commutative-sibling-swap",
        RewriteRule::TopLevelReorder => "top-level-reorder",
        RewriteRule::IfBranchSwap => "if-branch-swap",
        RewriteRule::ConstantFolding => "constant-folding",
    }
}

pub fn rewrite_rules() -> &'static [RewriteRule] {
    &[
        RewriteRule::AlphaConversion,
        RewriteRule::LetExtract,
        RewriteRule::LetInline,
        RewriteRule::CommutativeSiblingSwap,
        RewriteRule::TopLevelReorder,
        RewriteRule::IfBranchSwap,
        RewriteRule::ConstantFolding,
    ]
}

pub fn law_ref(rule: RewriteRule) -> LawRef {
    match rule {
        RewriteRule::AlphaConversion => LawRef {
            name: "alpha-equivalence",
            rationale: "Effect rows are name-agnostic over resolver-stable LocalIds.",
        },
        RewriteRule::LetExtract => LawRef {
            name: "binder-introduction",
            rationale: "Introducing a binder for a pure expression preserves the composed row.",
        },
        RewriteRule::LetInline => LawRef {
            name: "binder-elimination",
            rationale: "Inlining a pure single-use binder preserves the composed row.",
        },
        RewriteRule::CommutativeSiblingSwap => LawRef {
            name: "locality",
            rationale: "Independent pure sibling bindings compose through a commutative row operator.",
        },
        RewriteRule::TopLevelReorder => LawRef {
            name: "declaration-order-irrelevance",
            rationale: "Top-level declaration order is not type-relevant when dependencies do not cross.",
        },
        RewriteRule::IfBranchSwap => LawRef {
            name: "branch-symmetry",
            rationale: "Swapping same-context branches while negating the guard preserves the branch join row.",
        },
        RewriteRule::ConstantFolding => LawRef {
            name: "constant-subexpression-equivalence",
            rationale: "Literal-only subexpressions carry an empty row, so folding them preserves effects.",
        },
    }
}

pub fn apply_rewrite(source: &str, rule: RewriteRule) -> Result<RewriteResult> {
    let mut file = parse_source(source)?;
    let changed = match rule {
        RewriteRule::AlphaConversion => alpha_convert(&mut file)?,
        RewriteRule::LetExtract => let_extract(&mut file)?,
        RewriteRule::LetInline => let_inline(&mut file)?,
        RewriteRule::CommutativeSiblingSwap => commutative_sibling_swap(&mut file)?,
        RewriteRule::TopLevelReorder => top_level_reorder(&mut file)?,
        RewriteRule::IfBranchSwap => if_branch_swap(&mut file)?,
        RewriteRule::ConstantFolding => constant_folding(&mut file)?,
    };
    Ok(RewriteResult {
        rule,
        source: render_file(&file),
        changed,
        law: law_ref(rule),
    })
}

pub fn parse_source(source: &str) -> Result<File> {
    let tokens = lex(source).map_err(|errs| anyhow!("lex failed: {errs:?}"))?;
    let (file, parse_errors) = parse_file(&tokens);
    if !parse_errors.is_empty() {
        bail!("parse failed: {parse_errors:?}");
    }
    Ok(file)
}

fn alpha_convert(file: &mut File) -> Result<bool> {
    let resolved = resolve(file);
    if !resolved.errors.is_empty() {
        bail!(
            "resolve failed before alpha-conversion: {:?}",
            resolved.errors
        );
    }
    for decl in &mut file.decls {
        let Decl::Agent(agent) = decl else { continue };
        let mut used_names = collect_agent_local_names(agent);
        let mut candidate = None;
        collect_rename_candidate_from_block(&agent.body, &resolved, &mut candidate);
        let Some((target, original_name)) = candidate else {
            continue;
        };
        let fresh = fresh_name(&used_names, &format!("{original_name}_alpha"));
        used_names.insert(fresh.clone());
        rename_local_in_agent(agent, &resolved, target, &fresh);
        return Ok(true);
    }
    Ok(false)
}

fn let_extract(file: &mut File) -> Result<bool> {
    let mut allocator = SpanAllocator::new(file);
    let mut reserved = collect_all_names(file);
    for decl in &mut file.decls {
        let Decl::Agent(agent) = decl else { continue };
        if extract_from_block(&mut agent.body, &mut allocator, &mut reserved) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn let_inline(file: &mut File) -> Result<bool> {
    let resolved = resolve(file);
    if !resolved.errors.is_empty() {
        bail!("resolve failed before let-inline: {:?}", resolved.errors);
    }

    for decl in &mut file.decls {
        let Decl::Agent(agent) = decl else { continue };
        let binding_counts = collect_local_binding_counts(agent, &resolved);
        if inline_in_block(&mut agent.body, &resolved, &binding_counts) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn commutative_sibling_swap(file: &mut File) -> Result<bool> {
    let resolved = resolve(file);
    if !resolved.errors.is_empty() {
        bail!(
            "resolve failed before commutative-sibling-swap: {:?}",
            resolved.errors
        );
    }
    for decl in &mut file.decls {
        let Decl::Agent(agent) = decl else { continue };
        if swap_independent_lets_in_block(&mut agent.body, &resolved) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn top_level_reorder(file: &mut File) -> Result<bool> {
    let resolved = resolve(file);
    if !resolved.errors.is_empty() {
        bail!(
            "resolve failed before top-level-reorder: {:?}",
            resolved.errors
        );
    }
    let graph = build_dep_graph(file, &resolved);
    for index in 0..file.decls.len().saturating_sub(1) {
        if can_reorder_top_level_pair(
            &file.decls[index],
            &file.decls[index + 1],
            file,
            &resolved,
            &graph,
        ) {
            file.decls.swap(index, index + 1);
            return Ok(true);
        }
    }
    Ok(false)
}

fn if_branch_swap(file: &mut File) -> Result<bool> {
    let mut allocator = SpanAllocator::new(file);
    for decl in &mut file.decls {
        let Decl::Agent(agent) = decl else { continue };
        if swap_if_branches_in_block(&mut agent.body, &mut allocator) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn constant_folding(file: &mut File) -> Result<bool> {
    for decl in &mut file.decls {
        let Decl::Agent(agent) = decl else { continue };
        if fold_constants_in_block(&mut agent.body) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn collect_rename_candidate_from_block(
    block: &Block,
    resolved: &Resolved,
    candidate: &mut Option<(LocalId, String)>,
) {
    for stmt in &block.stmts {
        if candidate.is_some() {
            return;
        }
        match stmt {
            Stmt::Let { name, .. } => {
                if let Some(Binding::Local(id)) = resolved.bindings.get(&name.span) {
                    *candidate = Some((*id, name.name.clone()));
                    return;
                }
            }
            Stmt::If {
                then_block,
                else_block,
                ..
            } => {
                collect_rename_candidate_from_block(then_block, resolved, candidate);
                if candidate.is_some() {
                    return;
                }
                if let Some(block) = else_block {
                    collect_rename_candidate_from_block(block, resolved, candidate);
                }
            }
            Stmt::For { body, .. } => {
                collect_rename_candidate_from_block(body, resolved, candidate)
            }
            _ => {}
        }
    }
}

fn rename_local_in_agent(agent: &mut AgentDecl, resolved: &Resolved, target: LocalId, fresh: &str) {
    for param in &mut agent.params {
        rename_ident_if_matches(&mut param.name, resolved, target, fresh);
    }
    rename_local_in_block(&mut agent.body, resolved, target, fresh);
}

fn rename_local_in_block(block: &mut Block, resolved: &Resolved, target: LocalId, fresh: &str) {
    for stmt in &mut block.stmts {
        match stmt {
            Stmt::Let { name, value, .. } => {
                rename_ident_if_matches(name, resolved, target, fresh);
                rename_local_in_expr(value, resolved, target, fresh);
            }
            Stmt::Return { value, .. } => {
                if let Some(expr) = value {
                    rename_local_in_expr(expr, resolved, target, fresh);
                }
            }
            Stmt::Yield { value, .. } => rename_local_in_expr(value, resolved, target, fresh),
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                rename_local_in_expr(cond, resolved, target, fresh);
                rename_local_in_block(then_block, resolved, target, fresh);
                if let Some(block) = else_block {
                    rename_local_in_block(block, resolved, target, fresh);
                }
            }
            Stmt::For {
                var, iter, body, ..
            } => {
                rename_ident_if_matches(var, resolved, target, fresh);
                rename_local_in_expr(iter, resolved, target, fresh);
                rename_local_in_block(body, resolved, target, fresh);
            }
            Stmt::Approve { action, .. } => rename_local_in_expr(action, resolved, target, fresh),
            Stmt::Expr { expr, .. } => rename_local_in_expr(expr, resolved, target, fresh),
        }
    }
}

fn rename_local_in_expr(expr: &mut Expr, resolved: &Resolved, target: LocalId, fresh: &str) {
    match expr {
        Expr::Literal { .. } => {}
        Expr::Ident { name, .. } => rename_ident_if_matches(name, resolved, target, fresh),
        Expr::Call { callee, args, .. } => {
            rename_local_in_expr(callee, resolved, target, fresh);
            for arg in args {
                rename_local_in_expr(arg, resolved, target, fresh);
            }
        }
        Expr::FieldAccess { target: inner, .. } => {
            rename_local_in_expr(inner, resolved, target, fresh);
        }
        Expr::Index {
            target: inner,
            index,
            ..
        } => {
            rename_local_in_expr(inner, resolved, target, fresh);
            rename_local_in_expr(index, resolved, target, fresh);
        }
        Expr::BinOp { left, right, .. } => {
            rename_local_in_expr(left, resolved, target, fresh);
            rename_local_in_expr(right, resolved, target, fresh);
        }
        Expr::UnOp { operand, .. } => rename_local_in_expr(operand, resolved, target, fresh),
        Expr::List { items, .. } => {
            for item in items {
                rename_local_in_expr(item, resolved, target, fresh);
            }
        }
        Expr::TryPropagate { inner, .. } => rename_local_in_expr(inner, resolved, target, fresh),
        Expr::TryRetry { body, .. } => rename_local_in_expr(body, resolved, target, fresh),
        Expr::Replay {
            trace,
            arms,
            else_body,
            ..
        } => {
            rename_local_in_expr(trace, resolved, target, fresh);
            for arm in arms {
                rename_local_in_expr(&mut arm.body, resolved, target, fresh);
            }
            rename_local_in_expr(else_body, resolved, target, fresh);
        }
    }
}

fn rename_ident_if_matches(id: &mut Ident, resolved: &Resolved, target: LocalId, fresh: &str) {
    if matches!(resolved.bindings.get(&id.span), Some(Binding::Local(local)) if *local == target) {
        id.name = fresh.to_string();
    }
}

fn extract_from_block(
    block: &mut Block,
    allocator: &mut SpanAllocator,
    reserved: &mut BTreeSet<String>,
) -> bool {
    let mut index = 0;
    while index < block.stmts.len() {
        if let Some(extracted) = extract_from_stmt(&mut block.stmts[index], allocator, reserved) {
            block.stmts.insert(index, extracted);
            return true;
        }
        match &mut block.stmts[index] {
            Stmt::If {
                then_block,
                else_block,
                ..
            } => {
                if extract_from_block(then_block, allocator, reserved) {
                    return true;
                }
                if let Some(block) = else_block {
                    if extract_from_block(block, allocator, reserved) {
                        return true;
                    }
                }
            }
            Stmt::For { body, .. } => {
                if extract_from_block(body, allocator, reserved) {
                    return true;
                }
            }
            _ => {}
        }
        index += 1;
    }
    false
}

fn extract_from_stmt(
    stmt: &mut Stmt,
    allocator: &mut SpanAllocator,
    reserved: &mut BTreeSet<String>,
) -> Option<Stmt> {
    match stmt {
        Stmt::Let { value, .. } => extract_expr_to_let(value, allocator, reserved),
        Stmt::Return { value, .. } => value
            .as_mut()
            .and_then(|expr| extract_expr_to_let(expr, allocator, reserved)),
        Stmt::Yield { value, .. } => extract_expr_to_let(value, allocator, reserved),
        Stmt::If { cond, .. } => extract_expr_to_let(cond, allocator, reserved),
        Stmt::For { iter, .. } => extract_expr_to_let(iter, allocator, reserved),
        Stmt::Expr { expr, .. } => extract_expr_to_let(expr, allocator, reserved),
        Stmt::Approve { action, .. } => extract_expr_to_let(action, allocator, reserved),
    }
}

fn extract_expr_to_let(
    expr: &mut Expr,
    allocator: &mut SpanAllocator,
    reserved: &mut BTreeSet<String>,
) -> Option<Stmt> {
    if !is_nontrivial_pure_expr(expr) {
        return None;
    }
    let binding_name = fresh_name(reserved, "extracted");
    reserved.insert(binding_name.clone());
    let binding_ident = Ident::new(binding_name, allocator.fresh_span());
    let replacement = Expr::Ident {
        name: Ident::new(binding_ident.name.clone(), allocator.fresh_span()),
        span: allocator.fresh_span(),
    };
    let value = expr.clone();
    *expr = replacement;
    Some(Stmt::Let {
        name: binding_ident,
        ty: None,
        value,
        span: allocator.fresh_span(),
    })
}

fn inline_in_block(
    block: &mut Block,
    resolved: &Resolved,
    binding_counts: &std::collections::HashMap<LocalId, usize>,
) -> bool {
    let mut index = 0;
    while index + 1 < block.stmts.len() {
        let inline_value = inline_candidate(
            &block.stmts[index],
            &block.stmts[index + 1],
            resolved,
            binding_counts,
        );
        if let Some(value) = inline_value {
            apply_inline_to_stmt(&mut block.stmts[index + 1], resolved, value);
            block.stmts.remove(index);
            return true;
        }
        match &mut block.stmts[index] {
            Stmt::If {
                then_block,
                else_block,
                ..
            } => {
                if inline_in_block(then_block, resolved, binding_counts) {
                    return true;
                }
                if let Some(block) = else_block {
                    if inline_in_block(block, resolved, binding_counts) {
                        return true;
                    }
                }
            }
            Stmt::For { body, .. } => {
                if inline_in_block(body, resolved, binding_counts) {
                    return true;
                }
            }
            _ => {}
        }
        index += 1;
    }
    if let Some(last) = block.stmts.last_mut() {
        match last {
            Stmt::If {
                then_block,
                else_block,
                ..
            } => {
                if inline_in_block(then_block, resolved, binding_counts) {
                    return true;
                }
                if let Some(block) = else_block {
                    if inline_in_block(block, resolved, binding_counts) {
                        return true;
                    }
                }
            }
            Stmt::For { body, .. } => {
                if inline_in_block(body, resolved, binding_counts) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn inline_candidate(
    current: &Stmt,
    next: &Stmt,
    resolved: &Resolved,
    binding_counts: &std::collections::HashMap<LocalId, usize>,
) -> Option<Expr> {
    let Stmt::Let { name, value, .. } = current else {
        return None;
    };
    if !is_pure_expr(value) {
        return None;
    }
    let Some(Binding::Local(local)) = resolved.bindings.get(&name.span) else {
        return None;
    };
    if binding_counts.get(local).copied().unwrap_or_default() != 1 {
        return None;
    }
    direct_local_use(next, resolved, *local).then(|| value.clone())
}

fn direct_local_use(stmt: &Stmt, resolved: &Resolved, local: LocalId) -> bool {
    match stmt {
        Stmt::Let { value, .. } => is_direct_local_expr(value, resolved, local),
        Stmt::Return { value, .. } => value
            .as_ref()
            .is_some_and(|expr| is_direct_local_expr(expr, resolved, local)),
        Stmt::Yield { value, .. } => is_direct_local_expr(value, resolved, local),
        Stmt::If { cond, .. } => is_direct_local_expr(cond, resolved, local),
        Stmt::For { iter, .. } => is_direct_local_expr(iter, resolved, local),
        Stmt::Approve { action, .. } => is_direct_local_expr(action, resolved, local),
        Stmt::Expr { expr, .. } => is_direct_local_expr(expr, resolved, local),
    }
}

fn is_direct_local_expr(expr: &Expr, resolved: &Resolved, local: LocalId) -> bool {
    match expr {
        Expr::Ident { name, .. } => matches!(
            resolved.bindings.get(&name.span),
            Some(Binding::Local(id)) if *id == local
        ),
        _ => false,
    }
}

fn apply_inline_to_stmt(stmt: &mut Stmt, resolved: &Resolved, replacement: Expr) {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Yield { value, .. } | Stmt::Expr { expr: value, .. } => {
            if matches!(value, Expr::Ident { .. }) {
                *value = replacement;
            }
        }
        Stmt::Return { value, .. } => {
            if let Some(expr) = value {
                if matches!(expr, Expr::Ident { .. }) {
                    *expr = replacement;
                }
            }
        }
        Stmt::If { cond, .. } => {
            if matches!(cond, Expr::Ident { .. }) {
                *cond = replacement;
            }
        }
        Stmt::For { iter, .. } => {
            if matches!(iter, Expr::Ident { .. }) {
                *iter = replacement;
            }
        }
        Stmt::Approve { action, .. } => {
            if matches!(action, Expr::Ident { .. }) {
                *action = replacement;
            }
        }
    }
    let _ = resolved;
}

fn swap_independent_lets_in_block(block: &mut Block, resolved: &Resolved) -> bool {
    let mut index = 0;
    while index + 1 < block.stmts.len() {
        if can_swap_independent_lets(&block.stmts[index], &block.stmts[index + 1], resolved) {
            block.stmts.swap(index, index + 1);
            return true;
        }
        match &mut block.stmts[index] {
            Stmt::If {
                then_block,
                else_block,
                ..
            } => {
                if swap_independent_lets_in_block(then_block, resolved) {
                    return true;
                }
                if let Some(block) = else_block {
                    if swap_independent_lets_in_block(block, resolved) {
                        return true;
                    }
                }
            }
            Stmt::For { body, .. } => {
                if swap_independent_lets_in_block(body, resolved) {
                    return true;
                }
            }
            _ => {}
        }
        index += 1;
    }
    if let Some(last) = block.stmts.last_mut() {
        match last {
            Stmt::If {
                then_block,
                else_block,
                ..
            } => {
                if swap_independent_lets_in_block(then_block, resolved) {
                    return true;
                }
                if let Some(block) = else_block {
                    if swap_independent_lets_in_block(block, resolved) {
                        return true;
                    }
                }
            }
            Stmt::For { body, .. } => {
                if swap_independent_lets_in_block(body, resolved) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn can_swap_independent_lets(left: &Stmt, right: &Stmt, resolved: &Resolved) -> bool {
    let Stmt::Let {
        name: left_name,
        value: left_value,
        ..
    } = left
    else {
        return false;
    };
    let Stmt::Let {
        name: right_name,
        value: right_value,
        ..
    } = right
    else {
        return false;
    };
    if !is_pure_expr(left_value) || !is_pure_expr(right_value) {
        return false;
    }
    let Some(Binding::Local(left_local)) = resolved.bindings.get(&left_name.span) else {
        return false;
    };
    let Some(Binding::Local(right_local)) = resolved.bindings.get(&right_name.span) else {
        return false;
    };
    if left_local == right_local {
        return false;
    }
    !expr_mentions_local(left_value, resolved, *right_local)
        && !expr_mentions_local(right_value, resolved, *left_local)
}

fn expr_mentions_local(expr: &Expr, resolved: &Resolved, local: LocalId) -> bool {
    match expr {
        Expr::Literal { .. } => false,
        Expr::Ident { name, .. } => matches!(
            resolved.bindings.get(&name.span),
            Some(Binding::Local(id)) if *id == local
        ),
        Expr::Call { callee, args, .. } => {
            expr_mentions_local(callee, resolved, local)
                || args
                    .iter()
                    .any(|arg| expr_mentions_local(arg, resolved, local))
        }
        Expr::FieldAccess { target, .. } => expr_mentions_local(target, resolved, local),
        Expr::Index { target, index, .. } => {
            expr_mentions_local(target, resolved, local)
                || expr_mentions_local(index, resolved, local)
        }
        Expr::BinOp { left, right, .. } => {
            expr_mentions_local(left, resolved, local)
                || expr_mentions_local(right, resolved, local)
        }
        Expr::UnOp { operand, .. } => expr_mentions_local(operand, resolved, local),
        Expr::List { items, .. } => items
            .iter()
            .any(|item| expr_mentions_local(item, resolved, local)),
        Expr::TryPropagate { inner, .. } => expr_mentions_local(inner, resolved, local),
        Expr::TryRetry { body, .. } => expr_mentions_local(body, resolved, local),
        Expr::Replay {
            trace,
            arms,
            else_body,
            ..
        } => {
            expr_mentions_local(trace, resolved, local)
                || arms
                    .iter()
                    .any(|arm| expr_mentions_local(&arm.body, resolved, local))
                || expr_mentions_local(else_body, resolved, local)
        }
    }
}

fn can_reorder_top_level_pair(
    left: &Decl,
    right: &Decl,
    file: &File,
    resolved: &Resolved,
    graph: &corvid_resolve::DepGraph,
) -> bool {
    if !is_reorderable_top_level_decl(left) || !is_reorderable_top_level_decl(right) {
        return false;
    }
    if matches!(left, Decl::Effect(_)) || matches!(right, Decl::Effect(_)) {
        return true;
    }
    let Some(left_id) = top_level_decl_id(left, resolved) else {
        return false;
    };
    let Some(right_id) = top_level_decl_id(right, resolved) else {
        return false;
    };
    let _ = file;
    !depends_transitively(graph, left_id, right_id)
        && !depends_transitively(graph, right_id, left_id)
}

fn is_reorderable_top_level_decl(decl: &Decl) -> bool {
    matches!(
        decl,
        Decl::Effect(_) | Decl::Tool(_) | Decl::Prompt(_) | Decl::Agent(_)
    )
}

fn top_level_decl_id(decl: &Decl, resolved: &Resolved) -> Option<corvid_resolve::DefId> {
    let name = match decl {
        Decl::Tool(tool) => &tool.name.name,
        Decl::Prompt(prompt) => &prompt.name.name,
        Decl::Agent(agent) => &agent.name.name,
        Decl::Effect(effect) => &effect.name.name,
        _ => return None,
    };
    resolved.symbols.lookup_def(name)
}

fn depends_transitively(
    graph: &corvid_resolve::DepGraph,
    start: corvid_resolve::DefId,
    target: corvid_resolve::DefId,
) -> bool {
    let mut pending = vec![start];
    let mut visited = BTreeSet::new();
    while let Some(current) = pending.pop() {
        if !visited.insert(current.0) {
            continue;
        }
        if current == target {
            return true;
        }
        if let Some(next) = graph.forward.get(&current) {
            pending.extend(next.iter().copied());
        }
    }
    false
}

fn swap_if_branches_in_block(block: &mut Block, allocator: &mut SpanAllocator) -> bool {
    for stmt in &mut block.stmts {
        match stmt {
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                if let Some(else_block) = else_block {
                    let original = std::mem::replace(
                        cond,
                        Expr::UnOp {
                            op: UnaryOp::Not,
                            operand: Box::new(cond.clone()),
                            span: allocator.fresh_span(),
                        },
                    );
                    *cond = negate_expr(original, allocator);
                    std::mem::swap(then_block, else_block);
                    return true;
                }
                if swap_if_branches_in_block(then_block, allocator) {
                    return true;
                }
            }
            Stmt::For { body, .. } => {
                if swap_if_branches_in_block(body, allocator) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn negate_expr(expr: Expr, allocator: &mut SpanAllocator) -> Expr {
    Expr::UnOp {
        op: UnaryOp::Not,
        operand: Box::new(expr),
        span: allocator.fresh_span(),
    }
}

fn fold_constants_in_block(block: &mut Block) -> bool {
    for stmt in &mut block.stmts {
        if fold_constants_in_stmt(stmt) {
            return true;
        }
        match stmt {
            Stmt::If {
                then_block,
                else_block,
                ..
            } => {
                if fold_constants_in_block(then_block) {
                    return true;
                }
                if let Some(block) = else_block {
                    if fold_constants_in_block(block) {
                        return true;
                    }
                }
            }
            Stmt::For { body, .. } => {
                if fold_constants_in_block(body) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn fold_constants_in_stmt(stmt: &mut Stmt) -> bool {
    match stmt {
        Stmt::Let { value, .. }
        | Stmt::Yield { value, .. }
        | Stmt::Expr { expr: value, .. }
        | Stmt::Approve { action: value, .. } => fold_constants_in_expr(value),
        Stmt::Return { value, .. } => value.as_mut().is_some_and(fold_constants_in_expr),
        Stmt::If { cond, .. } => fold_constants_in_expr(cond),
        Stmt::For { iter, .. } => fold_constants_in_expr(iter),
    }
}

fn fold_constants_in_expr(expr: &mut Expr) -> bool {
    if let Expr::BinOp {
        op,
        left,
        right,
        span,
    } = expr
    {
        if let (Expr::Literal { value: left, .. }, Expr::Literal { value: right, .. }) =
            (&**left, &**right)
        {
            if let Some(value) = fold_literal_binop(*op, left, right) {
                *expr = Expr::Literal { value, span: *span };
                return true;
            }
        }
    }
    match expr {
        Expr::Literal { .. } | Expr::Ident { .. } => false,
        Expr::Call { callee, args, .. } => {
            if fold_constants_in_expr(callee) {
                return true;
            }
            for arg in args {
                if fold_constants_in_expr(arg) {
                    return true;
                }
            }
            false
        }
        Expr::FieldAccess { target, .. } => fold_constants_in_expr(target),
        Expr::Index { target, index, .. } => {
            fold_constants_in_expr(target) || fold_constants_in_expr(index)
        }
        Expr::BinOp { left, right, .. } => {
            fold_constants_in_expr(left) || fold_constants_in_expr(right)
        }
        Expr::UnOp { operand, .. } => fold_constants_in_expr(operand),
        Expr::List { items, .. } => {
            for item in items {
                if fold_constants_in_expr(item) {
                    return true;
                }
            }
            false
        }
        Expr::TryPropagate { inner, .. } => fold_constants_in_expr(inner),
        Expr::TryRetry { body, .. } => fold_constants_in_expr(body),
        Expr::Replay {
            trace,
            arms,
            else_body,
            ..
        } => {
            if fold_constants_in_expr(trace) {
                return true;
            }
            for arm in arms {
                if fold_constants_in_expr(&mut arm.body) {
                    return true;
                }
            }
            fold_constants_in_expr(else_body)
        }
    }
}

fn fold_literal_binop(op: BinaryOp, left: &Literal, right: &Literal) -> Option<Literal> {
    match (op, left, right) {
        (BinaryOp::Add, Literal::Int(left), Literal::Int(right)) => {
            Some(Literal::Int(left + right))
        }
        (BinaryOp::Add, Literal::String(left), Literal::String(right)) => {
            Some(Literal::String(format!("{left}{right}")))
        }
        _ => None,
    }
}

fn collect_local_binding_counts(
    agent: &AgentDecl,
    resolved: &Resolved,
) -> std::collections::HashMap<LocalId, usize> {
    let mut counts = std::collections::HashMap::new();
    for param in &agent.params {
        if let Some(Binding::Local(id)) = resolved.bindings.get(&param.name.span) {
            *counts.entry(*id).or_insert(0) += 1;
        }
    }
    collect_binding_counts_from_block(&agent.body, resolved, &mut counts);
    counts
}

fn collect_binding_counts_from_block(
    block: &Block,
    resolved: &Resolved,
    counts: &mut std::collections::HashMap<LocalId, usize>,
) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let { name, .. } | Stmt::For { var: name, .. } => {
                if let Some(Binding::Local(id)) = resolved.bindings.get(&name.span) {
                    *counts.entry(*id).or_insert(0) += 1;
                }
            }
            _ => {}
        }
        match stmt {
            Stmt::If {
                then_block,
                else_block,
                ..
            } => {
                collect_binding_counts_from_block(then_block, resolved, counts);
                if let Some(block) = else_block {
                    collect_binding_counts_from_block(block, resolved, counts);
                }
            }
            Stmt::For { body, .. } => collect_binding_counts_from_block(body, resolved, counts),
            _ => {}
        }
    }
}

fn is_nontrivial_pure_expr(expr: &Expr) -> bool {
    !matches!(expr, Expr::Literal { .. } | Expr::Ident { .. }) && is_pure_expr(expr)
}

fn is_pure_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Literal { .. } | Expr::Ident { .. } => true,
        Expr::BinOp { left, right, .. } => is_pure_expr(left) && is_pure_expr(right),
        Expr::UnOp { operand, .. } => is_pure_expr(operand),
        Expr::List { items, .. } => items.iter().all(is_pure_expr),
        Expr::FieldAccess { target, .. } => is_pure_expr(target),
        Expr::Index { target, index, .. } => is_pure_expr(target) && is_pure_expr(index),
        Expr::Call { .. }
        | Expr::TryPropagate { .. }
        | Expr::TryRetry { .. }
        | Expr::Replay { .. } => false,
    }
}

fn collect_agent_local_names(agent: &AgentDecl) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = agent
        .params
        .iter()
        .map(|param| param.name.name.clone())
        .collect();
    collect_names_from_block(&agent.body, &mut names);
    names
}

fn collect_all_names(file: &File) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for decl in &file.decls {
        match decl {
            Decl::Effect(effect) => {
                names.insert(effect.name.name.clone());
                for dimension in &effect.dimensions {
                    names.insert(dimension.name.name.clone());
                }
            }
            Decl::Type(ty) => {
                names.insert(ty.name.name.clone());
                for field in &ty.fields {
                    names.insert(field.name.name.clone());
                }
            }
            Decl::Store(store) => {
                names.insert(store.name.name.clone());
                for field in &store.fields {
                    names.insert(field.name.name.clone());
                }
                for policy in &store.policies {
                    names.insert(policy.name.name.clone());
                }
            }
            Decl::Tool(tool) => {
                names.insert(tool.name.name.clone());
                for param in &tool.params {
                    names.insert(param.name.name.clone());
                }
            }
            Decl::Prompt(prompt) => {
                names.insert(prompt.name.name.clone());
                for param in &prompt.params {
                    names.insert(param.name.name.clone());
                }
            }
            Decl::Agent(agent) => {
                names.insert(agent.name.name.clone());
                for param in &agent.params {
                    names.insert(param.name.name.clone());
                }
                collect_names_from_block(&agent.body, &mut names);
            }
            Decl::Eval(eval) => {
                names.insert(eval.name.name.clone());
                collect_names_from_block(&eval.body, &mut names);
            }
            Decl::Test(test) => {
                names.insert(test.name.name.clone());
                collect_names_from_block(&test.body, &mut names);
            }
            Decl::Fixture(fixture) => {
                names.insert(fixture.name.name.clone());
                for param in &fixture.params {
                    names.insert(param.name.name.clone());
                }
                collect_names_from_block(&fixture.body, &mut names);
            }
            Decl::Mock(mock) => {
                names.insert(mock.target.name.clone());
                for param in &mock.params {
                    names.insert(param.name.name.clone());
                }
                collect_names_from_block(&mock.body, &mut names);
            }
            Decl::Import(import) => {
                if let Some(alias) = &import.alias {
                    names.insert(alias.name.clone());
                }
            }
            Decl::Extend(extend) => {
                names.insert(extend.type_name.name.clone());
                for method in &extend.methods {
                    names.insert(method.name().name.clone());
                }
            }
            Decl::Model(model) => {
                names.insert(model.name.name.clone());
                for field in &model.fields {
                    names.insert(field.name.name.clone());
                }
            }
            Decl::Server(server) => {
                names.insert(server.name.name.clone());
                for route in &server.routes {
                    for param in &route.path_params {
                        names.insert(param.name.name.clone());
                    }
                    collect_names_from_block(&route.body, &mut names);
                }
            }
            Decl::Schedule(schedule) => {
                names.insert(schedule.target.name.clone());
                for effect in &schedule.effect_row.effects {
                    names.insert(effect.name.name.clone());
                }
            }
        }
    }
    names
}

fn collect_names_from_block(block: &Block, names: &mut BTreeSet<String>) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let { name, .. } => {
                names.insert(name.name.clone());
            }
            Stmt::For { var, body, .. } => {
                names.insert(var.name.clone());
                collect_names_from_block(body, names);
            }
            Stmt::If {
                then_block,
                else_block,
                ..
            } => {
                collect_names_from_block(then_block, names);
                if let Some(block) = else_block {
                    collect_names_from_block(block, names);
                }
            }
            _ => {}
        }
    }
}

fn fresh_name(reserved: &BTreeSet<String>, base: &str) -> String {
    if !reserved.contains(base) {
        return base.to_string();
    }
    for suffix in 0..u32::MAX {
        let candidate = format!("{base}_{suffix}");
        if !reserved.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!("exhausted fresh-name search")
}

struct SpanAllocator {
    next: usize,
}

impl SpanAllocator {
    fn new(file: &File) -> Self {
        Self {
            next: file.span.end.saturating_add(1).max(1),
        }
    }

    fn fresh_span(&mut self) -> Span {
        let span = Span::new(self.next, self.next + 1);
        self.next += 2;
        span
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_and_reparses_simple_agent() {
        let source = r#"
agent main() -> Int:
    total = 1 + 2
    return total
"#;
        let file = parse_source(source).expect("parse");
        let rendered = render_file(&file);
        parse_source(&rendered).expect("round-trip parse");
    }

    #[test]
    fn renders_and_reparses_fixtures_and_mocks() {
        let source = r#"
tool lookup(id: String) -> Int

fixture sample_id() -> String:
    return "ord_42"

mock lookup(id: String) -> Int:
    return 42

test mocked_lookup:
    score = lookup(sample_id())
    assert_snapshot score
    assert score == 42

test lookup_trace from_trace "traces/lookup.jsonl":
    assert called lookup
"#;
        let file = parse_source(source).expect("parse");
        let rendered = render_file(&file);
        parse_source(&rendered).expect("round-trip parse");
    }
}
