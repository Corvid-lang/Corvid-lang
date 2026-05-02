//! Approval-card formatter.
//!
//! When the runtime asks the user to approve a dangerous call,
//! it builds an `ApprovalCard` from the request — a typed,
//! serializable summary the CLI / IDE / web approver UI can
//! render any way it likes (terminal table, HTML page, etc.).
//! The card carries a humanized title, a risk classification,
//! per-argument records (with sensitive-value redaction
//! applied), context lines, and an optional diff preview.
//!
//! `ApprovalCard::from_request` is the constructor that pulls
//! all of this together; `to_html` is the rendering path used
//! by the web approver. The remaining `fn` helpers live here
//! because they're consumed only by the card builders.

use serde::{Deserialize, Serialize};

use super::ApprovalRequest;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalCard {
    pub label: String,
    pub title: String,
    pub risk: ApprovalRisk,
    pub arguments: Vec<ApprovalCardArgument>,
    pub context: Vec<String>,
    pub diff_preview: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRisk {
    Review,
    MoneyMovement,
    ExternalSideEffect,
    Irreversible,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalCardArgument {
    pub index: usize,
    pub json_type: String,
    pub value: serde_json::Value,
    pub redacted: bool,
}

impl ApprovalCard {
    pub fn from_request(req: &ApprovalRequest) -> Self {
        let arguments = req
            .args
            .iter()
            .enumerate()
            .map(|(index, value)| ApprovalCardArgument {
                index,
                json_type: json_type_name(value).into(),
                value: redact_if_sensitive(value),
                redacted: is_sensitive_value(value),
            })
            .collect::<Vec<_>>();
        let title = humanize_label(&req.label);
        let risk = infer_risk(&req.label, &req.args);
        Self {
            label: req.label.clone(),
            title: title.clone(),
            risk,
            context: build_context(&req.label, &arguments),
            diff_preview: Some(build_diff_preview(&title, risk, &arguments)),
            arguments,
        }
    }

    pub fn render_text(&self) -> String {
        let mut out = String::new();
        out.push_str("corvid approval card\n");
        out.push_str(&format!("  action: {}\n", self.title));
        out.push_str(&format!("  label: {}\n", self.label));
        out.push_str(&format!("  risk: {:?}\n", self.risk));
        for arg in &self.arguments {
            let suffix = if arg.redacted { " (redacted)" } else { "" };
            out.push_str(&format!(
                "  arg {} [{}]{}: {}\n",
                arg.index, arg.json_type, suffix, arg.value
            ));
        }
        for line in &self.context {
            out.push_str(&format!("  context: {line}\n"));
        }
        if let Some(diff) = &self.diff_preview {
            out.push_str("  preview:\n");
            out.push_str(diff);
            out.push('\n');
        }
        out
    }

    pub fn render_html(&self) -> String {
        let mut out = String::new();
        out.push_str("<!doctype html>\n");
        out.push_str("<html lang=\"en\"><head><meta charset=\"utf-8\">\n");
        out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
        out.push_str("<title>Corvid Approval</title>\n");
        out.push_str("<style>");
        out.push_str("body{font-family:system-ui,sans-serif;margin:0;background:#f6f7f9;color:#111827}");
        out.push_str("main{max-width:760px;margin:32px auto;padding:0 16px}");
        out.push_str(".card{background:#fff;border:1px solid #d1d5db;border-radius:8px;padding:20px}");
        out.push_str("h1{font-size:22px;margin:0 0 12px}.meta{color:#4b5563;margin:4px 0}");
        out.push_str("pre{background:#111827;color:#f9fafb;padding:12px;border-radius:6px;overflow:auto}");
        out.push_str("table{width:100%;border-collapse:collapse;margin-top:12px}");
        out.push_str("th,td{text-align:left;border-top:1px solid #e5e7eb;padding:8px;vertical-align:top}");
        out.push_str(".actions{display:flex;gap:8px;margin-top:16px}");
        out.push_str("button{border:1px solid #9ca3af;border-radius:6px;background:#fff;padding:8px 12px}");
        out.push_str("button.primary{background:#065f46;color:#fff;border-color:#065f46}");
        out.push_str("</style></head><body><main><section class=\"card\">\n");
        out.push_str(&format!("<h1>{}</h1>\n", escape_html(&self.title)));
        out.push_str(&format!(
            "<p class=\"meta\"><strong>Label:</strong> {}</p>\n",
            escape_html(&self.label)
        ));
        out.push_str(&format!(
            "<p class=\"meta\"><strong>Risk:</strong> {:?}</p>\n",
            self.risk
        ));
        out.push_str("<h2>Context</h2><ul>\n");
        for line in &self.context {
            out.push_str(&format!("<li>{}</li>\n", escape_html(line)));
        }
        out.push_str("</ul>\n<h2>Arguments</h2><table><thead><tr><th>#</th><th>Type</th><th>Value</th></tr></thead><tbody>\n");
        for arg in &self.arguments {
            let redacted = if arg.redacted { " (redacted)" } else { "" };
            out.push_str(&format!(
                "<tr><td>{}</td><td>{}{}</td><td><code>{}</code></td></tr>\n",
                arg.index,
                escape_html(&arg.json_type),
                redacted,
                escape_html(&arg.value.to_string())
            ));
        }
        out.push_str("</tbody></table>\n");
        if let Some(diff) = &self.diff_preview {
            out.push_str("<h2>Preview</h2>\n");
            out.push_str(&format!("<pre>{}</pre>\n", escape_html(diff)));
        }
        out.push_str("<div class=\"actions\"><button class=\"primary\">Approve</button><button>Deny</button></div>\n");
        out.push_str("</section></main></body></html>\n");
        out
    }
}

fn escape_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn build_context(label: &str, args: &[ApprovalCardArgument]) -> Vec<String> {
    let plural = if args.len() == 1 { "" } else { "s" };
    let types = args
        .iter()
        .map(|arg| format!("arg{}:{}", arg.index, arg.json_type))
        .collect::<Vec<_>>()
        .join(", ");
    vec![
        format!("why: program requested approval `{label}` before a protected action"),
        format!("scope: one decision covers this label and exact argument payload"),
        format!(
            "argument inspection: {} argument{}{}",
            args.len(),
            plural,
            if types.is_empty() {
                String::new()
            } else {
                format!(" ({types})")
            }
        ),
    ]
}

fn build_diff_preview(
    title: &str,
    risk: ApprovalRisk,
    args: &[ApprovalCardArgument],
) -> String {
    let mut out = String::new();
    out.push_str("    before: protected action is blocked\n");
    out.push_str(&format!(
        "    after: `{title}` may proceed as {}\n",
        risk_preview_label(risk)
    ));
    if args.is_empty() {
        out.push_str("    arguments: <none>");
    } else {
        out.push_str("    arguments:\n");
        for arg in args {
            let suffix = if arg.redacted { " (redacted)" } else { "" };
            out.push_str(&format!(
                "      - arg {} [{}]{} = {}\n",
                arg.index, arg.json_type, suffix, arg.value
            ));
        }
        while out.ends_with('\n') {
            out.pop();
        }
    }
    out
}

fn risk_preview_label(risk: ApprovalRisk) -> &'static str {
    match risk {
        ApprovalRisk::Review => "a reviewed operation",
        ApprovalRisk::MoneyMovement => "a money-moving operation",
        ApprovalRisk::ExternalSideEffect => "an external side effect",
        ApprovalRisk::Irreversible => "an irreversible operation",
    }
}

fn humanize_label(label: &str) -> String {
    let mut out = String::new();
    for (index, ch) in label.chars().enumerate() {
        if index > 0 && ch.is_ascii_uppercase() {
            out.push(' ');
        }
        if index == 0 {
            out.extend(ch.to_uppercase());
        } else {
            out.push(ch);
        }
    }
    out.replace(['_', '-'], " ")
}

fn infer_risk(label: &str, args: &[serde_json::Value]) -> ApprovalRisk {
    let lower = label.to_ascii_lowercase();
    if lower.contains("delete") || lower.contains("irreversible") || lower.contains("void") {
        ApprovalRisk::Irreversible
    } else if lower.contains("refund")
        || lower.contains("charge")
        || lower.contains("payment")
        || args
            .iter()
            .any(|value| value.as_f64().is_some_and(|n| n.abs() >= 100.0))
    {
        ApprovalRisk::MoneyMovement
    } else if lower.contains("send") || lower.contains("external") || lower.contains("email") {
        ApprovalRisk::ExternalSideEffect
    } else {
        ApprovalRisk::Review
    }
}

fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn redact_if_sensitive(value: &serde_json::Value) -> serde_json::Value {
    if is_sensitive_value(value) {
        serde_json::Value::String("<redacted>".into())
    } else {
        value.clone()
    }
}

fn is_sensitive_value(value: &serde_json::Value) -> bool {
    value.as_str().is_some_and(|text| {
        let lower = text.to_ascii_lowercase();
        lower.contains("secret")
            || lower.contains("token")
            || lower.contains("password")
            || lower.chars().filter(|ch| ch.is_ascii_digit()).count() >= 12
    })
}
