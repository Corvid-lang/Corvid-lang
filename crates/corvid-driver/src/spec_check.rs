//! Spec example extractor + verifier.
//!
//! Scans every `.md` file under a spec directory, pulls each fenced
//! ```corvid``` code block, and compiles it against the current
//! Corvid toolchain. A directive on the first comment line inside
//! each block tells the verifier what outcome to expect:
//!
//! Example fence contents: `# expect: compile`, followed by Corvid code.
//!
//! Supported directives:
//!
//! ```text
//!   `# expect: compile`           — must compile with zero errors (default)
//!   `# expect: error`             — must produce at least one error
//!   `# expect: error "pattern"`   — must produce an error whose message
//!                                   contains the pattern (case-sensitive)
//!   `# expect: skip`              — illustrative fragment, don't compile
//! ```
//!
//! When the extractor + verifier run under `corvid test spec`, a
//! mismatch between the declared expectation and the actual compile
//! outcome fails the build. This is the mechanism that keeps the
//! specification and the compiler in lockstep: if the spec claims an
//! example compiles and the compiler disagrees, CI fails.
//!
//! See `docs/effects-spec/01-dimensional-syntax.md` §7 for the
//! spec↔compiler bidirectional sync invention this module implements.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::{compile, CompileResult};

/// A single code block extracted from a markdown spec file.
#[derive(Debug, Clone)]
pub struct SpecExample {
    /// Markdown file the block came from.
    pub file: PathBuf,
    /// Line of the opening fence (1-indexed, suitable for error messages).
    pub line: usize,
    /// Zero-indexed position of the block within its file — so if a
    /// file has five corvid blocks, the third is at `block_index = 2`.
    pub block_index: usize,
    /// The code inside the fence, with the expectation directive
    /// already stripped so the compiler sees only the program.
    pub source: String,
    /// What outcome the spec claims this block produces.
    pub expectation: Expectation,
}

/// What the spec claims about a block's compile outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expectation {
    /// Must compile with zero errors (default when no directive given).
    Compile,
    /// Must produce at least one error.
    ErrorAny,
    /// Must produce an error whose message contains the given substring.
    ErrorContains(String),
    /// Illustrative fragment — not verified.
    Skip,
}

/// The verdict for a single example after running the compiler.
#[derive(Debug, Clone)]
pub struct SpecVerdict {
    pub example: SpecExample,
    pub kind: VerdictKind,
}

#[derive(Debug, Clone)]
pub enum VerdictKind {
    /// Example matched its expectation.
    Pass,
    /// Mismatch between expectation and actual compile outcome.
    Fail { reason: String },
    /// Example had `# expect: skip`; no compilation attempted.
    Skipped,
}

/// Extract every `corvid` fenced block from every `.md` file under
/// `dir`. Files are visited alphabetically; blocks are ordered by
/// their position within the file.
pub fn extract_spec_examples(dir: &Path) -> Result<Vec<SpecExample>> {
    let mut out = Vec::new();
    let mut md_files = collect_markdown_files(dir)?;
    md_files.sort();
    for file in md_files {
        let text = fs::read_to_string(&file)
            .with_context(|| format!("cannot read `{}`", file.display()))?;
        out.extend(extract_from_markdown(&file, &text));
    }
    Ok(out)
}

/// Compile every non-skipped example under `dir` and return one
/// `SpecVerdict` per example. Order matches `extract_spec_examples`.
pub fn verify_spec_examples(dir: &Path) -> Result<Vec<SpecVerdict>> {
    let examples = extract_spec_examples(dir)?;
    let mut out = Vec::with_capacity(examples.len());
    for example in examples {
        out.push(verify_one(example));
    }
    Ok(out)
}

/// Render a human-readable report over a set of verdicts.
pub fn render_spec_report(verdicts: &[SpecVerdict]) -> String {
    let mut out = String::new();
    let mut pass = 0;
    let mut fail = 0;
    let mut skip = 0;
    for v in verdicts {
        match &v.kind {
            VerdictKind::Pass => pass += 1,
            VerdictKind::Skipped => skip += 1,
            VerdictKind::Fail { reason } => {
                fail += 1;
                out.push_str(&format!(
                    "  FAIL  {}:{} (block {}) — {reason}\n",
                    v.example.file.display(),
                    v.example.line,
                    v.example.block_index + 1,
                ));
            }
        }
    }
    out.push_str(&format!(
        "\n{pass} passed, {fail} failed, {skip} skipped out of {} examples.\n",
        verdicts.len()
    ));
    out
}

fn verify_one(example: SpecExample) -> SpecVerdict {
    if matches!(example.expectation, Expectation::Skip) {
        return SpecVerdict {
            example,
            kind: VerdictKind::Skipped,
        };
    }
    let result = compile(&example.source);
    let kind = compare(&example.expectation, &result);
    SpecVerdict { example, kind }
}

