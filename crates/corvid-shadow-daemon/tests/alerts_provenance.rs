mod common;

use common::outcome;
use corvid_shadow_daemon::alerts::provenance;
use corvid_shadow_daemon::config::ProvenanceAlertConfig;
use std::collections::BTreeSet;

#[test]
fn provenance_reasoning_drift_alerts_when_output_same_sources_different() {
    let mut result = outcome("refund_bot");
    result.shadow_provenance.root_sources = BTreeSet::from(["retrieval:search".into()]);
    let alerts = provenance::evaluate(&ProvenanceAlertConfig::default(), &result);
    assert!(alerts.iter().any(|alert| alert.summary.contains("cited provenance sources drifted")));
}

#[test]
fn provenance_chain_break_alerts_when_grounded_return_has_no_chain() {
    let mut result = outcome("refund_bot");
    result.shadow_provenance.has_chain = false;
    let alerts = provenance::evaluate(&ProvenanceAlertConfig::default(), &result);
    assert!(alerts.iter().any(|alert| alert.summary.contains("lost its provenance chain")));
}
