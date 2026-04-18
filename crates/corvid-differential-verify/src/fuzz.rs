use anyhow::{anyhow, bail, Result};
use corvid_ast::{BackpressurePolicy, Decl, DimensionValue};
use corvid_resolve::resolve;
use corvid_syntax::{lex, parse_file};
use corvid_types::{analyze_effects, EffectRegistry};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::rewrite::{apply_rewrite, rewrite_rules, rule_name, RewriteResult, RewriteRule};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CanonicalSummary {
    pub agent_name: String,
    pub declared_effects: Vec<String>,
    pub inferred_effects: Vec<String>,
    pub composed_effects: Vec<String>,
    pub dimensions: BTreeMap<String, String>,
    pub violations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RewriteDivergenceReport {
    pub path: PathBuf,
    pub rule: String,
    pub law: String,
    pub rationale: String,
    pub line: Option<usize>,
    pub original_profile: Vec<CanonicalSummary>,
    pub rewritten_profile: Vec<CanonicalSummary>,
    pub shrunk_reproducer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoverageMatrix {
    pub programs: Vec<String>,
    pub rows: Vec<CoverageRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoverageRow {
    pub rule: String,
    pub law: String,
    pub counts: Vec<u64>,
}

pub fn assert_effect_equivalence(original: &str, rewritten: &str) -> Result<()> {
    let original = analyze_source(original)?;
    let rewritten = analyze_source(rewritten)?;
    if original == rewritten {
        Ok(())
    } else {
        bail!(
            "effect summaries diverged between original and rewritten source:\noriginal={original:#?}\nrewritten={rewritten:#?}"
        )
    }
}

pub fn assert_rewrite_equivalence(
    path: &Path,
    original: &str,
    rewritten: &RewriteResult,
) -> Result<()> {
    if let Some(report) = build_rewrite_divergence_report(path, original, rewritten)? {
        bail!("{}", render_rewrite_divergence_report(&report));
    }
    Ok(())
}

pub fn build_rewrite_divergence_report(
    path: &Path,
    original: &str,
    rewritten: &RewriteResult,
) -> Result<Option<RewriteDivergenceReport>> {
    let Some((original_profile, rewritten_profile)) =
        effect_drift(original, &rewritten.source)?
    else {
        return Ok(None);
    };

    Ok(Some(RewriteDivergenceReport {
        path: path.to_path_buf(),
        rule: rule_name(rewritten.rule).into(),
        law: rewritten.law.name.into(),
        rationale: rewritten.law.rationale.into(),
        line: first_changed_line(original, &rewritten.source),
        original_profile,
        rewritten_profile,
        shrunk_reproducer: shrink_rewrite_counterexample(path, original, rewritten.rule)?,
    }))
}

pub fn render_rewrite_divergence_report(report: &RewriteDivergenceReport) -> String {
    let mut lines = vec![format!(
        "{} broken at {}{}",
        report.law,
        report.path.display(),
        report
            .line
            .map(|line| format!(":{line}"))
            .unwrap_or_default()
    )];
    lines.push(format!("  rewrite: {}", report.rule));
    lines.push(format!("  rationale: {}", report.rationale));
    lines.push(format!(
        "  original profile: {}",
        serde_json::to_string_pretty(&report.original_profile).unwrap_or_else(|_| "[]".into())
    ));
    lines.push(format!(
        "  rewritten profile: {}",
        serde_json::to_string_pretty(&report.rewritten_profile).unwrap_or_else(|_| "[]".into())
    ));
    if let Some(reproducer) = &report.shrunk_reproducer {
        lines.push("  shrunk reproducer:".into());
        lines.extend(reproducer.lines().map(|line| format!("    {line}")));
    }
    lines.join("\n")
}

pub fn build_coverage_matrix() -> Result<CoverageMatrix> {
    let programs = clean_corpus_programs();
    let columns = programs.iter().map(|(name, _)| name.clone()).collect::<Vec<_>>();
    let mut rows = Vec::new();

    for &rule in rewrite_rules() {
        let mut counts = Vec::with_capacity(programs.len());
        for (name, source) in programs {
            let rewritten = apply_rewrite(source, rule)?;
            let passed = rewritten.changed
                && assert_rewrite_equivalence(Path::new(name), source, &rewritten).is_ok();
            counts.push(u64::from(passed));
        }
        rows.push(CoverageRow {
            rule: rule_name(rule).into(),
            law: crate::rewrite::law_ref(rule).name.into(),
            counts,
        });
    }

    Ok(CoverageMatrix { programs: columns, rows })
}

pub fn render_coverage_matrix(matrix: &CoverageMatrix) -> String {
    let mut lines = Vec::new();
    let mut header = format!("{:<30} {:<24}", "rewrite", "law");
    for program in &matrix.programs {
        header.push(' ');
        header.push_str(&format!("{:<18}", truncate_cell(program)));
    }
    lines.push(header);
    lines.push("-".repeat(lines[0].len()));

    for row in &matrix.rows {
        let mut line = format!("{:<30} {:<24}", row.rule, row.law);
        for count in &row.counts {
            line.push(' ');
            line.push_str(&format!("{:<18}", count));
        }
        lines.push(line);
    }

    let dead_rows = matrix
        .rows
        .iter()
        .filter(|row| row.counts.iter().all(|count| *count == 0))
        .map(|row| row.rule.as_str())
        .collect::<Vec<_>>();
    if !dead_rows.is_empty() {
        lines.push(String::new());
        lines.push(format!(
            "unexercised rewrites: {}",
            dead_rows.join(", ")
        ));
    }

    lines.join("\n")
}

pub fn render_coverage_matrix_json(matrix: &CoverageMatrix) -> Result<String> {
    serde_json::to_string_pretty(matrix).map_err(Into::into)
}

pub fn emit_coverage_matrix(matrix: &CoverageMatrix, json: bool) -> Result<()> {
    println!("{}", render_coverage_matrix(matrix));
    if json {
        eprintln!("{}", render_coverage_matrix_json(matrix)?);
    }
    Ok(())
}

pub fn clean_corpus_programs() -> &'static [(String, String)] {
    static PROGRAMS: OnceLock<Vec<(String, String)>> = OnceLock::new();
    PROGRAMS.get_or_init(|| load_clean_corpus_programs().expect("load clean corpus fixtures"))
}

fn analyze_source(source: &str) -> Result<Vec<CanonicalSummary>> {
    let tokens = lex(source).map_err(|errs| anyhow!("lex failed: {errs:?}"))?;
    let (file, parse_errors) = parse_file(&tokens);
    if !parse_errors.is_empty() {
        bail!("parse failed: {parse_errors:?}");
    }
    let resolved = resolve(&file);
    if !resolved.errors.is_empty() {
        bail!("resolve failed: {:?}", resolved.errors);
    }
    let effect_decls: Vec<_> = file
        .decls
        .iter()
        .filter_map(|decl| match decl {
            Decl::Effect(effect) => Some(effect.clone()),
            _ => None,
        })
        .collect();
    let registry = EffectRegistry::from_decls(&effect_decls);
    let mut summaries: Vec<_> = analyze_effects(&file, &resolved, &registry)
        .into_iter()
        .map(|summary| CanonicalSummary {
            agent_name: summary.agent_name,
            declared_effects: summary.declared_effects,
            inferred_effects: summary.inferred_effects,
            composed_effects: summary.composed.effect_names,
            dimensions: summary
                .composed
                .dimensions
                .into_iter()
                .map(|(name, value)| (name, render_dimension_value(&value)))
                .collect(),
            violations: summary
                .violations
                .into_iter()
                .map(|violation| violation.to_string())
                .collect(),
        })
        .collect();
    summaries.sort_by(|left, right| left.agent_name.cmp(&right.agent_name));
    Ok(summaries)
}

fn effect_drift(
    original: &str,
    rewritten: &str,
) -> Result<Option<(Vec<CanonicalSummary>, Vec<CanonicalSummary>)>> {
    let original_profile = analyze_source(original)?;
    let rewritten_profile = analyze_source(rewritten)?;
    if original_profile == rewritten_profile {
        Ok(None)
    } else {
        Ok(Some((original_profile, rewritten_profile)))
    }
}

fn shrink_rewrite_counterexample(
    path: &Path,
    source: &str,
    rule: RewriteRule,
) -> Result<Option<String>> {
    let shrunk = shrink_source(source, |candidate| rewrite_still_diverges(path, candidate, rule))?;
    if normalize_lines(source) == normalize_lines(&shrunk) {
        Ok(None)
    } else {
        Ok(Some(shrunk))
    }
}

fn rewrite_still_diverges(path: &Path, source: &str, rule: RewriteRule) -> Result<bool> {
    let Ok(rewritten) = apply_rewrite(source, rule) else {
        return Ok(false);
    };
    if !rewritten.changed {
        return Ok(false);
    }
    let Ok(drift) = effect_drift(source, &rewritten.source) else {
        return Ok(false);
    };
    Ok(drift.is_some() && !path.as_os_str().is_empty())
}

fn shrink_source(
    source: &str,
    mut still_fails: impl FnMut(&str) -> Result<bool>,
) -> Result<String> {
    let mut lines: Vec<String> = source.lines().map(|line| line.to_string()).collect();
    let mut changed = true;
    while changed {
        changed = false;
        for index in 0..lines.len() {
            let line = lines[index].trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut candidate = lines.clone();
            candidate.remove(index);
            let candidate_source = candidate.join("\n");
            if still_fails(&candidate_source)? {
                lines = candidate;
                changed = true;
                break;
            }
        }
    }
    Ok(lines.join("\n"))
}

fn first_changed_line(original: &str, rewritten: &str) -> Option<usize> {
    let original_lines: Vec<_> = original.lines().collect();
    let rewritten_lines: Vec<_> = rewritten.lines().collect();
    let max = original_lines.len().max(rewritten_lines.len());
    for index in 0..max {
        if original_lines.get(index) != rewritten_lines.get(index) {
            return Some(index + 1);
        }
    }
    None
}

fn normalize_lines(source: &str) -> String {
    source.lines().collect::<Vec<_>>().join("\n")
}

fn load_clean_corpus_programs() -> Result<Vec<(String, String)>> {
    let root = workspace_root();
    let mut files = Vec::new();
    collect_corpus_files(&root.join("tests/corpus"), &mut files)?;
    files.retain(|path| !path.components().any(|component| component.as_os_str() == "should_fail"));
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let source = std::fs::read_to_string(&path)
                .map_err(|err| anyhow!("failed to read `{}`: {err}", path.display()))?;
            let label = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .display()
                .to_string();
            Ok((label, source))
        })
        .collect()
}

