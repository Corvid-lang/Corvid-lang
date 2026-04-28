mod common;

use common::emit_descriptor;

const APPROVAL_SRC: &str = r#"
effect transfer_money:
    cost: $0.10
    trust: human_required
    reversible: false

tool issue_refund(order_id: String, amount: Float) -> Bool dangerous uses transfer_money

pub extern "c"
agent refund_bot(order_id: String, amount: Float) -> Bool:
    approve IssueRefund(order_id, amount)
    return issue_refund(order_id, amount)
"#;

#[test]
fn agent_with_approve_site_lists_labels_with_args() {
    let abi = emit_descriptor(APPROVAL_SRC);
    let contract = &abi.agents[0].approval_contract;
    assert!(contract.required);
    assert_eq!(contract.labels[0].label, "IssueRefund");
    assert_eq!(contract.labels[0].args.len(), 2);
}

#[test]
fn agent_without_dangerous_path_has_required_false() {
    let abi = emit_descriptor(
        r#"
pub extern "c"
agent refund_bot(order_id: String) -> Bool:
    return order_id == "vip"
"#,
    );
    assert!(!abi.agents[0].approval_contract.required);
}

#[test]
fn approval_site_lists_dangerous_targets_it_gates() {
    let abi = emit_descriptor(APPROVAL_SRC);
    let site = abi
        .approval_sites
        .iter()
        .find(|site| site.label == "IssueRefund")
        .expect("approval site");
    assert_eq!(site.dangerous_targets, vec!["issue_refund".to_string()]);
}

#[test]
fn approval_site_carries_cost_and_reversibility_from_target() {
    let abi = emit_descriptor(APPROVAL_SRC);
    let site = abi
        .approval_sites
        .iter()
        .find(|site| site.label == "IssueRefund")
        .expect("approval site");
    assert_eq!(site.effects.cost.as_ref().unwrap().projected_usd, 0.10);
    assert_eq!(
        site.effects.reversibility.as_deref(),
        Some("non_reversible")
    );
}

#[test]
fn tool_contract_records_domain_effects_and_approval_requirements() {
    let abi = emit_descriptor(
        r#"
effect stripe_charge:
    trust: human_required
    reversible: false
    domain: money
    money: amount
    external: stripe
    requires_approval: charge_card

tool charge_card(customer_id: String, amount: Float) -> Bool uses stripe_charge

pub extern "c"
agent noop() -> Bool:
    return true
"#,
    );

    let tool = abi
        .tools
        .iter()
        .find(|tool| tool.name == "charge_card")
        .expect("tool contract");
    assert_eq!(
        tool.contract.requires_approval.as_deref(),
        Some("charge-card")
    );
    assert!(tool
        .contract
        .domain_effects
        .iter()
        .any(|effect| effect.kind == "money" && effect.target.as_deref() == Some("amount")));
    assert!(tool
        .contract
        .domain_effects
        .iter()
        .any(|effect| effect.kind == "external" && effect.target.as_deref() == Some("stripe")));
    assert!(tool
        .contract
        .domain_effects
        .iter()
        .any(|effect| effect.kind == "irreversible"));
    assert!(tool.contract.ci_fail_on.contains(&"money".to_string()));
    assert!(tool
        .contract
        .ci_fail_on
        .contains(&"irreversible".to_string()));
}
