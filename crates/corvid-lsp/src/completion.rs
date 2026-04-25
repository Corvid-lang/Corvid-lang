use crate::lsp_position_to_byte;
use corvid_ast::{Decl, Effect, File};
use corvid_syntax::{lex, parse_file};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionResponse, Documentation, MarkupContent,
    MarkupKind, Position,
};
use std::collections::BTreeMap;

const DECL_KEYWORDS: &[&str] = &[
    "agent", "tool", "prompt", "effect", "type", "model", "eval", "import", "extend", "public",
];

const BODY_KEYWORDS: &[&str] = &[
    "approve", "return", "yield", "if", "else", "for", "in", "break", "continue", "pass",
    "replay",
];

const AI_NATIVE_KEYWORDS: &[&str] = &[
    "uses",
    "dangerous",
    "cacheable",
    "calibrated",
    "cites",
    "strictly",
    "requires",
    "output_format",
    "route",
    "progressive",
    "rollout",
    "ensemble",
    "adversarial",
    "approve",
];

pub fn completion_at(source: &str, position: Position) -> CompletionResponse {
    let offset = lsp_position_to_byte(source, position);
    let context = CompletionContext::new(source, offset);
    let file = parse_lenient(source);
    let mut items = CompletionSet::default();

    if context.is_approval_context() {
        add_approval_labels(&mut items, &file);
        return items.into_response();
    }

    if context.is_uses_context() {
        add_effect_names(&mut items, &file);
        return items.into_response();
    }

    if context.is_model_context() {
        add_model_names(&mut items, &file);
        return items.into_response();
    }

    add_keywords(&mut items, DECL_KEYWORDS, "declaration keyword");
    add_keywords(&mut items, BODY_KEYWORDS, "statement keyword");
    add_keywords(&mut items, AI_NATIVE_KEYWORDS, "AI-native Corvid keyword");
    add_declarations(&mut items, &file);
    add_approval_labels(&mut items, &file);
    items.into_response()
}

fn parse_lenient(source: &str) -> File {
    match lex(source) {
        Ok(tokens) => {
            let (file, _errors) = parse_file(&tokens);
            file
        }
        Err(_) => File {
            decls: Vec::new(),
            span: corvid_ast::Span { start: 0, end: 0 },
        },
    }
}

#[derive(Debug)]
struct CompletionContext<'a> {
    before_cursor: &'a str,
    line_before_cursor: &'a str,
}

impl<'a> CompletionContext<'a> {
    fn new(source: &'a str, offset: usize) -> Self {
        let before_cursor = &source[..offset.min(source.len())];
        let line_start = before_cursor.rfind('\n').map(|idx| idx + 1).unwrap_or(0);
        Self {
            before_cursor,
            line_before_cursor: &before_cursor[line_start..],
        }
    }

    fn is_approval_context(&self) -> bool {
        self.line_before_cursor.trim_start().starts_with("approve ")
            || self
                .line_before_cursor
                .trim_start()
                .starts_with("assert approved ")
    }

    fn is_uses_context(&self) -> bool {
        let line = self.line_before_cursor;
        line.contains(" uses ")
            || line.trim_end().ends_with(" uses")
            || line.trim_end().ends_with(" uses,")
    }

    fn is_model_context(&self) -> bool {
        let line = self.line_before_cursor.trim_start();
        line.ends_with("->")
            || line.contains(" -> ")
            || line.ends_with("escalate_to")
            || line.contains("escalate_to ")
            || line.ends_with("propose:")
            || line.ends_with("challenge:")
            || line.ends_with("adjudicate:")
            || line.ends_with("ensemble [")
            || line.contains("ensemble [")
            || self.before_cursor.ends_with("model ")
    }
}

#[derive(Default)]
struct CompletionSet {
    items: BTreeMap<String, CompletionItem>,
}