fn collect_corpus_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(root)
        .map_err(|err| anyhow!("failed to read corpus directory `{}`: {err}", root.display()))?
    {
        let entry = entry.map_err(|err| anyhow!("failed to read corpus entry: {err}"))?;
        let path = entry.path();
        if path.is_dir() {
            collect_corpus_files(&path, files)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("cor") {
            files.push(path);
        }
    }
    Ok(())
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

fn truncate_cell(cell: &str) -> String {
    if cell.len() <= 18 {
        cell.to_string()
    } else {
        format!("{}...", &cell[..15])
    }
}

fn render_dimension_value(value: &DimensionValue) -> String {
    match value {
        DimensionValue::Bool(value) => value.to_string(),
        DimensionValue::Name(name) => name.clone(),
        DimensionValue::Cost(value) => format!("${value:.6}"),
        DimensionValue::Number(value) => format!("{value:.6}"),
        DimensionValue::Streaming { backpressure } => match backpressure {
            BackpressurePolicy::Bounded(size) => format!("streaming(bounded({size}))"),
            BackpressurePolicy::Unbounded => "streaming(unbounded)".into(),
        },
        DimensionValue::ConfidenceGated {
            threshold,
            above,
            below,
        } => format!("{above}_if_confident({threshold:.6},{below})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn alpha_fixtures() -> &'static [&'static str] {
        &[
            r#"
agent main() -> Int:
    value = 1
    total = value + 2
    return total
"#,
            r#"
agent main() -> String:
    text = "hi"
    if true:
        result = text
        return result
    return text
"#,
        ]
    }

    fn extract_fixtures() -> &'static [&'static str] {
        &[
            r#"
agent main() -> Int:
    return 1 + 2
"#,
            r#"
agent main() -> Int:
    total = (1 + 2)
    return total
"#,
            r#"
agent main() -> Bool:
    if (true and false):
        return true
    return false
"#,
        ]
    }

    fn inline_fixtures() -> &'static [&'static str] {
        &[
            r#"
agent main() -> Int:
    extracted = 1 + 2
    return extracted
"#,
            r#"
agent main() -> Bool:
    cond = true and false
    if cond:
        return true
    return false
"#,
            r#"
agent main() -> Int:
    seed = 7
    value = seed
    return value
"#,
        ]
    }

    fn swap_fixtures() -> &'static [&'static str] {
        &[
            r#"
agent main() -> Int:
    left = 1 + 2
    right = 3 + 4
    return left
"#,
            r#"
agent main() -> String:
    prefix = "a" + "b"
    suffix = "c" + "d"
    return prefix
"#,
        ]
    }

    fn reorder_fixtures() -> &'static [&'static str] {
        &[
            r#"
effect first_effect:
    cost: $0.01

effect second_effect:
    cost: $0.02

agent main() -> Int:
    return 7
"#,
            r#"
prompt alpha() -> String:
    "alpha"

prompt beta() -> String:
    "beta"

agent main() -> String:
    return alpha()
"#,
        ]
    }

    fn branch_swap_fixtures() -> &'static [&'static str] {
        &[
            r#"
agent main() -> Int:
    if true:
        return 1
    else:
        return 2
"#,
            r#"
agent main() -> String:
    if false:
        return "left"
    else:
        return "right"
"#,
        ]
    }

    fn constant_fold_fixtures() -> &'static [&'static str] {
        &[
            r#"
agent main() -> Int:
    return 1 + 2
"#,
            r#"
agent main() -> String:
    return "a" + "b"
"#,
            r#"
agent main() -> Int:
    total = 1 + 2
    return total
"#,
        ]
    }

    fn corpus_index() -> impl Strategy<Value = usize> {
        0usize..clean_corpus_programs().len()
    }

    fn assert_rule(
        path: PathBuf,
        original: &str,
        rule: RewriteRule,
    ) -> Result<RewriteResult> {
        let rewritten = apply_rewrite(original, rule)?;
        if !rewritten.changed {
            bail!("{} did not change {}", rule_name(rule), path.display());
        }
        assert_rewrite_equivalence(&path, original, &rewritten)?;
        Ok(rewritten)
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 10_000,
            .. ProptestConfig::default()
        })]

        #[test]
        fn alpha_conversion_preserves_effect_summaries(
            fixture_index in 0usize..alpha_fixtures().len(),
            _corpus_index in corpus_index(),
        ) {
            let original = alpha_fixtures()[fixture_index];
            let result = assert_rule(PathBuf::from(format!("generated/alpha_{fixture_index}.cor")), original, RewriteRule::AlphaConversion);
            prop_assert!(result.is_ok(), "{}", result.unwrap_err());
        }

        #[test]
        fn let_extract_preserves_effect_summaries(
            fixture_index in 0usize..extract_fixtures().len(),
            _corpus_index in corpus_index(),
        ) {
            let original = extract_fixtures()[fixture_index];
            let result = assert_rule(PathBuf::from(format!("generated/let_extract_{fixture_index}.cor")), original, RewriteRule::LetExtract);
            prop_assert!(result.is_ok(), "{}", result.unwrap_err());
        }

        #[test]
        fn let_inline_preserves_effect_summaries(
            fixture_index in 0usize..inline_fixtures().len(),
            _corpus_index in corpus_index(),
        ) {
            let original = inline_fixtures()[fixture_index];
            let result = assert_rule(PathBuf::from(format!("generated/let_inline_{fixture_index}.cor")), original, RewriteRule::LetInline);
            prop_assert!(result.is_ok(), "{}", result.unwrap_err());
        }

        #[test]
        fn commutative_sibling_swap_preserves_effect_summaries(
            fixture_index in 0usize..swap_fixtures().len(),
            _corpus_index in corpus_index(),
        ) {
            let original = swap_fixtures()[fixture_index];
            let result = assert_rule(PathBuf::from(format!("generated/commutative_swap_{fixture_index}.cor")), original, RewriteRule::CommutativeSiblingSwap);
            prop_assert!(result.is_ok(), "{}", result.unwrap_err());
        }

        #[test]
        fn top_level_reorder_preserves_effect_summaries(
            fixture_index in 0usize..reorder_fixtures().len(),
            _corpus_index in corpus_index(),
        ) {
            let original = reorder_fixtures()[fixture_index];
            let result = assert_rule(PathBuf::from(format!("generated/top_level_reorder_{fixture_index}.cor")), original, RewriteRule::TopLevelReorder);
            prop_assert!(result.is_ok(), "{}", result.unwrap_err());
        }

        #[test]
        fn if_branch_swap_preserves_effect_summaries(
            fixture_index in 0usize..branch_swap_fixtures().len(),
            _corpus_index in corpus_index(),
        ) {
            let original = branch_swap_fixtures()[fixture_index];
            let result = assert_rule(PathBuf::from(format!("generated/if_branch_swap_{fixture_index}.cor")), original, RewriteRule::IfBranchSwap);
            prop_assert!(result.is_ok(), "{}", result.unwrap_err());
        }

        #[test]
        fn constant_folding_preserves_effect_summaries(
            fixture_index in 0usize..constant_fold_fixtures().len(),
            _corpus_index in corpus_index(),
        ) {
            let original = constant_fold_fixtures()[fixture_index];
            let result = assert_rule(PathBuf::from(format!("generated/constant_folding_{fixture_index}.cor")), original, RewriteRule::ConstantFolding);
            prop_assert!(result.is_ok(), "{}", result.unwrap_err());
        }
    }

    #[test]
    fn rewrite_rule_set_includes_all_slice_b_rules() {
        assert_eq!(
            rewrite_rules(),
            &[
                RewriteRule::AlphaConversion,
                RewriteRule::LetExtract,
                RewriteRule::LetInline,
                RewriteRule::CommutativeSiblingSwap,
                RewriteRule::TopLevelReorder,
                RewriteRule::IfBranchSwap,
                RewriteRule::ConstantFolding,
            ]
        );
    }

    #[test]
    fn divergence_report_names_law_and_line() {
        let original = "agent main() -> Int:\n    return 1\n";
        let rewritten = RewriteResult {
            rule: RewriteRule::AlphaConversion,
            source: "effect external_call:\n    cost: $0.25\n\nprompt answer() -> Int uses external_call:\n    \"42\"\n\nagent main() -> Int:\n    return answer()\n".into(),
            changed: true,
            law: crate::rewrite::law_ref(RewriteRule::AlphaConversion),
        };

        let report = build_rewrite_divergence_report(
            Path::new("tests/corpus/generated/divergence.cor"),
            original,
            &rewritten,
        )
        .expect("build divergence report")
        .expect("report should exist");

        assert_eq!(report.law, "alpha-equivalence");
        assert_eq!(report.line, Some(1));
        assert!(render_rewrite_divergence_report(&report).contains("alpha-equivalence broken"));
    }

    #[test]
    fn shrinker_reuses_line_reduction_strategy() {
        let source = r#"
# comment
keep_one
drop_me
keep_two
"#;
        let shrunk = shrink_source(source, |candidate| {
            Ok(candidate.contains("keep_one") && candidate.contains("keep_two"))
        })
        .expect("shrink source");

        assert!(shrunk.contains("keep_one"));
        assert!(shrunk.contains("keep_two"));
        assert!(!shrunk.contains("drop_me"));
    }

    #[test]
    fn coverage_matrix_tracks_nontrivial_corpus_paths() {
        let matrix = build_coverage_matrix().expect("coverage matrix");
        assert_eq!(matrix.programs.len(), clean_corpus_programs().len());
        assert_eq!(matrix.rows.len(), rewrite_rules().len());
        assert!(matrix.rows.iter().any(|row| row.counts.iter().any(|count| *count > 0)));

        let rendered = render_coverage_matrix(&matrix);
        assert!(rendered.contains("alpha-conversion"));
        assert!(rendered.contains("law"));

        let json = render_coverage_matrix_json(&matrix).expect("coverage matrix json");
        assert!(json.contains("\"rows\""));

        emit_coverage_matrix(&matrix, std::env::var_os("CORVID_FUZZ_JSON").is_some())
            .expect("emit coverage matrix");
    }
}
