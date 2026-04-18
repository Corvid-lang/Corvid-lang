use corvid_differential_verify::fuzz::{assert_effect_equivalence, clean_corpus_programs};
use corvid_differential_verify::rewrite::{apply_rewrite, parse_source, render_file, RewriteRule};

#[test]
fn clean_corpus_round_trips_through_ast_printer() {
    for (name, source) in clean_corpus_programs() {
        let file = parse_source(source).unwrap_or_else(|err| panic!("parse `{name}`: {err}"));
        let rendered = render_file(&file);
        parse_source(&rendered).unwrap_or_else(|err| panic!("reparse `{name}`: {err}"));
        assert_effect_equivalence(source, &rendered)
            .unwrap_or_else(|err| panic!("effect round-trip `{name}`: {err}"));
    }
}

#[test]
fn alpha_conversion_is_structural_and_preserves_effects() {
    let source = r#"
agent main() -> Int:
    value = 1
    total = value + 2
    return total
"#;
    let rewritten = apply_rewrite(source, RewriteRule::AlphaConversion).expect("alpha-convert");
    assert!(rewritten.changed, "alpha-conversion should rename a local");
    assert_ne!(source.trim(), rewritten.source.trim(), "source should change");
    assert_effect_equivalence(source, &rewritten.source).expect("effect equivalence");
}

#[test]
fn let_extract_and_inline_round_trip_a_pure_expression() {
    let source = r#"
agent main() -> Int:
    return 1 + 2
"#;
    let extracted = apply_rewrite(source, RewriteRule::LetExtract).expect("let-extract");
    assert!(extracted.changed, "let-extract should introduce a binder");
    assert_effect_equivalence(source, &extracted.source).expect("effect equivalence after extract");

    let inlined = apply_rewrite(&extracted.source, RewriteRule::LetInline).expect("let-inline");
    assert!(inlined.changed, "let-inline should eliminate the binder");
    assert_effect_equivalence(&extracted.source, &inlined.source)
        .expect("effect equivalence after inline");
}
