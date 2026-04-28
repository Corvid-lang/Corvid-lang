use crate::{byte_span_to_lsp_range, lsp_position_to_byte};
use corvid_ast::{
    AgentDecl, Decl, EffectDecl, EvalDecl, File, FixtureDecl, ModelDecl, MockDecl, PromptDecl,
    Span, TestDecl, ToolDecl, TypeDecl,
};
use corvid_resolve::{resolve, Binding, DeclKind, DefId, LocalId, Resolved};
use corvid_syntax::{lex, parse_file};
use lsp_types::{Location, Position, Range, SymbolInformation, SymbolKind, Url};
use std::collections::BTreeSet;

pub fn definition_range_at(source: &str, position: Position) -> Option<Range> {
    let offset = lsp_position_to_byte(source, position);
    let index = NavigationIndex::build(source)?;
    let target = index.target_at(offset)?;
    index
        .definition_span(target)
        .map(|span| byte_span_to_lsp_range(source, span.start, span.end))
}

pub fn references_at(source: &str, position: Position) -> Vec<Range> {
    let offset = lsp_position_to_byte(source, position);
    let Some(index) = NavigationIndex::build(source) else {
        return Vec::new();
    };
    let Some(target) = index.target_at(offset) else {
        return Vec::new();
    };
    index
        .reference_spans(target)
        .into_iter()
        .map(|span| byte_span_to_lsp_range(source, span.start, span.end))
        .collect()
}

pub fn rename_ranges_at(source: &str, position: Position, new_name: &str) -> Option<Vec<Range>> {
    if !is_identifier(new_name) {
        return None;
    }
    let offset = lsp_position_to_byte(source, position);
    let index = NavigationIndex::build(source)?;
    let target = index.target_at(offset)?;
    if matches!(target, NavTarget::Builtin) {
        return None;
    }
    Some(
        index
            .reference_spans(target)
            .into_iter()
            .map(|span| byte_span_to_lsp_range(source, span.start, span.end))
            .collect(),
    )
}

pub fn workspace_symbols_for_document(
    uri: Url,
    source: &str,
    query: &str,
) -> Vec<SymbolInformation> {
    let Some(index) = NavigationIndex::build(source) else {
        return Vec::new();
    };
    let query = query.to_ascii_lowercase();
    top_level_definitions(&index.file, &index.resolved)
        .into_iter()
        .filter(|definition| {
            query.is_empty() || definition.name.to_ascii_lowercase().contains(query.as_str())
        })
        .map(|definition| {
            #[allow(deprecated)]
            SymbolInformation {
                name: definition.name,
                kind: definition.kind,
                tags: None,
                deprecated: None,
                location: Location {
                    uri: uri.clone(),
                    range: byte_span_to_lsp_range(
                        source,
                        definition.span.start,
                        definition.span.end,
                    ),
                },
                container_name: None,
            }
        })
        .collect()
}

struct NavigationIndex {
    file: File,
    resolved: Resolved,
}

impl NavigationIndex {
    fn build(source: &str) -> Option<Self> {
        let tokens = lex(source).ok()?;
        let (file, parse_errors) = parse_file(&tokens);
        if !parse_errors.is_empty() {
            return None;
        }
        let resolved = resolve(&file);
        if !resolved.errors.is_empty() {
            return None;
        }
        Some(Self { file, resolved })
    }

    fn target_at(&self, offset: usize) -> Option<NavTarget> {
        self.binding_at(offset).or_else(|| self.decl_at(offset))
    }

    fn binding_at(&self, offset: usize) -> Option<NavTarget> {
        self.resolved
            .bindings
            .iter()
            .filter(|(span, _)| contains(**span, offset))
            .min_by_key(|(span, _)| span.end.saturating_sub(span.start))
            .map(|(_, binding)| match binding {
                Binding::Decl(id) => NavTarget::Decl(*id),
                Binding::Local(id) => NavTarget::Local(*id),
                Binding::BuiltIn(_) => NavTarget::Builtin,
            })
    }