fn compare(expectation: &Expectation, result: &CompileResult) -> VerdictKind {
    let error_messages: Vec<&str> = result
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    match expectation {
        Expectation::Compile => {
            if result.ok() {
                VerdictKind::Pass
            } else {
                VerdictKind::Fail {
                    reason: format!(
                        "spec expected `compile`, but compiler reported {} error(s): {}",
                        result.diagnostics.len(),
                        short_diag_list(&error_messages),
                    ),
                }
            }
        }
        Expectation::ErrorAny => {
            if result.diagnostics.is_empty() {
                VerdictKind::Fail {
                    reason: "spec expected `error`, but program compiled cleanly".into(),
                }
            } else {
                VerdictKind::Pass
            }
        }
        Expectation::ErrorContains(pattern) => {
            if error_messages.iter().any(|m| m.contains(pattern.as_str())) {
                VerdictKind::Pass
            } else if result.diagnostics.is_empty() {
                VerdictKind::Fail {
                    reason: format!(
                        "spec expected error containing `{pattern}`, but program compiled cleanly"
                    ),
                }
            } else {
                VerdictKind::Fail {
                    reason: format!(
                        "spec expected error containing `{pattern}`, got: {}",
                        short_diag_list(&error_messages),
                    ),
                }
            }
        }
        Expectation::Skip => unreachable!("Skip handled in verify_one"),
    }
}

fn short_diag_list(messages: &[&str]) -> String {
    let snippets: Vec<String> = messages
        .iter()
        .take(3)
        .map(|m| format!("`{}`", truncate(m, 80)))
        .collect();
    if messages.len() > 3 {
        format!("{}, … ({} more)", snippets.join(", "), messages.len() - 3)
    } else {
        snippets.join(", ")
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

fn collect_markdown_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)
        .with_context(|| format!("cannot read directory `{}`", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(path);
        }
    }
    Ok(out)
}

/// Parse a markdown file's ```corvid``` fenced blocks.
fn extract_from_markdown(file: &Path, text: &str) -> Vec<SpecExample> {
    let mut out = Vec::new();
    let mut in_block = false;
    let mut block_lines: Vec<String> = Vec::new();
    let mut block_start_line = 0usize;
    let mut block_index = 0usize;

    for (idx, line) in text.lines().enumerate() {
        let lineno = idx + 1;
        let trimmed = line.trim_end();
        if !in_block {
            if trimmed == "```corvid" {
                in_block = true;
                block_start_line = lineno;
                block_lines.clear();
            }
        } else if trimmed == "```" {
            let example = build_example(file, block_start_line, block_index, &block_lines);
            out.push(example);
            block_index += 1;
            in_block = false;
            block_lines.clear();
        } else {
            block_lines.push(line.to_string());
        }
    }

    out
}

fn build_example(
    file: &Path,
    line: usize,
    block_index: usize,
    block_lines: &[String],
) -> SpecExample {
    let (expectation, source_lines) = parse_expectation(block_lines);
    SpecExample {
        file: file.to_path_buf(),
        line,
        block_index,
        source: source_lines.join("\n"),
        expectation,
    }
}

/// Pull an `# expect: ...` directive off the top of the block, if
/// present. Returns the expectation plus the remaining source lines
/// (with the directive stripped so the compiler sees only real code).
fn parse_expectation(block_lines: &[String]) -> (Expectation, Vec<String>) {
    let mut iter = block_lines.iter();
    let mut consumed = 0;

    // Skip leading blank lines — users may want a visual break between
    // the directive and the code. But only blank lines: the directive
    // must appear on the first non-blank line to count.
    while let Some(line) = iter.clone().next() {
        if line.trim().is_empty() {
            iter.next();
            consumed += 1;
        } else {
            break;
        }
    }

    let expectation = if let Some(first) = iter.clone().next() {
        let trimmed = first.trim_start();
        if let Some(rest) = trimmed.strip_prefix("# expect:") {
            let rest = rest.trim();
            iter.next();
            consumed += 1;
            parse_directive(rest)
        } else {
            Expectation::Compile
        }
    } else {
        Expectation::Compile
    };

    let remaining: Vec<String> = block_lines.iter().skip(consumed).cloned().collect();
    (expectation, remaining)
}

