//! Static HTML renderer for the executable effects specification.
//!
//! The generator consumes the same fenced Corvid examples as
//! `corvid test spec`, so the website never drifts into hand-written
//! examples that the compiler does not verify.

use crate::{extract_spec_examples, Expectation, SpecExample};
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct SpecSiteReport {
    pub output_dir: PathBuf,
    pub pages: Vec<SpecSitePage>,
    pub example_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpecSitePage {
    pub source: PathBuf,
    pub output: PathBuf,
    pub examples: usize,
}

pub fn build_spec_site(spec_dir: &Path, out_dir: &Path) -> Result<SpecSiteReport> {
    fs::create_dir_all(out_dir)
        .with_context(|| format!("failed to create `{}`", out_dir.display()))?;
    let examples = extract_spec_examples(spec_dir)?;
    let mut examples_by_file = BTreeMap::<PathBuf, Vec<SpecExample>>::new();
    for example in examples {
        examples_by_file
            .entry(example.file.clone())
            .or_default()
            .push(example);
    }

    let mut pages = Vec::new();
    for md in collect_markdown_files(spec_dir)? {
        let markdown =
            fs::read_to_string(&md).with_context(|| format!("cannot read `{}`", md.display()))?;
        let file_examples = examples_by_file.remove(&md).unwrap_or_default();
        let output = out_dir.join(html_file_name(&md));
        let html = render_spec_page(spec_dir, &md, &markdown, &file_examples);
        fs::write(&output, html).with_context(|| format!("cannot write `{}`", output.display()))?;
        pages.push(SpecSitePage {
            source: md,
            output,
            examples: file_examples.len(),
        });
    }

    let index = render_spec_index(&pages);
    fs::write(out_dir.join("index.html"), index)
        .with_context(|| format!("cannot write `{}`", out_dir.join("index.html").display()))?;
    fs::write(out_dir.join("site.js"), SITE_JS)
        .with_context(|| format!("cannot write `{}`", out_dir.join("site.js").display()))?;
    fs::write(out_dir.join("site.css"), SITE_CSS)
        .with_context(|| format!("cannot write `{}`", out_dir.join("site.css").display()))?;

    let example_count = pages.iter().map(|p| p.examples).sum();
    Ok(SpecSiteReport {
        output_dir: out_dir.to_path_buf(),
        pages,
        example_count,
    })
}

pub fn render_spec_site_report(report: &SpecSiteReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "spec site written to `{}`\n",
        report.output_dir.display()
    ));
    out.push_str(&format!(
        "{} page(s), {} runnable Corvid example(s)\n",
        report.pages.len(),
        report.example_count
    ));
    for page in &report.pages {
        out.push_str(&format!(
            "  {} -> {} ({} examples)\n",
            page.source.display(),
            page.output.display(),
            page.examples
        ));
    }
    out
}

fn collect_markdown_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)
        .with_context(|| format!("cannot read directory `{}`", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn html_file_name(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("page");
    format!("{stem}.html")
}

fn render_spec_index(pages: &[SpecSitePage]) -> String {
    let mut body = String::new();
    body.push_str("<h1>Corvid executable effects specification</h1>\n");
    body.push_str("<p>Every page is generated from <code>docs/effects-spec</code>. Every runnable block is the exact Corvid source verified by <code>corvid test spec</code>.</p>\n");
    body.push_str("<ol class=\"toc\">\n");
    for page in pages {
        let name = page
            .source
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("page");
        body.push_str(&format!(
            "<li><a href=\"{}\">{}</a> <span>{} runnable examples</span></li>\n",
            html_escape(&html_file_name(&page.source)),
            html_escape(name),
            page.examples
        ));
    }
    body.push_str("</ol>\n");
    wrap_html("Corvid spec", &body)
}

fn render_spec_page(
    spec_dir: &Path,
    file: &Path,
    markdown: &str,
    examples: &[SpecExample],
) -> String {
    let title = file.file_stem().and_then(|s| s.to_str()).unwrap_or("Spec");
    let rel = file.strip_prefix(spec_dir).unwrap_or(file);
    let mut body = String::new();
    body.push_str(&format!(
        "<a class=\"back\" href=\"index.html\">← spec index</a><h1>{}</h1><p class=\"source\">{}</p>\n",
        html_escape(title),
        html_escape(&rel.display().to_string())
    ));
    body.push_str("<section class=\"markdown\"><pre>");
    body.push_str(&html_escape(markdown));
    body.push_str("</pre></section>\n");
    body.push_str("<section class=\"examples\"><h2>Runnable Corvid blocks</h2>\n");
    if examples.is_empty() {
        body.push_str("<p>No runnable Corvid fences in this section.</p>\n");
    }
    for example in examples {
        body.push_str(&render_example_card(example));
    }
    body.push_str("</section>\n");
    wrap_html(title, &body)
}

fn render_example_card(example: &SpecExample) -> String {
    let expectation = match &example.expectation {
        Expectation::Compile => "compile",
        Expectation::ErrorAny => "error",
        Expectation::ErrorContains(_) => "error contains",
        Expectation::Skip => "skip",
    };
    let json_source = serde_json::to_string(&example.source).unwrap_or_else(|_| "\"\"".into());
    format!(
        "<article class=\"example\"><div class=\"example-head\"><span>line {line}, block {block}</span><span>expect: {expectation}</span></div><pre><code>{source}</code></pre><button data-corvid-source='{json_source}'>Run in REPL</button></article>\n",
        line = example.line,
        block = example.block_index + 1,
        expectation = html_escape(expectation),
        source = html_escape(&example.source),
        json_source = html_escape(&json_source)
    )
}

fn wrap_html(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{}</title><link rel=\"stylesheet\" href=\"site.css\"></head><body><main>{}</main><script src=\"site.js\"></script></body></html>",
        html_escape(title),
        body
    )
}

fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const SITE_JS: &str = r#"document.addEventListener("click", async (event) => {
  const button = event.target.closest("button[data-corvid-source]");
  if (!button) return;
  const source = button.dataset.corvidSource;
  await navigator.clipboard.writeText(source);
  button.textContent = "Copied for corvid repl";
  button.classList.add("copied");
});"#;

