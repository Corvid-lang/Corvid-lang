use crate::{byte_span_to_lsp_range, lsp_position_to_byte};
use corvid_ast::{Decl, Effect, EffectRow, File, Param, Span, TypeRef};
use corvid_resolve::resolve;
use corvid_syntax::{lex, parse_file};
use corvid_types::{typecheck, Checked};
use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position};

pub fn hover_at(source: &str, position: Position) -> Option<Hover> {
    let offset = lsp_position_to_byte(source, position);
    let (file, checked) = checked_file(source)?;
    declaration_hover(source, &file, offset)
        .or_else(|| expression_type_hover(source, &checked, offset))
}

fn checked_file(source: &str) -> Option<(File, Checked)> {
    let tokens = lex(source).ok()?;
    let (file, parse_errors) = parse_file(&tokens);
    if !parse_errors.is_empty() {
        return None;
    }
    let resolved = resolve(&file);
    if !resolved.errors.is_empty() {
        return None;
    }
    let checked = typecheck(&file, &resolved);
    if !checked.errors.is_empty() {
        return None;
    }
    Some((file, checked))
}

fn declaration_hover(source: &str, file: &File, offset: usize) -> Option<Hover> {
    for decl in &file.decls {
        match decl {
            Decl::Agent(agent) if contains(agent.name.span, offset) => {
                return Some(markdown_hover(
                    source,
                    agent.name.span,
                    format!(
                        "```corvid\nagent {}({}) -> {}\n```\n{}{}",
                        agent.name.name,
                        format_params(&agent.params),
                        type_ref_name(&agent.return_ty),
                        effect_row_summary(&agent.effect_row),
                        constraint_summary(agent.constraints.len()),
                    ),
                ));
            }
            Decl::Tool(tool) if contains(tool.name.span, offset) => {
                return Some(markdown_hover(
                    source,
                    tool.name.span,
                    format!(
                        "```corvid\ntool {}({}) -> {}\n```\n{}{}",
                        tool.name.name,
                        format_params(&tool.params),
                        type_ref_name(&tool.return_ty),
                        legacy_effect_summary(tool.effect),
                        effect_row_summary(&tool.effect_row),
                    ),
                ));
            }
            Decl::Prompt(prompt) if contains(prompt.name.span, offset) => {
                return Some(markdown_hover(
                    source,
                    prompt.name.span,
                    format!(
                        "```corvid\nprompt {}({}) -> {}\n```\n{}{}{}",
                        prompt.name.name,
                        format_params(&prompt.params),
                        type_ref_name(&prompt.return_ty),
                        effect_row_summary(&prompt.effect_row),
                        prompt_route_summary(prompt),
                        prompt_runtime_summary(prompt),
                    ),
                ));
            }
            Decl::Type(ty) if contains(ty.name.span, offset) => {
                return Some(markdown_hover(
                    source,
                    ty.name.span,
                    format!("```corvid\ntype {}\n```", ty.name.name),
                ));
            }
            Decl::Effect(effect) if contains(effect.name.span, offset) => {
                let dimensions = effect
                    .dimensions
                    .iter()
                    .map(|dim| dim.name.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Some(markdown_hover(
                    source,
                    effect.name.span,
                    format!(
                        "```corvid\neffect {}\n```\ndimensions: {}",
                        effect.name.name,
                        if dimensions.is_empty() { "none" } else { &dimensions }
                    ),
                ));
            }
            _ => {}
        }
    }
    None
}

fn expression_type_hover(source: &str, checked: &Checked, offset: usize) -> Option<Hover> {
    checked
        .types
        .iter()
        .filter(|(span, _)| contains(**span, offset))
        .min_by_key(|(span, _)| span.end.saturating_sub(span.start))
        .map(|(span, ty)| {
            markdown_hover(
                source,
                *span,
                format!("```corvid\n{}\n```", ty.display_name()),
            )
        })
}

fn markdown_hover(source: &str, span: Span, value: String) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: Some(byte_span_to_lsp_range(source, span.start, span.end)),
    }
}

