mod common;

use common::outcome;
use corvid_shadow_daemon::alerts::consensus;
use corvid_shadow_daemon::config::ConsensusAlertConfig;
use std::collections::HashMap;

#[test]
fn consensus_disagreement_emits_alert_when_prod_disagrees_with_others() {
    let base = outcome("refund_bot");
    let mut model_a = outcome("refund_bot");
    model_a.shadow_output = Some(serde_json::json!("cancel"));
    let mut model_b = outcome("refund_bot");
    model_b.shadow_output = Some(serde_json::json!("cancel"));
    let mut by_model = HashMap::new();
    by_model.insert("a".into(), model_a);
    by_model.insert("b".into(), model_b);
    let config = ConsensusAlertConfig {
        models: vec!["a".into(), "b".into()],
        min_agreement: 2,
        sample_fraction: 1.0,
    };
    let alert = consensus::evaluate(&config, &base, &by_model).unwrap();
    assert!(alert.summary.contains("production result disagreed"));
}

#[test]
fn consensus_agreement_does_not_alert() {
    let base = outcome("refund_bot");
    let by_model = HashMap::from([
        ("a".into(), outcome("refund_bot")),
        ("b".into(), outcome("refund_bot")),
    ]);
    let config = ConsensusAlertConfig {
        models: vec!["a".into(), "b".into()],
        min_agreement: 2,
        sample_fraction: 1.0,
    };
    assert!(consensus::evaluate(&config, &base, &by_model).is_none());
}

#[test]
fn consensus_handles_model_unavailable_as_insufficient_agreement() {
    let base = outcome("refund_bot");
    let by_model = HashMap::from([("a".into(), outcome("refund_bot"))]);
    let config = ConsensusAlertConfig {
        models: vec!["a".into(), "b".into()],
        min_agreement: 2,
        sample_fraction: 1.0,
    };
    let alert = consensus::evaluate(&config, &base, &by_model).unwrap();
    assert!(alert.summary.contains("insufficient agreement"));
}