    fn decl_at(&self, offset: usize) -> Option<NavTarget> {
        top_level_definitions(&self.file, &self.resolved)
            .into_iter()
            .filter(|definition| contains(definition.span, offset))
            .min_by_key(|definition| definition.span.end.saturating_sub(definition.span.start))
            .map(|definition| NavTarget::Decl(definition.def_id))
    }

    fn definition_span(&self, target: NavTarget) -> Option<Span> {
        match target {
            NavTarget::Decl(id) => top_level_definitions(&self.file, &self.resolved)
                .into_iter()
                .find(|definition| definition.def_id == id)
                .map(|definition| definition.span),
            NavTarget::Local(id) => self
                .resolved
                .bindings
                .iter()
                .filter_map(|(span, binding)| match binding {
                    Binding::Local(local_id) if *local_id == id => Some(*span),
                    _ => None,
                })
                .min_by_key(|span| span.start),
            NavTarget::Builtin => None,
        }
    }

    fn reference_spans(&self, target: NavTarget) -> Vec<Span> {
        match target {
            NavTarget::Decl(id) => {
                let mut spans = BTreeSet::<(usize, usize)>::new();
                if let Some(definition) = self.definition_span(target) {
                    spans.insert((definition.start, definition.end));
                }
                for (span, binding) in &self.resolved.bindings {
                    if matches!(binding, Binding::Decl(binding_id) if *binding_id == id) {
                        spans.insert((span.start, span.end));
                    }
                }
                spans
                    .into_iter()
                    .map(|(start, end)| Span { start, end })
                    .collect()
            }
            NavTarget::Local(id) => self
                .resolved
                .bindings
                .iter()
                .filter_map(|(span, binding)| match binding {
                    Binding::Local(local_id) if *local_id == id => Some(*span),
                    _ => None,
                })
                .collect(),
            NavTarget::Builtin => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NavTarget {
    Decl(DefId),
    Local(LocalId),
    Builtin,
}

struct Definition {
    def_id: DefId,
    name: String,
    kind: SymbolKind,
    span: Span,
}

fn top_level_definitions(file: &File, resolved: &Resolved) -> Vec<Definition> {
    let mut out = Vec::new();
    for decl in &file.decls {
        match decl {
            Decl::Type(TypeDecl { name, .. }) => push_definition(
                &mut out,
                resolved,
                &name.name,
                DeclKind::Type,
                name.span,
                SymbolKind::STRUCT,
            ),
            Decl::Store(store) => push_definition(
                &mut out,
                resolved,
                &store.name.name,
                DeclKind::Store,
                store.name.span,
                SymbolKind::STRUCT,
            ),
            Decl::Tool(ToolDecl { name, .. }) => push_definition(
                &mut out,
                resolved,
                &name.name,
                DeclKind::Tool,
                name.span,
                SymbolKind::FUNCTION,
            ),
            Decl::Prompt(PromptDecl { name, .. }) => push_definition(
                &mut out,
                resolved,
                &name.name,
                DeclKind::Prompt,
                name.span,
                SymbolKind::FUNCTION,
            ),
            Decl::Agent(AgentDecl { name, .. }) => push_definition(
                &mut out,
                resolved,
                &name.name,
                DeclKind::Agent,
                name.span,
                SymbolKind::FUNCTION,
            ),
            Decl::Eval(EvalDecl { name, .. }) => push_definition(
                &mut out,
                resolved,
                &name.name,
                DeclKind::Eval,
                name.span,
                SymbolKind::METHOD,
            ),
            Decl::Test(TestDecl { name, .. }) => push_definition(
                &mut out,
                resolved,
                &name.name,
                DeclKind::Test,
                name.span,
                SymbolKind::METHOD,
            ),
            Decl::Fixture(FixtureDecl { name, .. }) => push_definition(
                &mut out,
                resolved,
                &name.name,
                DeclKind::Fixture,
                name.span,
                SymbolKind::FUNCTION,
            ),
            Decl::Mock(MockDecl { target, .. }) => push_definition(
                &mut out,
                resolved,
                &target.name,
                DeclKind::Tool,
                target.span,
                SymbolKind::FUNCTION,
            ),
            Decl::Effect(EffectDecl { name, .. }) => push_definition(
                &mut out,
                resolved,
                &name.name,
                DeclKind::Effect,
                name.span,
                SymbolKind::EVENT,
            ),
            Decl::Model(ModelDecl { name, .. }) => push_definition(
                &mut out,
                resolved,
                &name.name,
                DeclKind::Model,
                name.span,
                SymbolKind::CLASS,
            ),
            Decl::Server(server) => push_definition(
                &mut out,
                resolved,
                &server.name.name,
                DeclKind::Server,
                server.name.span,
                SymbolKind::CLASS,
            ),
            Decl::Import(_) | Decl::Extend(_) => {}
        }
    }
    out
}

fn push_definition(
    out: &mut Vec<Definition>,
    resolved: &Resolved,
    name: &str,
    expected: DeclKind,
    span: Span,
    kind: SymbolKind,
) {
    let Some(def_id) = resolved.symbols.lookup_def(name) else {
        return;
    };
    if resolved.symbols.get(def_id).kind == expected {
        out.push(Definition {
            def_id,
            name: name.to_string(),
            kind,
            span,
        });
    }
}

fn contains(span: Span, offset: usize) -> bool {
    span.start <= offset && offset <= span.end
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range_text<'a>(source: &'a str, range: Range) -> &'a str {
        let start = offset_for_position(source, range.start);
        let end = offset_for_position(source, range.end);
        &source[start..end]
    }

    fn offset_for_position(source: &str, position: Position) -> usize {
        crate::lsp_position_to_byte(source, position)
    }

    #[test]
    fn finds_definition_for_tool_call() {
        let src = "tool get_order(id: String) -> String\n\nagent run(id: String) -> String:\n    return get_order(id)\n";
        let range = definition_range_at(src, Position { line: 3, character: 13 }).unwrap();
        assert_eq!(range_text(src, range), "get_order");
    }

    #[test]
    fn finds_references_for_local() {
        let src = "agent run(id: String) -> String:\n    order = id\n    return order\n";
        let refs = references_at(src, Position { line: 2, character: 13 });
        let texts = refs
            .into_iter()
            .map(|range| range_text(src, range).to_string())
            .collect::<Vec<_>>();
        assert_eq!(texts, vec!["order".to_string(), "order".to_string()]);
    }

    #[test]
    fn rename_uses_resolver_identity_not_text_search() {
        let src = "tool id(x: String) -> String\n\nagent run(id: String) -> String:\n    return id\n";
        let ranges = rename_ranges_at(src, Position { line: 2, character: 10 }, "ticket_id").unwrap();
        let texts = ranges
            .into_iter()
            .map(|range| range_text(src, range).to_string())
            .collect::<Vec<_>>();
        assert_eq!(texts, vec!["id".to_string(), "id".to_string()]);
    }

    #[test]
    fn rejects_invalid_rename_identifier() {
        let src = "agent run(id: String) -> String:\n    return id\n";
        assert!(rename_ranges_at(src, Position { line: 0, character: 11 }, "not valid").is_none());
    }

    #[test]
    fn workspace_symbols_include_ai_native_decls() {
        let uri = Url::parse("file:///workspace/main.cor").unwrap();
        let src = "effect retrieval:\n    data: grounded\n\nmodel haiku:\n    capability: basic\n\nprompt answer(x: String) -> String:\n    \"ok\"\n";
        let symbols = workspace_symbols_for_document(uri, src, "");
        let names = symbols
            .into_iter()
            .map(|symbol| symbol.name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["retrieval", "haiku", "answer"]);
    }
}
