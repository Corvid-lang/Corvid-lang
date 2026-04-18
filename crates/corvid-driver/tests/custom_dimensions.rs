//! End-to-end tests for custom effect dimensions declared via
//! `corvid.toml`. Proves the driver discovers the config file via
//! upward-walking and the type checker sees the custom dimensions.

use std::fs;

use corvid_driver::{
    compile_with_config, load_corvid_config_for, render_law_check_report, run_law_checks,
    LawVerdict, DEFAULT_SAMPLES,
};
use tempfile::TempDir;

fn write_project(root: &std::path::Path, corvid_toml: &str, main_cor: &str) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("corvid.toml"), corvid_toml).unwrap();
    fs::write(root.join("src").join("main.cor"), main_cor).unwrap();
}

#[test]
fn walking_finds_corvid_toml_from_sibling_source_file() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_project(
        root,
        r#"
[effect-system.dimensions.freshness]
composition = "Max"
type = "timestamp"
default = "0"
semantics = "maximum data age in seconds"
"#,
        "agent noop() -> String:\n    return \"x\"\n",
    );

    let src_path = root.join("src").join("main.cor");
    let config = load_corvid_config_for(&src_path).expect("config should load");
    assert_eq!(config.effect_system.dimensions.len(), 1);
    assert!(config.effect_system.dimensions.contains_key("freshness"));
}

#[test]
fn custom_dimension_drives_end_to_end_compile() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_project(
        root,
        r#"
[effect-system.dimensions.freshness]
composition = "Max"
type = "number"
default = "0"
"#,
        "\
effect retrieve_doc:
    freshness: 3600

tool fetch(id: String) -> String uses retrieve_doc

agent lookup(id: String) -> String:
    result = fetch(id)
    return result
",
    );

    let src_path = root.join("src").join("main.cor");
    let source = fs::read_to_string(&src_path).unwrap();
    let config = load_corvid_config_for(&src_path);
    assert!(config.is_some(), "corvid.toml should have been discovered");

    let result = compile_with_config(&source, config.as_ref());
    assert!(
        result.ok(),
        "program using custom dimension should compile cleanly; got: {:?}",
        result.diagnostics
    );
}

#[test]
fn malformed_custom_dimension_surfaces_as_diagnostic() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_project(
        root,
        r#"
[effect-system.dimensions.freshness]
composition = "Product"
type = "number"
"#,
        "agent noop() -> String:\n    return \"x\"\n",
    );

    let src_path = root.join("src").join("main.cor");
    let source = fs::read_to_string(&src_path).unwrap();
    let config = load_corvid_config_for(&src_path);
    let result = compile_with_config(&source, config.as_ref());
    assert!(
        !result.ok(),
        "program with malformed corvid.toml should not compile"
    );
    let msg = result
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        msg.contains("freshness") && msg.contains("Product"),
        "expected dimension + bad-rule in diagnostic, got:\n{msg}"
    );
}

#[test]
fn collision_with_builtin_dimension_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_project(
        root,
        r#"
[effect-system.dimensions.cost]
composition = "Sum"
type = "cost"
"#,
        "agent noop() -> String:\n    return \"x\"\n",
    );

    let src_path = root.join("src").join("main.cor");
    let source = fs::read_to_string(&src_path).unwrap();
    let config = load_corvid_config_for(&src_path);
    let result = compile_with_config(&source, config.as_ref());
    assert!(!result.ok(), "collision with built-in must be rejected");
    let msg = result
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        msg.contains("cost") && msg.contains("built-in"),
        "expected built-in collision message, got:\n{msg}"
    );
}

#[test]
fn law_check_runs_against_real_custom_dimension() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write_project(
        root,
        r#"
[effect-system.dimensions.freshness]
composition = "Max"
type = "number"
default = "0"

[effect-system.dimensions.carbon]
composition = "Sum"
type = "number"
default = "0"
"#,
        "agent noop() -> String:\n    return \"x\"\n",
    );

    let src_path = root.join("src").join("main.cor");
    let config = load_corvid_config_for(&src_path).expect("config loads");
    let results = run_law_checks(Some(&config), 500);

    // Every built-in + every custom dimension got checked.
    let dims: std::collections::BTreeSet<_> =
        results.iter().map(|r| r.dimension.as_str()).collect();
    assert!(dims.contains("freshness"), "freshness should be checked");
    assert!(dims.contains("carbon"), "carbon should be checked");
    assert!(dims.contains("cost"), "built-in cost should be checked");

    // No counter-examples for correctly-declared dimensions.
    let failures: Vec<_> = results
        .iter()
        .filter(|r| matches!(r.verdict, LawVerdict::CounterExample { .. }))
        .collect();
    assert!(
        failures.is_empty(),
        "unexpected law failures for well-formed dimensions: {:?}",
        failures
            .iter()
            .map(|r| format!("{} / {}", r.dimension, r.law.as_str()))
            .collect::<Vec<_>>()
    );

    // Human-readable render mentions each dimension by name.
    let rendered = render_law_check_report(&results);
    assert!(rendered.contains("freshness"));
    assert!(rendered.contains("carbon"));
    assert!(rendered.contains("cost"));
}

#[test]
fn law_check_default_samples_matches_expected() {
    // Regression: DEFAULT_SAMPLES is exported through the driver so CLI
    // callers don't guess at the sample count. If this constant ever
    // changes deliberately, this test surfaces it.
    assert_eq!(DEFAULT_SAMPLES, 10_000);
}

#[test]
fn spec_check_passes_compile_and_skip_examples_and_fails_on_mismatch() {
    use corvid_driver::{verify_spec_examples, VerdictKind};
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::write(
        root.join("section.md"),
        "\
# Section

A compilable example:

```corvid
# expect: compile
tool echo(id: String) -> String

agent run(id: String) -> String:
    return echo(id)
```

An illustrative fragment:

```corvid
# expect: skip
pseudo-code with undefined references
```

An example that should produce an error:

```corvid
# expect: error
agent bad() -> String:
    return nonexistent_callee()
```

A mismatched example (claims compile, but doesn't):

```corvid
# expect: compile
agent also_bad() -> String:
    return also_unknown()
```
",
    )
    .unwrap();

    let verdicts = verify_spec_examples(root).expect("spec verification runs");
    assert_eq!(verdicts.len(), 4);
    assert!(matches!(verdicts[0].kind, VerdictKind::Pass));
    assert!(matches!(verdicts[1].kind, VerdictKind::Skipped));
    assert!(matches!(verdicts[2].kind, VerdictKind::Pass));
    assert!(
        matches!(verdicts[3].kind, VerdictKind::Fail { .. }),
        "mismatched block must fail; got {:?}",
        verdicts[3].kind
    );
}

#[test]
fn missing_corvid_toml_returns_none_no_error() {
    let tmp = TempDir::new().unwrap();
    let src_path = tmp.path().join("nested").join("main.cor");
    fs::create_dir_all(src_path.parent().unwrap()).unwrap();
    fs::write(&src_path, "agent noop() -> String:\n    return \"x\"\n").unwrap();

    let config = load_corvid_config_for(&src_path);
    assert!(
        config.is_none(),
        "no corvid.toml anywhere in the walk → None"
    );
}