fn parse_directive(rest: &str) -> Expectation {
    if rest == "compile" {
        return Expectation::Compile;
    }
    if rest == "skip" {
        return Expectation::Skip;
    }
    if rest == "error" {
        return Expectation::ErrorAny;
    }
    if let Some(pattern) = rest.strip_prefix("error ") {
        let pattern = pattern.trim();
        // Accept `"..."` or bare substring.
        let pattern = pattern.trim_matches('"').to_string();
        return Expectation::ErrorContains(pattern);
    }
    // Unknown directive — fall back to Compile so a typo surfaces as a
    // real compile failure the user has to resolve.
    Expectation::Compile
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_temp_file(content: &str) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("sample.md");
        std::fs::write(&path, content).unwrap();
        (tmp, path)
    }

    #[test]
    fn extracts_single_fenced_block_with_default_compile_expectation() {
        let md = "\
# Title

some prose.

```corvid
effect noop:
    cost: $0.00
```

more prose.
";
        let (_tmp, path) = make_temp_file(md);
        let examples = extract_from_markdown(&path, md);
        assert_eq!(examples.len(), 1);
        let example = &examples[0];
        assert_eq!(example.expectation, Expectation::Compile);
        assert!(example.source.contains("effect noop:"));
        assert_eq!(example.line, 5);
        assert_eq!(example.block_index, 0);
    }

    #[test]
    fn extracts_expect_error_directive() {
        let md = "\
```corvid
# expect: error
this is not valid corvid
```
";
        let (_tmp, path) = make_temp_file(md);
        let examples = extract_from_markdown(&path, md);
        assert_eq!(examples[0].expectation, Expectation::ErrorAny);
        // Directive line is stripped from source.
        assert!(!examples[0].source.contains("# expect"));
    }

    #[test]
    fn extracts_expect_error_with_pattern() {
        let md = r#"
```corvid
# expect: error "unapproved dangerous call"
agent bad() -> String:
    return foo()
```
"#;
        let (_tmp, path) = make_temp_file(md);
        let examples = extract_from_markdown(&path, md);
        match &examples[0].expectation {
            Expectation::ErrorContains(p) => assert_eq!(p, "unapproved dangerous call"),
            other => panic!("expected ErrorContains, got {other:?}"),
        }
    }

    #[test]
    fn extracts_expect_skip_directive() {
        let md = "\
```corvid
# expect: skip
pseudo-code that won't compile
```
";
        let (_tmp, path) = make_temp_file(md);
        let examples = extract_from_markdown(&path, md);
        assert_eq!(examples[0].expectation, Expectation::Skip);
    }

    #[test]
    fn extracts_multiple_blocks_with_independent_expectations() {
        let md = "\
```corvid
# expect: compile
effect a:
    cost: $0.01
```

```corvid
# expect: skip
illustrative
```
";
        let (_tmp, path) = make_temp_file(md);
        let examples = extract_from_markdown(&path, md);
        assert_eq!(examples.len(), 2);
        assert_eq!(examples[0].expectation, Expectation::Compile);
        assert_eq!(examples[1].expectation, Expectation::Skip);
        // Block indices are stable.
        assert_eq!(examples[0].block_index, 0);
        assert_eq!(examples[1].block_index, 1);
    }

    #[test]
    fn ignores_non_corvid_fences() {
        let md = "\
```rust
fn main() {}
```

```corvid
effect noop:
    cost: $0.0
```

```json
{\"a\": 1}
```
";
        let (_tmp, path) = make_temp_file(md);
        let examples = extract_from_markdown(&path, md);
        assert_eq!(examples.len(), 1, "only the corvid block should be extracted");
    }

    #[test]
    fn ignores_blank_lines_before_directive() {
        let md = "\
```corvid

# expect: skip
content
```
";
        let (_tmp, path) = make_temp_file(md);
        let examples = extract_from_markdown(&path, md);
        assert_eq!(examples[0].expectation, Expectation::Skip);
    }

    #[test]
    fn unknown_directive_falls_back_to_compile() {
        let md = "\
```corvid
# expect: bogus
effect foo:
    cost: $0.00
```
";
        let (_tmp, path) = make_temp_file(md);
        let examples = extract_from_markdown(&path, md);
        assert_eq!(examples[0].expectation, Expectation::Compile);
    }

    #[test]
    fn verify_one_passes_on_compile_expectation_with_clean_program() {
        let example = SpecExample {
            file: PathBuf::from("test.md"),
            line: 1,
            block_index: 0,
            source: "tool ping(id: String) -> String\n\nagent run(id: String) -> String:\n    return ping(id)\n".into(),
            expectation: Expectation::Compile,
        };
        let verdict = verify_one(example);
        assert!(matches!(verdict.kind, VerdictKind::Pass), "got {:?}", verdict.kind);
    }

    #[test]
    fn verify_one_fails_when_compile_expected_but_errors_present() {
        let example = SpecExample {
            file: PathBuf::from("test.md"),
            line: 1,
            block_index: 0,
            source: "agent broken() -> String:\n    return unknown_callee()\n".into(),
            expectation: Expectation::Compile,
        };
        let verdict = verify_one(example);
        assert!(matches!(verdict.kind, VerdictKind::Fail { .. }));
    }

    #[test]
    fn verify_one_passes_when_error_expected_and_error_present() {
        let example = SpecExample {
            file: PathBuf::from("test.md"),
            line: 1,
            block_index: 0,
            source: "agent broken() -> String:\n    return undefined_thing()\n".into(),
            expectation: Expectation::ErrorAny,
        };
        let verdict = verify_one(example);
        assert!(matches!(verdict.kind, VerdictKind::Pass));
    }

    #[test]
    fn verify_one_skipped_never_compiles() {
        let example = SpecExample {
            file: PathBuf::from("test.md"),
            line: 1,
            block_index: 0,
            source: "intentionally not corvid at all\n".into(),
            expectation: Expectation::Skip,
        };
        let verdict = verify_one(example);
        assert!(matches!(verdict.kind, VerdictKind::Skipped));
    }
}
