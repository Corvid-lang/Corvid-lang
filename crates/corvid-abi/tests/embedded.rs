mod common;

use common::{compile_bundle, FIXED_GENERATED_AT};
use corvid_abi::{
    descriptor_from_embedded_section, descriptor_to_embedded_bytes, emit_catalog_abi, hash_json_str,
    parse_embedded_section_bytes, with_introspection_agents, CORVID_ABI_SECTION_MAGIC,
    CORVID_ABI_VERSION,
};

const SOURCE: &str = r#"
pub extern "c"
agent classify(text: String) -> String:
    return text
"#;

fn emit_catalog_descriptor() -> corvid_abi::CorvidAbi {
    let bundle = compile_bundle(SOURCE, None);
    emit_catalog_abi(
        &bundle.file,
        &bundle.resolved,
        &bundle.checked,
        &bundle.ir,
        &bundle.registry,
        &corvid_abi::EmitOptions {
            source_path: "examples/cdylib_catalog_demo/src/classify.cor",
            source_text: SOURCE,
            compiler_version: "0.6.0-phase22",
            generated_at: FIXED_GENERATED_AT,
        },
    )
}

#[test]
fn embed_and_extract_roundtrip_preserves_catalog_descriptor() {
    let abi = emit_catalog_descriptor();
    let bytes = descriptor_to_embedded_bytes(&abi).expect("encode");
    let section = parse_embedded_section_bytes(&bytes).expect("parse section");
    let decoded = descriptor_from_embedded_section(&section).expect("decode");
    assert_eq!(decoded, abi);
}

#[test]
fn embedded_section_has_corv_magic() {
    let abi = emit_catalog_descriptor();
    let bytes = descriptor_to_embedded_bytes(&abi).expect("encode");
    let magic = u32::from_le_bytes(bytes[0..4].try_into().expect("magic width"));
    assert_eq!(magic, CORVID_ABI_SECTION_MAGIC);
}

#[test]
fn embedded_section_abi_version_matches_current_constant() {
    let abi = emit_catalog_descriptor();
    let bytes = descriptor_to_embedded_bytes(&abi).expect("encode");
    let version = u32::from_le_bytes(bytes[4..8].try_into().expect("version width"));
    assert_eq!(version, CORVID_ABI_VERSION);
}

#[test]
fn embedded_section_sha256_matches_independent_json_hash() {
    let abi = emit_catalog_descriptor();
    let bytes = descriptor_to_embedded_bytes(&abi).expect("encode");
    let section = parse_embedded_section_bytes(&bytes).expect("parse section");
    assert_eq!(section.sha256, hash_json_str(&section.json));
}

#[test]
fn identical_catalog_descriptor_builds_produce_identical_embedded_sections() {
    let left = descriptor_to_embedded_bytes(&emit_catalog_descriptor()).expect("left");
    let right = descriptor_to_embedded_bytes(&emit_catalog_descriptor()).expect("right");
    assert_eq!(left, right);
}

#[test]
fn embedded_catalog_contains_the_introspection_agents() {
    let abi = emit_catalog_descriptor();
    assert!(abi
        .agents
        .iter()
        .any(|agent| agent.name == "__corvid_list_agents"));
    assert_eq!(abi, with_introspection_agents(abi.clone()));
}