const SITE_CSS: &str = r#":root {
  color-scheme: light;
  --ink: #111111;
  --muted: #636363;
  --paper: #f7f3ea;
  --card: #fffaf0;
  --line: #d8cdb8;
  --accent: #0b5f5a;
}
* { box-sizing: border-box; }
body {
  margin: 0;
  font-family: Georgia, "Times New Roman", serif;
  color: var(--ink);
  background:
    radial-gradient(circle at 20% 0%, rgba(11,95,90,0.15), transparent 30rem),
    linear-gradient(135deg, #fbf8f0, var(--paper));
}
main { max-width: 1120px; margin: 0 auto; padding: 56px 24px 96px; }
h1 { font-size: clamp(2.4rem, 7vw, 5rem); letter-spacing: -0.055em; line-height: 0.92; margin: 0 0 24px; }
h2 { font-size: 1.6rem; margin-top: 48px; }
p, li { font-size: 1.08rem; line-height: 1.65; }
a { color: var(--accent); }
.source, .toc span, .example-head { color: var(--muted); }
.toc { display: grid; gap: 12px; padding-left: 24px; }
.toc li { padding: 12px 0; border-bottom: 1px solid var(--line); }
.markdown pre, .example {
  border: 1px solid var(--line);
  background: rgba(255,250,240,0.72);
  border-radius: 18px;
  box-shadow: 0 24px 80px rgba(58, 45, 20, 0.08);
}
.markdown pre {
  white-space: pre-wrap;
  padding: 28px;
  overflow: auto;
}
.examples { display: grid; gap: 18px; }
.example { padding: 18px; }
.example-head { display: flex; justify-content: space-between; gap: 16px; font-size: 0.9rem; margin-bottom: 10px; }
.example pre { overflow: auto; background: #15130f; color: #f7ead2; padding: 18px; border-radius: 12px; }
button {
  border: 0;
  border-radius: 999px;
  background: var(--accent);
  color: white;
  padding: 10px 16px;
  font-weight: 700;
  cursor: pointer;
}
button.copied { background: #2f7d32; }
.back { display: inline-block; margin-bottom: 28px; }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_site_renders_run_buttons_for_corvid_fences() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = tmp.path().join("spec");
        let out = tmp.path().join("site");
        fs::create_dir(&spec).unwrap();
        fs::write(
            spec.join("01-demo.md"),
            r#"# Demo

```corvid
# expect: compile
agent main() -> Int:
    return 1
```
"#,
        )
        .unwrap();
        let report = build_spec_site(&spec, &out).unwrap();
        assert_eq!(report.example_count, 1);
        let page = fs::read_to_string(out.join("01-demo.html")).unwrap();
        assert!(page.contains("Run in REPL"));
        assert!(page.contains("agent main()"));
        assert!(fs::read_to_string(out.join("index.html")).unwrap().contains("01-demo"));
    }
}
