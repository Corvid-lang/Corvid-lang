use anyhow::{anyhow, bail, Result};
use corvid_ast::{
    AgentDecl, Backoff, BackpressurePolicy, BinaryOp, Block, Decl, DimensionValue, Effect,
    EffectConstraint, EvalAssert, Expr, ExtendMethod, ExtendMethodKind, File, ImportSource, Ident,
    Literal, Param, PromptDecl, Span, Stmt, ToolDecl, TypeRef, UnaryOp, Visibility,
};
use corvid_resolve::{resolve, Binding, LocalId, Resolved};
use corvid_syntax::{lex, parse_file};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewriteRule {
    AlphaConversion,
    LetExtract,
    LetInline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LawRef {
    pub name: &'static str,
    pub rationale: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewriteResult {
    pub source: String,
    pub changed: bool,
    pub law: LawRef,
}

pub fn rewrite_rules() -> &'static [RewriteRule] {
    &[
        RewriteRule::AlphaConversion,
        RewriteRule::LetExtract,
        RewriteRule::LetInline,
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
    }
}

pub fn apply_rewrite(source: &str, rule: RewriteRule) -> Result<RewriteResult> {
    let mut file = parse_source(source)?;
    let changed = match rule {
        RewriteRule::AlphaConversion => alpha_convert(&mut file)?,
        RewriteRule::LetExtract => let_extract(&mut file)?,
        RewriteRule::LetInline => let_inline(&mut file)?,
    };
    Ok(RewriteResult {
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

pub fn render_file(file: &File) -> String {
    let mut out = String::new();
    for (index, decl) in file.decls.iter().enumerate() {
        if index > 0 {
            out.push_str("\n\n");
        }
        render_decl(decl, 0, &mut out);
    }
    out.push('\n');
    out
}

fn alpha_convert(file: &mut File) -> Result<bool> {
    let resolved = resolve(file);
    if !resolved.errors.is_empty() {
        bail!("resolve failed before alpha-conversion: {:?}", resolved.errors);
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
            Stmt::For { body, .. } => collect_rename_candidate_from_block(body, resolved, candidate),
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
            Stmt::For { var, iter, body, .. } => {
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
        Expr::Index { target: inner, index, .. } => {
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
        Stmt::Let { value, .. }
        | Stmt::Yield { value, .. }
        | Stmt::Expr { expr: value, .. } => {
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
        Expr::Call { .. } | Expr::TryPropagate { .. } | Expr::TryRetry { .. } => false,
    }
}

fn collect_agent_local_names(agent: &AgentDecl) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = agent.params.iter().map(|param| param.name.name.clone()).collect();
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

fn render_decl(decl: &Decl, indent: usize, out: &mut String) {
    match decl {
        Decl::Import(import) => {
            push_indent(indent, out);
            out.push_str("import ");
            out.push_str(match import.source {
                ImportSource::Python => "python ",
            });
            out.push_str(&render_string_literal(&import.module));
            if let Some(alias) = &import.alias {
                out.push_str(" as ");
                out.push_str(&alias.name);
            }
        }
        Decl::Type(ty) => {
            push_indent(indent, out);
            out.push_str("type ");
            out.push_str(&ty.name.name);
            out.push_str(":\n");
            for (index, field) in ty.fields.iter().enumerate() {
                if index > 0 {
                    out.push('\n');
                }
                push_indent(indent + 1, out);
                out.push_str(&field.name.name);
                out.push_str(": ");
                out.push_str(&render_type_ref(&field.ty));
            }
        }
        Decl::Tool(tool) => render_tool(tool, indent, out),
        Decl::Prompt(prompt) => render_prompt(prompt, indent, out),
        Decl::Agent(agent) => render_agent(agent, indent, out),
        Decl::Eval(eval) => {
            push_indent(indent, out);
            out.push_str("eval ");
            out.push_str(&eval.name.name);
            out.push_str(":\n");
            render_block(&eval.body, indent + 1, out);
            if !eval.assertions.is_empty() {
                out.push('\n');
            }
            for (index, assertion) in eval.assertions.iter().enumerate() {
                if index > 0 {
                    out.push('\n');
                }
                push_indent(indent + 1, out);
                out.push_str(&render_eval_assert(assertion));
            }
        }
        Decl::Extend(extend) => {
            push_indent(indent, out);
            out.push_str("extend ");
            out.push_str(&extend.type_name.name);
            out.push_str(":\n");
            for (index, method) in extend.methods.iter().enumerate() {
                if index > 0 {
                    out.push('\n');
                }
                render_extend_method(method, indent + 1, out);
            }
        }
        Decl::Effect(effect) => {
            push_indent(indent, out);
            out.push_str("effect ");
            out.push_str(&effect.name.name);
            out.push_str(":\n");
            for (index, dimension) in effect.dimensions.iter().enumerate() {
                if index > 0 {
                    out.push('\n');
                }
                push_indent(indent + 1, out);
                out.push_str(&dimension.name.name);
                out.push_str(": ");
                out.push_str(&render_dimension_value(&dimension.value));
            }
        }
    }
}

fn render_tool(tool: &ToolDecl, indent: usize, out: &mut String) {
    push_indent(indent, out);
    out.push_str("tool ");
    out.push_str(&tool.name.name);
    out.push_str(&render_params(&tool.params));
    out.push_str(" -> ");
    out.push_str(&render_type_ref(&tool.return_ty));
    if matches!(tool.effect, Effect::Dangerous) {
        out.push_str(" dangerous");
    }
    if !tool.effect_row.effects.is_empty() {
        out.push_str(" uses ");
        out.push_str(&render_effect_row_names(&tool.effect_row.effects));
    }
}

fn render_prompt(prompt: &PromptDecl, indent: usize, out: &mut String) {
    push_indent(indent, out);
    out.push_str("prompt ");
    out.push_str(&prompt.name.name);
    out.push_str(&render_params(&prompt.params));
    out.push_str(" -> ");
    out.push_str(&render_type_ref(&prompt.return_ty));
    if !prompt.effect_row.effects.is_empty() {
        out.push_str(" uses ");
        out.push_str(&render_effect_row_names(&prompt.effect_row.effects));
    }
    out.push_str(":\n");
    if let Some(min_confidence) = prompt.stream.min_confidence {
        push_indent(indent + 1, out);
        out.push_str("with min_confidence ");
        out.push_str(&render_float(min_confidence));
        out.push('\n');
    }
    if let Some(max_tokens) = prompt.stream.max_tokens {
        push_indent(indent + 1, out);
        out.push_str("with max_tokens ");
        out.push_str(&max_tokens.to_string());
        out.push('\n');
    }
    if let Some(policy) = &prompt.stream.backpressure {
        push_indent(indent + 1, out);
        out.push_str("with backpressure ");
        out.push_str(&render_backpressure(policy));
        out.push('\n');
    }
    push_indent(indent + 1, out);
    out.push_str(&render_string_literal(&prompt.template));
}

fn render_agent(agent: &AgentDecl, indent: usize, out: &mut String) {
    for constraint in &agent.constraints {
        push_indent(indent, out);
        out.push_str(&render_constraint(constraint));
        out.push('\n');
    }
    push_indent(indent, out);
    out.push_str("agent ");
    out.push_str(&agent.name.name);
    out.push_str(&render_params(&agent.params));
    out.push_str(" -> ");
    out.push_str(&render_type_ref(&agent.return_ty));
    if !agent.effect_row.effects.is_empty() {
        out.push_str(" uses ");
        out.push_str(&render_effect_row_names(&agent.effect_row.effects));
    }
    out.push_str(":\n");
    render_block(&agent.body, indent + 1, out);
}

fn render_extend_method(method: &ExtendMethod, indent: usize, out: &mut String) {
    push_indent(indent, out);
    match method.visibility {
        Visibility::Private => {}
        Visibility::Public => out.push_str("public "),
        Visibility::PublicPackage => out.push_str("public(package) "),
    }
    match &method.kind {
        ExtendMethodKind::Tool(tool) => render_tool(tool, 0, out),
        ExtendMethodKind::Prompt(prompt) => render_prompt(prompt, 0, out),
        ExtendMethodKind::Agent(agent) => render_agent(agent, 0, out),
    }
}

fn render_block(block: &Block, indent: usize, out: &mut String) {
    for (index, stmt) in block.stmts.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        render_stmt(stmt, indent, out);
    }
}

fn render_stmt(stmt: &Stmt, indent: usize, out: &mut String) {
    match stmt {
        Stmt::Let { name, ty, value, .. } => {
            push_indent(indent, out);
            out.push_str(&name.name);
            if let Some(ty) = ty {
                out.push_str(": ");
                out.push_str(&render_type_ref(ty));
            }
            out.push_str(" = ");
            out.push_str(&render_expr(value));
        }
        Stmt::Return { value, .. } => {
            push_indent(indent, out);
            out.push_str("return");
            if let Some(value) = value {
                out.push(' ');
                out.push_str(&render_expr(value));
            }
        }
        Stmt::Yield { value, .. } => {
            push_indent(indent, out);
            out.push_str("yield ");
            out.push_str(&render_expr(value));
        }
        Stmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            push_indent(indent, out);
            out.push_str("if ");
            out.push_str(&render_expr(cond));
            out.push_str(":\n");
            render_block(then_block, indent + 1, out);
            if let Some(block) = else_block {
                out.push('\n');
                push_indent(indent, out);
                out.push_str("else:\n");
                render_block(block, indent + 1, out);
            }
        }
        Stmt::For { var, iter, body, .. } => {
            push_indent(indent, out);
            out.push_str("for ");
            out.push_str(&var.name);
            out.push_str(" in ");
            out.push_str(&render_expr(iter));
            out.push_str(":\n");
            render_block(body, indent + 1, out);
        }
        Stmt::Approve { action, .. } => {
            push_indent(indent, out);
            out.push_str("approve ");
            out.push_str(&render_expr(action));
        }
        Stmt::Expr { expr, .. } => {
            push_indent(indent, out);
            out.push_str(&render_expr(expr));
        }
    }
}

fn render_expr(expr: &Expr) -> String {
    match expr {
        Expr::Literal { value, .. } => render_literal(value),
        Expr::Ident { name, .. } => name.name.clone(),
        Expr::Call { callee, args, .. } => format!(
            "{}({})",
            render_expr(callee),
            args.iter().map(render_expr).collect::<Vec<_>>().join(", ")
        ),
        Expr::FieldAccess { target, field, .. } => format!("{}.{}", render_expr(target), field.name),
        Expr::Index { target, index, .. } => {
            format!("{}[{}]", render_expr(target), render_expr(index))
        }
        Expr::BinOp { op, left, right, .. } => format!(
            "({} {} {})",
            render_expr(left),
            render_binary_op(*op),
            render_expr(right)
        ),
        Expr::UnOp { op, operand, .. } => format!("({}{})", render_unary_op(*op), render_expr(operand)),
        Expr::List { items, .. } => format!(
            "[{}]",
            items.iter().map(render_expr).collect::<Vec<_>>().join(", ")
        ),
        Expr::TryPropagate { inner, .. } => format!("{}?", render_expr(inner)),
        Expr::TryRetry {
            body,
            attempts,
            backoff,
            ..
        } => format!(
            "try {} on error retry {} times backoff {}",
            render_expr(body),
            attempts,
            render_backoff(*backoff)
        ),
    }
}

fn render_eval_assert(assertion: &EvalAssert) -> String {
    match assertion {
        EvalAssert::Value {
            expr,
            confidence,
            runs,
            ..
        } => {
            let mut text = format!("assert {}", render_expr(expr));
            if let (Some(confidence), Some(runs)) = (confidence, runs) {
                text.push_str(&format!(
                    " with confidence {} over {} runs",
                    render_float(*confidence),
                    runs
                ));
            }
            text
        }
        EvalAssert::Called { tool, .. } => format!("assert called {}", tool.name),
        EvalAssert::Approved { label, .. } => format!("assert approved {}", label.name),
        EvalAssert::Cost { op, bound, .. } => {
            format!("assert cost {} ${}", render_binary_op(*op), render_float(*bound))
        }
        EvalAssert::Ordering { before, after, .. } => {
            format!("assert called {} before {}", before.name, after.name)
        }
    }
}

fn render_params(params: &[Param]) -> String {
    let rendered: Vec<String> = params
        .iter()
        .map(|param| format!("{}: {}", param.name.name, render_type_ref(&param.ty)))
        .collect();
    format!("({})", rendered.join(", "))
}

fn render_type_ref(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Named { name, .. } => name.name.clone(),
        TypeRef::Generic { name, args, .. } => format!(
            "{}<{}>",
            name.name,
            args.iter().map(render_type_ref).collect::<Vec<_>>().join(", ")
        ),
        TypeRef::Weak { inner, effects, .. } => {
            let inner = render_type_ref(inner);
            if let Some(effects) = effects {
                let names: Vec<&str> = effects
                    .effects()
                    .into_iter()
                    .map(|effect| match effect {
                        corvid_ast::WeakEffect::ToolCall => "tool_call",
                        corvid_ast::WeakEffect::Llm => "llm",
                        corvid_ast::WeakEffect::Approve => "approve",
                    })
                    .collect();
                format!("Weak<{}, {{{}}}>", inner, names.join(", "))
            } else {
                format!("Weak<{}>", inner)
            }
        }
        TypeRef::Function { params, ret, .. } => format!(
            "({}) -> {}",
            params.iter().map(render_type_ref).collect::<Vec<_>>().join(", "),
            render_type_ref(ret)
        ),
    }
}

fn render_literal(literal: &Literal) -> String {
    match literal {
        Literal::Int(value) => value.to_string(),
        Literal::Float(value) => render_float(*value),
        Literal::String(value) => render_string_literal(value),
        Literal::Bool(value) => value.to_string(),
        Literal::Nothing => "nothing".into(),
    }
}

fn render_constraint(constraint: &EffectConstraint) -> String {
    match &constraint.value {
        Some(value) => format!("@{}({})", constraint.dimension.name, render_dimension_value(value)),
        None => format!("@{}", constraint.dimension.name),
    }
}

fn render_dimension_value(value: &DimensionValue) -> String {
    match value {
        DimensionValue::Bool(value) => value.to_string(),
        DimensionValue::Name(name) => name.clone(),
        DimensionValue::Cost(value) => format!("${}", render_float(*value)),
        DimensionValue::Number(value) => render_float(*value),
        DimensionValue::Streaming { backpressure } => {
            format!("streaming({})", render_backpressure(backpressure))
        }
        DimensionValue::ConfidenceGated { threshold, above, .. } => {
            format!("{above}_if_confident({})", render_float(*threshold))
        }
    }
}

fn render_effect_row_names(effects: &[corvid_ast::EffectRef]) -> String {
    effects
        .iter()
        .map(|effect| effect.name.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_backpressure(policy: &BackpressurePolicy) -> String {
    match policy {
        BackpressurePolicy::Bounded(size) => format!("bounded({size})"),
        BackpressurePolicy::Unbounded => "unbounded".into(),
    }
}

fn render_backoff(backoff: Backoff) -> String {
    match backoff {
        Backoff::Linear(value) => format!("linear {value}"),
        Backoff::Exponential(value) => format!("exponential {value}"),
    }
}

fn render_binary_op(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "%",
        BinaryOp::Eq => "==",
        BinaryOp::NotEq => "!=",
        BinaryOp::Lt => "<",
        BinaryOp::LtEq => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::GtEq => ">=",
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
    }
}

fn render_unary_op(op: UnaryOp) -> &'static str {
    match op {
        UnaryOp::Neg => "-",
        UnaryOp::Not => "not ",
    }
}

fn render_float(value: f64) -> String {
    let text = format!("{value}");
    if text.contains('.') {
        text
    } else {
        format!("{text}.0")
    }
}

fn render_string_literal(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
}

fn push_indent(indent: usize, out: &mut String) {
    for _ in 0..indent {
        out.push_str("    ");
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
}