impl CompletionSet {
    fn add(&mut self, item: CompletionItem) {
        self.items.entry(item.label.clone()).or_insert(item);
    }

    fn into_response(self) -> CompletionResponse {
        CompletionResponse::Array(self.items.into_values().collect())
    }
}

fn add_keywords(items: &mut CompletionSet, keywords: &[&str], detail: &str) {
    for keyword in keywords {
        items.add(CompletionItem {
            label: (*keyword).to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some(detail.to_string()),
            ..CompletionItem::default()
        });
    }
}

fn add_declarations(items: &mut CompletionSet, file: &File) {
    for decl in &file.decls {
        match decl {
            Decl::Agent(agent) => items.add(symbol_item(
                &agent.name.name,
                CompletionItemKind::FUNCTION,
                "agent",
                format!("agent {}(...) -> {}", agent.name.name, type_name(&agent.return_ty)),
            )),
            Decl::Tool(tool) => items.add(symbol_item(
                &tool.name.name,
                CompletionItemKind::FUNCTION,
                if matches!(tool.effect, Effect::Dangerous) {
                    "dangerous tool"
                } else {
                    "tool"
                },
                format!("tool {}(...) -> {}", tool.name.name, type_name(&tool.return_ty)),
            )),
            Decl::Prompt(prompt) => items.add(symbol_item(
                &prompt.name.name,
                CompletionItemKind::FUNCTION,
                "prompt",
                format!(
                    "prompt {}(...) -> {}",
                    prompt.name.name,
                    type_name(&prompt.return_ty)
                ),
            )),
            Decl::Type(ty) => items.add(symbol_item(
                &ty.name.name,
                CompletionItemKind::STRUCT,
                "type",
                format!("type {}", ty.name.name),
            )),
            Decl::Effect(effect) => items.add(symbol_item(
                &effect.name.name,
                CompletionItemKind::EVENT,
                "effect",
                format!("effect {}", effect.name.name),
            )),
            Decl::Model(model) => items.add(symbol_item(
                &model.name.name,
                CompletionItemKind::CLASS,
                "model",
                format!("model {}", model.name.name),
            )),
            Decl::Eval(eval) => items.add(symbol_item(
                &eval.name.name,
                CompletionItemKind::METHOD,
                "eval",
                format!("eval {}", eval.name.name),
            )),
            Decl::Test(test) => items.add(symbol_item(
                &test.name.name,
                CompletionItemKind::METHOD,
                "test",
                format!("test {}", test.name.name),
            )),
            Decl::Import(_) | Decl::Extend(_) => {}
        }
    }
}

fn add_effect_names(items: &mut CompletionSet, file: &File) {
    for decl in &file.decls {
        if let Decl::Effect(effect) = decl {
            items.add(symbol_item(
                &effect.name.name,
                CompletionItemKind::EVENT,
                "declared effect",
                effect_detail(effect),
            ));
        }
    }
}

fn add_model_names(items: &mut CompletionSet, file: &File) {
    for decl in &file.decls {
        if let Decl::Model(model) = decl {
            items.add(symbol_item(
                &model.name.name,
                CompletionItemKind::CLASS,
                "model catalog entry",
                format!("model {}", model.name.name),
            ));
        }
    }
}

fn add_approval_labels(items: &mut CompletionSet, file: &File) {
    for decl in &file.decls {
        if let Decl::Tool(tool) = decl {
            if matches!(tool.effect, Effect::Dangerous) {
                let label = approval_label(&tool.name.name);
                items.add(CompletionItem {
                    label: label.clone(),
                    kind: Some(CompletionItemKind::EVENT),
                    detail: Some(format!("approval label for `{}`", tool.name.name)),
                    documentation: Some(markdown(format!(
                        "`approve {label}(...)` authorizes the dangerous tool `{}`.",
                        tool.name.name
                    ))),
                    insert_text: Some(label),
                    ..CompletionItem::default()
                });
            }
        }
    }
}

