use corvid_abi::{emit_abi, render_descriptor_json, CorvidAbi, EmitOptions};
use corvid_ast::File;
use corvid_ir::{lower, IrFile};
use corvid_resolve::{resolve, Resolved};
use corvid_syntax::{lex, parse_file};
use corvid_types::{typecheck_with_config, Checked, CorvidConfig, EffectRegistry};

pub const FIXED_GENERATED_AT: &str = "2026-04-21T10:30:00Z";

pub struct TestBundle {
    pub file: File,
    pub resolved: Resolved,
    pub checked: Checked,
    pub ir: IrFile,
    pub registry: EffectRegistry,
    pub config: Option<CorvidConfig>,
}

pub fn compile_bundle(source: &str, config_toml: Option<&str>) -> TestBundle {
    let config = config_toml
        .map(|text| toml::from_str::<CorvidConfig>(text).expect("parse corvid.toml"));
    let tokens = lex(source).expect("lex");
    let (file, parse_errs) = parse_file(&tokens);
    assert!(parse_errs.is_empty(), "parse errors: {parse_errs:?}");
    let resolved = resolve(&file);
    assert!(resolved.errors.is_empty(), "resolve errors: {:?}", resolved.errors);
    let checked = typecheck_with_config(&file, &resolved, config.as_ref());
    assert!(checked.errors.is_empty(), "type errors: {:?}", checked.errors);
    let effect_decls = file
        .decls
        .iter()
        .filter_map(|decl| match decl {
            corvid_ast::Decl::Effect(effect) => Some(effect.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let registry = EffectRegistry::from_decls_with_config(&effect_decls, config.as_ref());
    let ir = lower(&file, &resolved, &checked);
    TestBundle {
        file,
        resolved,
        checked,
        ir,
        registry,
        config,
    }
}

pub fn emit_descriptor(source: &str) -> CorvidAbi {
    emit_descriptor_with_config(source, None)
}

pub fn emit_descriptor_with_config(source: &str, config_toml: Option<&str>) -> CorvidAbi {
    let bundle = compile_bundle(source, config_toml);
    emit_abi(
        &bundle.file,
        &bundle.resolved,
        &bundle.checked,
        &bundle.ir,
        &bundle.registry,
        &EmitOptions {
            source_path: "src/refund_bot.cor",
            compiler_version: "0.0.1",
            generated_at: FIXED_GENERATED_AT,
        },
    )
}

pub fn render_descriptor(source: &str) -> String {
    render_descriptor_json(&emit_descriptor(source)).expect("render descriptor")
}

pub fn render_descriptor_with_config(source: &str, config_toml: Option<&str>) -> String {
    render_descriptor_json(&emit_descriptor_with_config(source, config_toml))
        .expect("render descriptor")
}
