mod common;

use common::{emit_descriptor, FIXED_GENERATED_AT};
use corvid_abi::{descriptor_from_json, CorvidAbi, CORVID_ABI_VERSION};
use serde_json::json;
use time::format_description::well_known::Rfc3339;

const BASIC_SRC: &str = r#"
pub extern "c"
agent refund_bot(ticket_id: String, amount: Float) -> Bool:
    return ticket_id == "vip" and amount > amount - 1.0
"#;

#[test]
fn descriptor_roundtrips_through_serde_json() {
    let abi = emit_descriptor(BASIC_SRC);
    let json = serde_json::to_string_pretty(&abi).expect("serialize");
    let parsed: CorvidAbi = descriptor_from_json(&json).expect("deserialize");
    assert_eq!(parsed, abi);
}

#[test]
fn version_lives_at_top_level_and_equals_corvid_abi_version() {
    let abi = emit_descriptor(BASIC_SRC);
    let value = serde_json::to_value(&abi).expect("to value");
    assert_eq!(value["corvid_abi_version"], json!(CORVID_ABI_VERSION));
}

#[test]
fn generated_at_is_rfc3339_utc() {
    let abi = emit_descriptor(BASIC_SRC);
    assert_eq!(abi.generated_at, FIXED_GENERATED_AT);
    time::OffsetDateTime::parse(&abi.generated_at, &Rfc3339).expect("rfc3339");
}

#[test]
fn unknown_future_fields_are_preserved_on_read() {
    let abi = emit_descriptor(BASIC_SRC);
    let mut value = serde_json::to_value(&abi).expect("to value");
    value["future_field"] = json!({"enabled": true});
    let parsed: CorvidAbi = serde_json::from_value(value).expect("from value");
    assert_eq!(parsed.extra["future_field"], json!({"enabled": true}));
}
