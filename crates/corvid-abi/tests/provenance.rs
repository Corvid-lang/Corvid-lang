mod common;

use common::emit_descriptor;

#[test]
fn agent_returning_grounded_t_flags_returns_grounded_true() {
    let abi = emit_descriptor(
        r#"
type Decision:
    ok: Bool

effect retrieval:
    data: grounded

tool fetch_order(ticket: String) -> Grounded<String> uses retrieval

agent grounded_helper(ticket: String) -> Grounded<Decision>:
    order = fetch_order(ticket)
    return decide(ticket, order)

prompt decide(ticket: String, order: Grounded<String>) -> Grounded<Decision>:
    "decide {ticket} using {order}"

pub extern "c"
agent refund_bot(ticket: String) -> Bool:
    decision = grounded_helper(ticket)
    return true
"#,
    );
    let helper = abi
        .agents
        .iter()
        .find(|agent| agent.name == "grounded_helper")
        .unwrap();
    assert!(helper.provenance.returns_grounded);
}

#[test]
fn agent_not_returning_grounded_flags_returns_grounded_false() {
    let abi = emit_descriptor(
        r#"
pub extern "c"
agent refund_bot(ticket: String) -> Bool:
    return true
"#,
    );
    assert!(!abi.agents[0].provenance.returns_grounded);
}

#[test]
fn grounded_param_deps_list_citation_sources() {
    let abi = emit_descriptor(
        r#"
type Decision:
    ok: Bool

effect retrieval:
    data: grounded

tool get_order(id: String) -> Grounded<String> uses retrieval

prompt decide(ticket: String, order: Grounded<String>) -> Grounded<Decision>:
    "decide {ticket} using {order}"

agent grounded_helper(ticket: String) -> Grounded<Decision>:
    order = get_order(ticket)
    return decide(ticket, order)

pub extern "c"
agent refund_bot(ticket: String) -> Bool:
    decision = grounded_helper(ticket)
    return true
"#,
    );
    let helper = abi
        .agents
        .iter()
        .find(|agent| agent.name == "grounded_helper")
        .unwrap();
    assert_eq!(
        helper.provenance.grounded_param_deps,
        vec!["ticket".to_string()]
    );
}
