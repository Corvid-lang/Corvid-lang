use crate::position::byte_span_to_lsp_range;
use corvid_driver::{compile_with_config_at_path, load_corvid_config_for};
use lsp_types::{
    Diagnostic, DiagnosticSeverity, DiagnosticTag, NumberOrString, Url,
};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct DocumentSnapshot {
    pub uri: Url,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct AnalysisResult {
    pub diagnostics: Vec<Diagnostic>,
}

pub fn analyze_document(snapshot: &DocumentSnapshot) -> AnalysisResult {
    let path = snapshot
        .uri
        .to_file_path()
        .unwrap_or_else(|_| PathBuf::from("<memory>"));
    let config = if path == Path::new("<memory>") {
        None
    } else {
        load_corvid_config_for(&path)
    };
    let compiled = compile_with_config_at_path(&snapshot.text, &path, config.as_ref());
    let diagnostics = compiled
        .diagnostics
        .iter()
        .map(|diagnostic| to_lsp_diagnostic(&snapshot.text, diagnostic))
        .collect();
    AnalysisResult { diagnostics }
}

fn to_lsp_diagnostic(source: &str, diagnostic: &corvid_driver::Diagnostic) -> Diagnostic {
    Diagnostic {
        range: byte_span_to_lsp_range(source, diagnostic.span.start, diagnostic.span.end),
        severity: Some(DiagnosticSeverity::ERROR),
        code: stable_code(&diagnostic.message).map(NumberOrString::String),
        code_description: None,
        source: Some("corvid".to_string()),
        message: match &diagnostic.hint {
            Some(hint) => format!("{}\nhelp: {}", diagnostic.message, hint),
            None => diagnostic.message.clone(),
        },
        related_information: None,
        tags: Some(Vec::<DiagnosticTag>::new()),
        data: None,
    }
}

fn stable_code(message: &str) -> Option<String> {
    message
        .split_whitespace()
        .find(|part| {
            part.len() == 5
                && part.starts_with('E')
                && part[1..].chars().all(|ch| ch.is_ascii_digit())
        })
        .map(|part| part.trim_matches([':', ',']).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(src: &str) -> DocumentSnapshot {
        DocumentSnapshot {
            uri: Url::parse("file:///workspace/main.cor").unwrap(),
            text: src.to_string(),
        }
    }

    #[test]
    fn clean_document_has_no_diagnostics() {
        let result = analyze_document(&snapshot(
            "agent answer() -> Int:\n    return 42\n",
        ));
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn unknown_name_surfaces_live_lsp_diagnostic() {
        let result = analyze_document(&snapshot(
            "agent answer() -> Int:\n    return missing_name\n",
        ));
        assert_eq!(result.diagnostics.len(), 1);
        let diagnostic = &result.diagnostics[0];
        assert_eq!(diagnostic.source.as_deref(), Some("corvid"));
        assert_eq!(diagnostic.severity, Some(DiagnosticSeverity::ERROR));
        assert!(
            diagnostic.message.contains("missing_name"),
            "{diagnostic:?}"
        );
        assert_eq!(diagnostic.range.start.line, 1);
    }

    #[test]
    fn approval_boundary_violation_uses_compiler_error_message() {
        let result = analyze_document(&snapshot(
            "tool send_email(to: String) -> Nothing dangerous\n\nagent bad(to: String) -> Nothing:\n    send_email(to)\n",
        ));
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("dangerous")
                    || diagnostic.message.contains("approve")),
            "{:?}",
            result.diagnostics
        );
    }
}
