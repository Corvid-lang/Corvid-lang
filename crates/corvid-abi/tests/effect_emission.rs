mod common;

use common::{emit_descriptor, emit_descriptor_with_config};

const EFFECT_SRC: &str = r#"
effect llm_call:
    cost: $0.005
    trust: autonomous
    latency_ms: 1500
    reversible: true
    data: none
    confidence: 0.80
    tokens: 4200

effect transfer_money:
    cost: $0.10
    trust: human_required
    latency_ms: 50
    reversible: false
    data: grounded
    confidence: 0.95
    tokens: 10

prompt classify(ticket_id: String) -> String uses llm_call:
    "classify {ticket_id}"

tool issue_refund(ticket_id: String, amount: Float) -> Bool dangerous uses transfer_money

pub extern "c"
agent refund_bot(ticket_id: String) -> Bool:
    approve IssueRefund(ticket_id, 10.0)
    verdict = classify(ticket_id)
    if verdict == "refund":
        return issue_refund(ticket_id, 10.0)
    return false
"#;

#[test]
fn cost_emits_as_projected_usd() {
    let abi = emit_descriptor(EFFECT_SRC);
    let actual = abi.agents[0].effects.cost.as_ref().unwrap().projected_usd;
    assert!((actual - 0.105).abs() < 1e-12, "actual cost was {actual}");
}

#[test]
fn trust_tier_emits_exact_tier_name() {
    let abi = emit_descriptor(EFFECT_SRC);
    assert_eq!(abi.agents[0].effects.trust_tier.as_deref(), Some("human_required"));
}

#[test]
fn latency_emits_p99_estimate() {
    let abi = emit_descriptor(EFFECT_SRC);
    assert_eq!(abi.agents[0].effects.latency_ms.as_ref().unwrap().p99_estimate, 1500.0);
}

#[test]
fn reversibility_emits_as_reversible_or_non_reversible() {
    let abi = emit_descriptor(EFFECT_SRC);
    assert_eq!(abi.agents[0].effects.reversibility.as_deref(), Some("non_reversible"));
}

#[test]
fn data_emits_as_none_ungrounded_or_grounded() {
    let abi = emit_descriptor(EFFECT_SRC);
    assert_eq!(abi.agents[0].effects.data.as_deref(), Some("grounded"));
}

#[test]
fn confidence_emits_min_expected_floor() {
    let abi = emit_descriptor(EFFECT_SRC);
    assert_eq!(abi.agents[0].effects.confidence.as_ref().unwrap().min_expected, 0.8);
}

#[test]
fn tokens_emits_projected_count() {
    let abi = emit_descriptor(EFFECT_SRC);
    assert_eq!(abi.agents[0].effects.tokens.as_ref().unwrap().projected, 4210.0);
}

#[test]
fn custom_dimension_from_corvid_toml_emits_under_effects_custom() {
    let source = r#"
effect llm_call:
    cost: $0.001
    freshness: 10

prompt classify(ticket_id: String) -> String uses llm_call:
    "classify {ticket_id}"

pub extern "c"
agent refund_bot(ticket_id: String) -> Bool:
    return classify(ticket_id) == "ok"
"#;
    let config = r#"
[effect-system.dimensions.freshness]
composition = "Max"
type = "number"
default = "0"
"#;
    let abi = emit_descriptor_with_config(source, Some(config));
    assert_eq!(abi.agents[0].effects.custom["freshness"], serde_json::json!(10.0));
}
