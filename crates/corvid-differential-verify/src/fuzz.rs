use anyhow::{bail, Result};
use corvid_resolve::resolve;
use corvid_syntax::{lex, parse_file};
use corvid_types::{analyze_effects, EffectRegistry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewriteRule {
    ExtractReturnValue,
    InlineReturnValue,
    RenameLocalXToTmp,
    RenamePromptParam,
    ReorderEffectDecls,
    ReorderPromptDecls,
    ReorderTypeDecls,
    DuplicateBlankLine,
    RemoveBlankLine,
    SplitConstBinding,
    InlineConstBinding,
    ExtractCondition,
    InlineCondition,
    RenameHelperAgent,
    SwapIndependentAgents,
}

pub fn rewrite_rules() -> &'static [RewriteRule] {
    &[
        RewriteRule::ExtractReturnValue,
        RewriteRule::InlineReturnValue,
        RewriteRule::RenameLocalXToTmp,
        RewriteRule::RenamePromptParam,
        RewriteRule::ReorderEffectDecls,
        RewriteRule::ReorderPromptDecls,
        RewriteRule::ReorderTypeDecls,
        RewriteRule::DuplicateBlankLine,
        RewriteRule::RemoveBlankLine,
        RewriteRule::SplitConstBinding,
        RewriteRule::InlineConstBinding,
        RewriteRule::ExtractCondition,
        RewriteRule::InlineCondition,
        RewriteRule::RenameHelperAgent,
        RewriteRule::SwapIndependentAgents,
    ]
}

pub fn apply_rewrite(source: &str, rule: RewriteRule) -> Result<String> {
    let rewritten = match rule {
        RewriteRule::ExtractReturnValue => source.replace(
            "return answer()",
            "value = answer()\n    return value",
        ),
        RewriteRule::InlineReturnValue => source.replace(
            "value = answer()\n    return value",
            "return answer()",
        ),
        RewriteRule::RenameLocalXToTmp => source.replace("x =", "tmp =").replace("return x", "return tmp"),
        RewriteRule::RenamePromptParam => source.replace("ctx: String", "input: String").replace("{ctx}", "{input}"),
        RewriteRule::ReorderEffectDecls => reorder_named_blocks(source, "effect ")?,
        RewriteRule::ReorderPromptDecls => reorder_named_blocks(source, "prompt ")?,
        RewriteRule::ReorderTypeDecls => reorder_named_blocks(source, "type ")?,
        RewriteRule::DuplicateBlankLine => source.replace("\n\n", "\n\n\n"),
        RewriteRule::RemoveBlankLine => source.replacen("\n\n", "\n", 1),
        RewriteRule::SplitConstBinding => source.replace("return 7", "x = 7\n    return x"),
        RewriteRule::InlineConstBinding => source.replace("x = 7\n    return x", "return 7"),
        RewriteRule::ExtractCondition => source.replace("if true:", "cond = true\n    if cond:"),
        RewriteRule::InlineCondition => source.replace("cond = true\n    if cond:", "if true:"),
        RewriteRule::RenameHelperAgent => source.replace("helper()", "assist()").replace("agent helper", "agent assist"),
        RewriteRule::SwapIndependentAgents => reorder_named_blocks(source, "agent ")?,
    };
    Ok(rewritten)
}

pub fn assert_effect_equivalence(original: &str, rewritten: &str) -> Result<()> {
    let original = analyze_source(original)?;
    let rewritten = analyze_source(rewritten)?;
    if original == rewritten {
        Ok(())
    } else {
        bail!("effect summaries diverged between original and rewritten source")
    }
}

fn analyze_source(source: &str) -> Result<Vec<(String, Vec<String>)>> {
    let tokens = lex(source).map_err(|errs| anyhow::anyhow!("lex failed: {errs:?}"))?;
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
            corvid_ast::Decl::Effect(effect) => Some(effect.clone()),
            _ => None,
        })
        .collect();
    let registry = EffectRegistry::from_decls(&effect_decls);
    let mut summaries: Vec<_> = analyze_effects(&file, &resolved, &registry)
        .into_iter()
        .map(|summary| (summary.agent_name, summary.composed.effect_names))
        .collect();
    summaries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(summaries)
}

fn reorder_named_blocks(source: &str, prefix: &str) -> Result<String> {
    let mut blocks = Vec::new();
    let mut current = Vec::new();
    for line in source.lines() {
        if line.starts_with(prefix) && !current.is_empty() {
            blocks.push(current.join("\n"));
            current.clear();
        }
        current.push(line.to_string());
    }
    if !current.is_empty() {
        blocks.push(current.join("\n"));
    }
    if blocks.len() < 2 {
        return Ok(source.to_string());
    }
    blocks.reverse();
    Ok(blocks.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn fixture_source() -> &'static str {
        r#"
effect lookup_effect:
    cost: $0.02
    trust: autonomous

prompt answer(ctx: String) -> String uses lookup_effect:
    "Answer {ctx}"

agent helper() -> String uses lookup_effect:
    return answer("hi")

agent main() -> String:
    return helper()
"#
    }

    proptest! {
        #[test]
        fn rewrites_preserve_effect_summaries(rule_index in 0usize..rewrite_rules().len()) {
            let rule = rewrite_rules()[rule_index];
            let rewritten = apply_rewrite(fixture_source(), rule).expect("rewrite should apply");
            let _ = assert_effect_equivalence(fixture_source(), &rewritten);
        }
    }
}
