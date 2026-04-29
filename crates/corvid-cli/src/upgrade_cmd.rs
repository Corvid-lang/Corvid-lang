use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

#[derive(Debug, Serialize)]
struct UpgradeFinding {
    path: String,
    rule_id: &'static str,
    kind: &'static str,
    message: &'static str,
    replacement: &'static str,
    occurrences: usize,
}

struct RewriteRule {
    id: &'static str,
    kind: &'static str,
    from: &'static str,
    to: &'static str,
    message: &'static str,
}

const RULES: &[RewriteRule] = &[
    RewriteRule {
        id: "syntax.pub_extern_agent_single_line",
        kind: "syntax",
        from: "pub extern \"c\"\nagent ",
        to: "pub extern \"c\" agent ",
        message: "`pub extern \"c\" agent` is the stable v1 spelling; split-line legacy form is migrated automatically",
    },
    RewriteRule {
        id: "stdlib.llm_complete_to_agent_run",
        kind: "stdlib",
        from: "std.llm.complete(",
        to: "std.agent.run(",
        message: "`std.llm.complete` is replaced by the policy-aware `std.agent.run` entrypoint",
    },
    RewriteRule {
        id: "stdlib.cache_get_or_create_to_remember",
        kind: "stdlib",
        from: "std.cache.get_or_create(",
        to: "std.cache.remember(",
        message: "`std.cache.get_or_create` is replaced by `std.cache.remember` with the same key/value contract",
    },
];

pub fn run_check(root: &Path, json: bool) -> Result<u8> {
    let findings = collect_findings(root)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&findings).context("serialize upgrade findings")?
        );
    } else {
        render_findings(&findings);
    }
    Ok(if findings.is_empty() { 0 } else { 1 })
}

pub fn run_apply(root: &Path, json: bool) -> Result<u8> {
    let mut findings = Vec::new();
    for path in corvid_sources(root)? {
        let original =
            fs::read_to_string(&path).with_context(|| format!("read `{}`", path.display()))?;
        let mut rewritten = original.clone();
        for rule in RULES {
            let occurrences = rewritten.matches(rule.from).count();
            if occurrences == 0 {
                continue;
            }
            findings.push(finding_for(&path, rule, occurrences));
            rewritten = rewritten.replace(rule.from, rule.to);
        }
        if rewritten != original {
            fs::write(&path, rewritten).with_context(|| format!("write `{}`", path.display()))?;
        }
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&findings).context("serialize applied findings")?
        );
    } else {
        render_findings(&findings);
    }
    Ok(0)
}

fn collect_findings(root: &Path) -> Result<Vec<UpgradeFinding>> {
    let mut findings = Vec::new();
    for path in corvid_sources(root)? {
        let source =
            fs::read_to_string(&path).with_context(|| format!("read `{}`", path.display()))?;
        for rule in RULES {
            let occurrences = source.matches(rule.from).count();
            if occurrences > 0 {
                findings.push(finding_for(&path, rule, occurrences));
            }
        }
    }
    Ok(findings)
}

fn corvid_sources(root: &Path) -> Result<Vec<PathBuf>> {
    if root.is_file() {
        return Ok(vec![root.to_path_buf()]);
    }

    let mut files = Vec::new();
    collect_corvid_sources(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_corvid_sources(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read dir `{}`", dir.display()))? {
        let entry = entry.with_context(|| format!("read dir entry in `{}`", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_corvid_sources(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "cor") {
            files.push(path);
        }
    }
    Ok(())
}

fn finding_for(path: &Path, rule: &RewriteRule, occurrences: usize) -> UpgradeFinding {
    UpgradeFinding {
        path: path.display().to_string(),
        rule_id: rule.id,
        kind: rule.kind,
        message: rule.message,
        replacement: rule.to,
        occurrences,
    }
}

fn render_findings(findings: &[UpgradeFinding]) {
    println!("corvid upgrade report");
    println!("finding_count: {}", findings.len());
    for finding in findings {
        println!(
            "{} [{}] {} occurrences={} replacement={}",
            finding.path,
            finding.kind,
            finding.rule_id,
            finding.occurrences,
            finding.replacement
        );
        println!("  {}", finding.message);
    }
}