fn contains(span: Span, offset: usize) -> bool {
    span.start <= offset && offset <= span.end
}

fn format_params(params: &[Param]) -> String {
    params
        .iter()
        .map(|param| format!("{}: {}", param.name.name, type_ref_name(&param.ty)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn type_ref_name(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Named { name, .. } => name.name.clone(),
        TypeRef::Qualified { alias, name, .. } => format!("{}.{}", alias.name, name.name),
        TypeRef::Generic { name, args, .. } => format!(
            "{}<{}>",
            name.name,
            args.iter().map(type_ref_name).collect::<Vec<_>>().join(", ")
        ),
        TypeRef::Weak { inner, .. } => format!("Weak<{}>", type_ref_name(inner)),
        TypeRef::Function { params, ret, .. } => format!(
            "({}) -> {}",
            params.iter().map(type_ref_name).collect::<Vec<_>>().join(", "),
            type_ref_name(ret)
        ),
    }
}

fn effect_row_summary(row: &EffectRow) -> String {
    if row.effects.is_empty() {
        "effects: inferred or none\n".to_string()
    } else {
        format!(
            "effects: {}\n",
            row.effects
                .iter()
                .map(|effect| effect.name.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

fn legacy_effect_summary(effect: Effect) -> String {
    match effect {
        Effect::Safe => "legacy safety: safe\n".to_string(),
        Effect::Dangerous => "legacy safety: dangerous; requires approval\n".to_string(),
    }
}

fn constraint_summary(count: usize) -> String {
    if count == 0 {
        String::new()
    } else {
        format!("constraints: {count}\n")
    }
}

fn prompt_route_summary(prompt: &corvid_ast::PromptDecl) -> String {
    let mut lines = Vec::new();
    if prompt.route.is_some() {
        lines.push("route: pattern-dispatched");
    }
    if prompt.progressive.is_some() {
        lines.push("route: progressive confidence escalation");
    }
    if prompt.rollout.is_some() {
        lines.push("route: rollout");
    }
    if prompt.ensemble.is_some() {
        lines.push("route: ensemble vote");
    }
    if prompt.adversarial.is_some() {
        lines.push("route: adversarial validation");
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

fn prompt_runtime_summary(prompt: &corvid_ast::PromptDecl) -> String {
    let mut lines = Vec::<String>::new();
    if prompt.calibrated {
        lines.push("calibration: enabled".to_string());
    }
    if prompt.cacheable {
        lines.push("cache: enabled".to_string());
    }
    if let Some(cites) = &prompt.cites_strictly {
        lines.push(format!("strict citations: {cites}"));
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hover_text(hover: Hover) -> String {
        match hover.contents {
            HoverContents::Markup(markup) => markup.value,
            other => panic!("unexpected hover contents: {other:?}"),
        }
    }

    #[test]
    fn hovers_agent_signature() {
        let src = "agent answer(x: Int) -> Int:\n    return x + 1\n";
        let hover = hover_at(src, Position { line: 0, character: 7 }).unwrap();
        let text = hover_text(hover);
        assert!(text.contains("agent answer(x: Int) -> Int"), "{text}");
    }

    #[test]
    fn hovers_expression_type() {
        let src = "agent answer(x: Int) -> Int:\n    return x + 1\n";
        let hover = hover_at(src, Position { line: 1, character: 13 }).unwrap();
        let text = hover_text(hover);
        assert!(text.contains("Int"), "{text}");
    }

    #[test]
    fn hovers_prompt_ai_native_metadata() {
        let src = "prompt classify(x: Int) -> Int:\n    cacheable: true\n    calibrated\n    \"score\"\n";
        let hover = hover_at(src, Position { line: 0, character: 8 }).unwrap();
        let text = hover_text(hover);
        assert!(text.contains("prompt classify(x: Int) -> Int"), "{text}");
        assert!(text.contains("calibration: enabled"), "{text}");
        assert!(text.contains("cache: enabled"), "{text}");
    }
}
