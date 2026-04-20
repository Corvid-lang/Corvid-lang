mod common;

use common::{render_descriptor, render_descriptor_with_config};
use serde_json::Value;

const DET_SRC: &str = r#"
effect llm_call:
    cost: $0.001
    confidence: 0.80

prompt classify(ticket: String) -> String uses llm_call:
    "classify {ticket}"

pub extern "c"
agent refund_bot(ticket: String) -> Bool:
    return classify(ticket) == "refund"
"#;

fn normalize_generated_at(json: &str) -> String {
    let mut value: Value = serde_json::from_str(json).expect("json");
    value["generated_at"] = Value::String("<normalized>".into());
    serde_json::to_string_pretty(&value).expect("serialize")
}

#[test]
fn identical_source_produces_byte_identical_descriptor_modulo_generated_at() {
    let left = normalize_generated_at(&render_descriptor(DET_SRC));
    let right = normalize_generated_at(&render_descriptor(DET_SRC));
    assert_eq!(left, right);
}

#[test]
fn map_iteration_order_does_not_affect_output() {
    let config_a = r#"
[effect-system.dimensions.freshness]
composition = "Max"
type = "number"
default = "0"

[effect-system.dimensions.jurisdiction]
composition = "Union"
type = "name"
default = "none"
"#;
    let config_b = r#"
[effect-system.dimensions.jurisdiction]
composition = "Union"
type = "name"
default = "none"

[effect-system.dimensions.freshness]
composition = "Max"
type = "number"
default = "0"
"#;
    let source = r#"
effect llm_call:
    cost: $0.001
    freshness: 5
    jurisdiction: us_hosted

prompt classify(ticket: String) -> String uses llm_call:
    "classify {ticket}"

pub extern "c"
agent refund_bot(ticket: String) -> Bool:
    return classify(ticket) == "refund"
"#;
    let left = normalize_generated_at(&render_descriptor_with_config(source, Some(config_a)));
    let right = normalize_generated_at(&render_descriptor_with_config(source, Some(config_b)));
    assert_eq!(left, right);
}
