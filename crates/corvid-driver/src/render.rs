//! Pretty diagnostic rendering via `ariadne`.
//!
//! Every diagnostic carries a span (byte offsets) and a rich message.
//! This module turns them into the Rust-style multi-line output that
//! makes first impressions count.

use crate::Diagnostic;
use ariadne::{Color, Label, Report, ReportKind, Source};
use std::path::Path;

/// Render a diagnostic to a string suitable for stderr.
///
/// Uses `ariadne` to produce multi-line output with the offending span
/// highlighted under the source code, plus the help hint as a footer.
pub fn render_pretty(diag: &Diagnostic, source_path: &Path, source: &str) -> String {
    let filename = source_path.display().to_string();
    let span = diag.span.start..diag.span.end.max(diag.span.start + 1);

    let code = detect_error_code(&diag.message);
    let kind = ReportKind::Custom("error", Color::Red);

    let mut builder = Report::build(kind, filename.as_str(), span.start)
        .with_code(code)
        .with_message(short_headline(&diag.message))
        .with_label(
            Label::new((filename.as_str(), span))
                .with_message(label_for(&diag.message))
                .with_color(Color::Red),
        );

    if let Some(hint) = &diag.hint {
        builder = builder.with_help(hint.as_str());
    }

    let mut buf = Vec::new();
    let _ = builder
        .finish()
        .write((filename.as_str(), Source::from(source)), &mut buf);
    String::from_utf8_lossy(&buf).to_string()
}

/// Render every diagnostic in sequence, followed by a summary line.
pub fn render_all_pretty(
    diags: &[Diagnostic],
    source_path: &Path,
    source: &str,
) -> String {
    let mut out = String::new();
    for d in diags {
        out.push_str(&render_pretty(d, source_path, source));
    }
    out.push_str(&format!("\n{} error(s) found.\n", diags.len()));
    out
}

/// Best-effort mapping from a diagnostic message to a stable error code.
/// These codes are documented and searchable.
fn detect_error_code(msg: &str) -> &'static str {
    if msg.contains("dangerous tool") && msg.contains("without a prior") {
        "E0101"
    } else if msg.contains("wrong number of arguments") {
        "E0201"
    } else if msg.contains("no field named") {
        "E0202"
    } else if msg.contains("cannot call a value") {
        "E0203"
    } else if msg.contains("field access requires a struct") {
        "E0204"
    } else if msg.contains("is a type, not a value") {
        "E0205"
    } else if msg.contains("is a function; call it with") {
        "E0206"
    } else if msg.contains("return type mismatch") {
        "E0207"
    } else if msg.contains("type mismatch") {
        "E0208"
    } else if msg.contains("undefined name") {
        "E0301"
    } else if msg.contains("duplicate declaration") {
        "E0302"
    } else if msg.contains("unterminated string") {
        "E0001"
    } else if msg.contains("tab character used for indentation") {
        "E0002"
    } else if msg.contains("unexpected character") {
        "E0003"
    } else if msg.contains("chained comparisons") {
        "E0051"
    } else if msg.contains("unclosed") {
        "E0052"
    } else if msg.contains("expected an indented block") {
        "E0053"
    } else if msg.contains("block is empty") {
        "E0054"
    } else {
        "E0000"
    }
}

/// Condensed one-line headline for the report's top message.
/// ariadne duplicates the message if we pass the full text, so we keep
/// the headline short and put detail on the label and help lines.
fn short_headline(msg: &str) -> String {
    // Strip anything after a colon so the headline stays tight.
    if let Some(idx) = msg.find(':') {
        if idx < 80 {
            return msg[..idx].to_string();
        }
    }
    msg.to_string()
}

fn label_for(msg: &str) -> String {
    // A per-error hint for the underline caret. These are human-readable
    // phrasings that complement the top headline.
    if msg.contains("dangerous tool") {
        "this call needs prior approval".into()
    } else if msg.contains("undefined name") {
        "not declared in this scope".into()
    } else if msg.contains("duplicate declaration") {
        "conflicts with an earlier declaration".into()
    } else if msg.contains("no field named") {
        "field does not exist".into()
    } else if msg.contains("wrong number of arguments") {
        "wrong argument count".into()
    } else if msg.contains("return type mismatch") {
        "wrong return type".into()
    } else if msg.contains("is a type, not a value") {
        "types cannot be used as values".into()
    } else if msg.contains("is a function; call it with") {
        "missing `()` for call".into()
    } else if msg.contains("type mismatch") {
        "wrong type here".into()
    } else {
        msg.to_string()
    }
}
