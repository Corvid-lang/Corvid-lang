use anyhow::{anyhow, bail, Result};
use corvid_ast::{BackpressurePolicy, Decl, DimensionValue};
use corvid_resolve::resolve;
use corvid_syntax::{lex, parse_file};
use corvid_types::{analyze_effects, EffectRegistry};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CanonicalSummary {
    agent_name: String,
    declared_effects: Vec<String>,
    inferred_effects: Vec<String>,
    composed_effects: Vec<String>,
    dimensions: BTreeMap<String, String>,
    violations: Vec<String>,
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

fn load_clean_corpus_programs() -> Result<Vec<(String, String)>> {
    let mut files = Vec::new();
    collect_corpus_files(&workspace_root().join("tests/corpus"), &mut files)?;
    files.retain(|path| !path.components().any(|component| component.as_os_str() == "should_fail"));
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let source = std::fs::read_to_string(&path)
                .map_err(|err| anyhow!("failed to read `{}`: {err}", path.display()))?;
            Ok((
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("<unknown>")
                    .to_string(),
                source,
            ))
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
    use crate::rewrite::{apply_rewrite, rewrite_rules, RewriteRule};

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
            let rewritten = apply_rewrite(original, RewriteRule::AlphaConversion).expect("alpha-conversion");
            prop_assert!(rewritten.changed, "alpha-conversion should change the fixture");
            prop_assert!(assert_effect_equivalence(original, &rewritten.source).is_ok());
        }

        #[test]
        fn let_extract_preserves_effect_summaries(
            fixture_index in 0usize..extract_fixtures().len(),
            _corpus_index in corpus_index(),
        ) {
            let original = extract_fixtures()[fixture_index];
            let rewritten = apply_rewrite(original, RewriteRule::LetExtract).expect("let-extract");
            prop_assert!(rewritten.changed, "let-extract should change the fixture");
            prop_assert!(assert_effect_equivalence(original, &rewritten.source).is_ok());
        }

        #[test]
        fn let_inline_preserves_effect_summaries(
            fixture_index in 0usize..inline_fixtures().len(),
            _corpus_index in corpus_index(),
        ) {
            let original = inline_fixtures()[fixture_index];
            let rewritten = apply_rewrite(original, RewriteRule::LetInline).expect("let-inline");
            prop_assert!(rewritten.changed, "let-inline should change the fixture");
            prop_assert!(assert_effect_equivalence(original, &rewritten.source).is_ok());
        }

        #[test]
        fn commutative_sibling_swap_preserves_effect_summaries(
            fixture_index in 0usize..swap_fixtures().len(),
            _corpus_index in corpus_index(),
        ) {
            let original = swap_fixtures()[fixture_index];
            let rewritten = apply_rewrite(original, RewriteRule::CommutativeSiblingSwap)
                .expect("commutative-sibling-swap");
            prop_assert!(rewritten.changed, "commutative-sibling-swap should change the fixture");
            prop_assert!(assert_effect_equivalence(original, &rewritten.source).is_ok());
        }

        #[test]
        fn top_level_reorder_preserves_effect_summaries(
            fixture_index in 0usize..reorder_fixtures().len(),
            _corpus_index in corpus_index(),
        ) {
            let original = reorder_fixtures()[fixture_index];
            let rewritten = apply_rewrite(original, RewriteRule::TopLevelReorder)
                .expect("top-level-reorder");
            prop_assert!(rewritten.changed, "top-level-reorder should change the fixture");
            prop_assert!(assert_effect_equivalence(original, &rewritten.source).is_ok());
        }

        #[test]
        fn if_branch_swap_preserves_effect_summaries(
            fixture_index in 0usize..branch_swap_fixtures().len(),
            _corpus_index in corpus_index(),
        ) {
            let original = branch_swap_fixtures()[fixture_index];
            let rewritten = apply_rewrite(original, RewriteRule::IfBranchSwap)
                .expect("if-branch-swap");
            prop_assert!(rewritten.changed, "if-branch-swap should change the fixture");
            prop_assert!(assert_effect_equivalence(original, &rewritten.source).is_ok());
        }

        #[test]
        fn constant_folding_preserves_effect_summaries(
            fixture_index in 0usize..constant_fold_fixtures().len(),
            _corpus_index in corpus_index(),
        ) {
            let original = constant_fold_fixtures()[fixture_index];
            let rewritten = apply_rewrite(original, RewriteRule::ConstantFolding)
                .expect("constant-folding");
            prop_assert!(rewritten.changed, "constant-folding should change the fixture");
            prop_assert!(assert_effect_equivalence(original, &rewritten.source).is_ok());
        }
    }

    #[test]
    fn slice_a_rule_set_is_limited_to_first_three_rewrites() {
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
}
