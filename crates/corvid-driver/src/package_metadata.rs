//! Package metadata page rendering.
//!
//! Registry pages and CLI package inspection should show the same semantic
//! contract the compiler enforces: exports, effects, approvals, provenance,
//! replayability, determinism, and policy violations.

use std::path::Path;

use anyhow::{Context, Result};
use corvid_resolve::{DeclKind, ModuleSemanticSummary};
use serde::Serialize;

use crate::modules::summarize_module_source;

#[derive(Debug, Clone, Serialize)]
pub struct PackageMetadata {
    pub name: String,
    pub version: String,
    pub uri: String,
    pub install: String,
    pub signature: Option<String>,
    pub summary: ModuleSemanticSummary,
}

pub fn package_metadata_from_source(
    source_path: &Path,
    name: &str,
    version: &str,
    signature: Option<&str>,
) -> Result<PackageMetadata> {
    validate_package_name_for_metadata(name)?;
    let source = std::fs::read_to_string(source_path)
        .with_context(|| format!("read package source `{}`", source_path.display()))?;
    let summary = summarize_module_source(&source)
        .map_err(|message| anyhow::anyhow!("package source failed semantic summary build: {message}"))?;
    let uri = format!("corvid://{name}/v{version}");
    Ok(PackageMetadata {
        name: name.to_string(),
        version: version.to_string(),
        uri,
        install: format!("corvid add {name}@{version}"),
        signature: signature.map(str::to_string),
        summary,
    })
}

pub fn render_package_metadata_markdown(metadata: &PackageMetadata) -> String {
    let mut out = String::new();
    out.push_str(&format!("# `{}`\n\n", metadata.name));
    out.push_str(&format!("- version: `{}`\n", metadata.version));
    out.push_str(&format!("- uri: `{}`\n", metadata.uri));
    out.push_str(&format!("- install: `{}`\n", metadata.install));
    match &metadata.signature {
        Some(signature) => out.push_str(&format!("- signature: `{signature}`\n")),
        None => out.push_str("- signature: `not supplied`\n"),
    }
    out.push_str("\n## Exported Contract\n\n");
    if metadata.summary.exports.is_empty() {
        out.push_str("No public exports.\n");
        return out;
    }
    out.push_str("| Export | Kind | Effects | Approval | Grounding | Replay | Determinism | Notes |\n");
    out.push_str("|---|---|---|---|---|---|---|---|\n");
    for export in metadata.summary.exports.values() {
        let agent = metadata.summary.agents.get(&export.name);
        let effects = if export.effect_names.is_empty() {
            "`none`".to_string()
        } else {
            export
                .effect_names
                .iter()
                .map(|effect| format!("`{effect}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let approval = if export.approval_required {
            "required"
        } else {
            "not required"
        };
        let grounding = grounding_cell(export.grounded_source, export.grounded_return);
        let replay = if export.replayable { "replayable" } else { "not declared" };
        let deterministic = if export.deterministic {
            "deterministic"
        } else {
            "not declared"
        };
        let notes = notes_cell(agent);
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} | {} | {} |\n",
            export.name,
            kind_label(export.kind),
            effects,
            approval,
            grounding,
            replay,
            deterministic,
            notes
        ));
    }
    out
}

fn grounding_cell(source: bool, ret: bool) -> &'static str {
    match (source, ret) {
        (true, true) => "source + return",
        (true, false) => "source",
        (false, true) => "return",
        (false, false) => "none",
    }
}

fn notes_cell(agent: Option<&corvid_resolve::AgentSemanticSummary>) -> String {
    let Some(agent) = agent else {
        return String::new();
    };
    let mut notes = Vec::new();
    if let Some(cost) = &agent.cost {
        notes.push(format!("cost `{}`", format_dimension_value(cost)));
    }
    if !agent.violations.is_empty() {
        notes.push(format!("{} effect violation(s)", agent.violations.len()));
    }
    if agent.approval_required {
        notes.push("approval path".to_string());
    }
    notes.join("; ")
}

fn kind_label(kind: DeclKind) -> &'static str {
    match kind {
        DeclKind::Import => "import",
        DeclKind::ImportedUse => "imported use",
        DeclKind::Type => "type",
        DeclKind::Tool => "tool",
        DeclKind::Prompt => "prompt",
        DeclKind::Agent => "agent",
        DeclKind::Eval => "eval",
        DeclKind::Test => "test",
        DeclKind::Effect => "effect",
        DeclKind::Model => "model",
    }
}

fn format_dimension_value(value: &corvid_ast::DimensionValue) -> String {
    match value {
        corvid_ast::DimensionValue::Bool(v) => v.to_string(),
        corvid_ast::DimensionValue::Name(v) => v.clone(),
        corvid_ast::DimensionValue::Cost(v) => format!("${v:.6}"),
        corvid_ast::DimensionValue::Number(v) => format!("{v:.3}"),
        corvid_ast::DimensionValue::Streaming { backpressure } => backpressure.label(),
        corvid_ast::DimensionValue::ConfidenceGated {
            threshold,
            above,
            below,
        } => format!("{}_if_confident({threshold:.3}) else {}", above, below),
    }
}

fn validate_package_name_for_metadata(name: &str) -> Result<()> {
    if name.starts_with('@') && name.contains('/') && !name.ends_with('/') {
        Ok(())
    } else {
        anyhow::bail!("package name must be scoped, e.g. `@scope/name`")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_page_exposes_ai_native_contract() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("pkg.cor");
        std::fs::write(
            &source,
            "\
effect retrieval:
    data: grounded

public type Answer:
    text: String

public tool search(q: String) -> Grounded<Answer> uses retrieval

public @replayable
agent answer(q: String) -> Grounded<Answer>:
    return search(q)
",
        )
        .unwrap();

        let metadata = package_metadata_from_source(
            &source,
            "@scope/answers",
            "1.0.0",
            Some("ed25519:key:pub:sig"),
        )
        .unwrap();
        let markdown = render_package_metadata_markdown(&metadata);

        assert!(markdown.contains("`@scope/answers`"), "{markdown}");
        assert!(markdown.contains("`ed25519:key:pub:sig`"), "{markdown}");
        assert!(markdown.contains("`search`"), "{markdown}");
        assert!(markdown.contains("`retrieval`"), "{markdown}");
        assert!(markdown.contains("return"), "{markdown}");
        assert!(markdown.contains("replayable"), "{markdown}");
    }
}
