use super::{stable_id, RagDocument};
use crate::errors::RuntimeError;
use std::path::Path;

pub fn document_from_text(
    id: impl Into<String>,
    source: impl Into<String>,
    media_type: impl Into<String>,
    text: impl Into<String>,
) -> Result<RagDocument, RuntimeError> {
    let id = id.into();
    if id.trim().is_empty() {
        return Err(RuntimeError::Other(
            "std.rag document id must not be empty".to_string(),
        ));
    }
    Ok(RagDocument {
        id,
        source: source.into(),
        media_type: media_type.into(),
        text: text.into(),
    })
}

pub fn load_markdown(path: impl AsRef<Path>) -> Result<RagDocument, RuntimeError> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|err| {
        RuntimeError::Other(format!(
            "failed to read markdown document `{}`: {err}",
            path.display()
        ))
    })?;
    let id = stable_id(path.display().to_string().as_bytes());
    document_from_text(id, path.display().to_string(), "text/markdown", text)
}

pub fn load_html(path: impl AsRef<Path>) -> Result<RagDocument, RuntimeError> {
    let path = path.as_ref();
    let html = std::fs::read_to_string(path).map_err(|err| {
        RuntimeError::Other(format!(
            "failed to read html document `{}`: {err}",
            path.display()
        ))
    })?;
    let text = extract_html_text(&html);
    let id = stable_id(path.display().to_string().as_bytes());
    document_from_text(id, path.display().to_string(), "text/html", text)
}

pub fn load_pdf(path: impl AsRef<Path>) -> Result<RagDocument, RuntimeError> {
    let path = path.as_ref();
    let text = pdf_extract::extract_text(path).map_err(|err| {
        RuntimeError::Other(format!(
            "failed to read pdf document `{}`: {err}",
            path.display()
        ))
    })?;
    let id = stable_id(path.display().to_string().as_bytes());
    document_from_text(
        id,
        path.display().to_string(),
        "application/pdf",
        normalize_html_text(&text),
    )
}

fn extract_html_text(html: &str) -> String {
    let stripped = strip_html_blocks(html, "script");
    let stripped = strip_html_blocks(&stripped, "style");
    let mut out = String::with_capacity(stripped.len());
    let mut in_tag = false;
    let mut tag_name = String::new();
    for ch in stripped.chars() {
        if in_tag {
            if ch == '>' {
                let tag = tag_name
                    .trim()
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("");
                if matches!(
                    tag,
                    "br" | "p"
                        | "div"
                        | "li"
                        | "tr"
                        | "section"
                        | "article"
                        | "header"
                        | "footer"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                ) {
                    out.push('\n');
                }
                tag_name.clear();
                in_tag = false;
            } else {
                tag_name.push(ch.to_ascii_lowercase());
            }
            continue;
        }
        if ch == '<' {
            in_tag = true;
            continue;
        }
        out.push(ch);
    }
    normalize_html_text(&decode_html_entities(&out))
}

fn strip_html_blocks(html: &str, tag: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0;
    while let Some(relative_start) = lower[cursor..].find(&open) {
        let start = cursor + relative_start;
        out.push_str(&html[cursor..start]);
        let after_start = match lower[start..].find('>') {
            Some(offset) => start + offset + 1,
            None => {
                cursor = html.len();
                break;
            }
        };
        let block_end = match lower[after_start..].find(&close) {
            Some(offset) => after_start + offset + close.len(),
            None => {
                cursor = html.len();
                break;
            }
        };
        cursor = block_end;
    }
    if cursor < html.len() {
        out.push_str(&html[cursor..]);
    }
    out
}

fn decode_html_entities(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn normalize_html_text(text: &str) -> String {
    let mut out = String::new();
    let mut pending_space = false;
    let mut previous_was_newline = false;
    for ch in text.chars() {
        if ch == '\r' {
            continue;
        }
        if ch == '\n' {
            if !out.is_empty() && !previous_was_newline {
                out.push('\n');
            }
            pending_space = false;
            previous_was_newline = true;
            continue;
        }
        if ch.is_whitespace() {
            pending_space = !previous_was_newline;
            continue;
        }
        if pending_space && !out.is_empty() && !previous_was_newline {
            out.push(' ');
        }
        out.push(ch);
        pending_space = false;
        previous_was_newline = false;
    }
    out.trim().to_string()
}