fn symbol_item(label: &str, kind: CompletionItemKind, detail: &str, doc: String) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind: Some(kind),
        detail: Some(detail.to_string()),
        documentation: Some(markdown(format!("```corvid\n{doc}\n```"))),
        ..CompletionItem::default()
    }
}

fn markdown(value: String) -> Documentation {
    Documentation::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value,
    })
}

fn type_name(ty: &corvid_ast::TypeRef) -> String {
    match ty {
        corvid_ast::TypeRef::Named { name, .. } => name.name.clone(),
        corvid_ast::TypeRef::Qualified { alias, name, .. } => format!("{}.{}", alias.name, name.name),
        corvid_ast::TypeRef::Generic { name, args, .. } => format!(
            "{}<{}>",
            name.name,
            args.iter().map(type_name).collect::<Vec<_>>().join(", ")
        ),
        corvid_ast::TypeRef::Weak { inner, .. } => format!("Weak<{}>", type_name(inner)),
        corvid_ast::TypeRef::Function { params, ret, .. } => format!(
            "({}) -> {}",
            params.iter().map(type_name).collect::<Vec<_>>().join(", "),
            type_name(ret)
        ),
    }
}

fn effect_detail(effect: &corvid_ast::EffectDecl) -> String {
    if effect.dimensions.is_empty() {
        format!("effect {}", effect.name.name)
    } else {
        let dimensions = effect
            .dimensions
            .iter()
            .map(|dim| dim.name.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        format!("effect {}\n# dimensions: {dimensions}", effect.name.name)
    }
}

fn approval_label(tool_name: &str) -> String {
    tool_name
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(response: CompletionResponse) -> Vec<String> {
        match response {
            CompletionResponse::Array(items) => items.into_iter().map(|item| item.label).collect(),
            CompletionResponse::List(list) => list.items.into_iter().map(|item| item.label).collect(),
        }
    }

    #[test]
    fn completes_keywords_for_empty_document() {
        let got = labels(completion_at("", Position { line: 0, character: 0 }));
        assert!(got.contains(&"agent".to_string()), "{got:?}");
        assert!(got.contains(&"prompt".to_string()), "{got:?}");
        assert!(got.contains(&"approve".to_string()), "{got:?}");
    }

    #[test]
    fn completes_declared_symbols() {
        let src = "type Ticket:\n    id: String\n\ntool get_order(id: String) -> Ticket\n\nagent run(id: String) -> Ticket:\n    return get_order(id)\n\n";
        let got = labels(completion_at(src, Position { line: 7, character: 11 }));
        assert!(got.contains(&"Ticket".to_string()), "{got:?}");
        assert!(got.contains(&"get_order".to_string()), "{got:?}");
        assert!(got.contains(&"run".to_string()), "{got:?}");
    }

    #[test]
    fn completes_approval_labels_for_dangerous_tools() {
        let src = "tool issue_refund(id: String) -> String dangerous\n\nagent run(id: String) -> String:\n    approve \n";
        let got = labels(completion_at(src, Position { line: 3, character: 12 }));
        assert_eq!(got, vec!["IssueRefund".to_string()]);
    }

    #[test]
    fn completes_effects_after_uses() {
        let src = "effect transfer_money:\n    cost: $0.50\n\ntool issue_refund(id: String) -> String uses \n";
        let got = labels(completion_at(src, Position { line: 3, character: 52 }));
        assert_eq!(got, vec!["transfer_money".to_string()]);
    }

    #[test]
    fn completes_models_in_prompt_routing_context() {
        let src = "model cheap:\n    capability: basic\n\nmodel strong:\n    capability: expert\n\nprompt answer(x: String) -> String:\n    route:\n        _ -> \n";
        let got = labels(completion_at(src, Position { line: 8, character: 13 }));
        assert_eq!(got, vec!["cheap".to_string(), "strong".to_string()]);
    }
}
