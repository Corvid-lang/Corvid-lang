//! End-to-end tests for custom effect dimensions declared via
//! `corvid.toml`. Proves the driver discovers the config file via
//! upward-walking and the type checker sees the custom dimensions.

use std::fs;

use corvid_driver::{compile_with_config, load_corvid_config_for};
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
