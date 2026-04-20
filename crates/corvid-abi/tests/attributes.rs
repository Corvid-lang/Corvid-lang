mod common;

use common::emit_descriptor;

#[test]
fn replayable_true_when_agent_has_at_replayable() {
    let abi = emit_descriptor(
        r#"
@replayable
pub extern "c"
agent refund_bot(ticket_id: String) -> Bool:
    return true
"#,
    );
    assert!(abi.agents[0].attributes.replayable);
}

#[test]
fn deterministic_true_when_agent_has_at_deterministic() {
    let abi = emit_descriptor(
        r#"
@deterministic
pub extern "c"
agent refund_bot(ticket_id: String) -> Bool:
    return ticket_id == "vip"
"#,
    );
    assert!(abi.agents[0].attributes.deterministic);
    assert!(abi.agents[0].attributes.replayable);
}

#[test]
fn dangerous_true_when_agent_transitively_calls_dangerous_tool() {
    let abi = emit_descriptor(
        r#"
tool issue_refund(id: String) -> Bool dangerous

pub extern "c"
agent refund_bot(ticket_id: String) -> Bool:
    approve IssueRefund(ticket_id)
    return issue_refund(ticket_id)
"#,
    );
    assert!(abi.agents[0].attributes.dangerous);
}

#[test]
fn dangerous_false_when_agent_has_no_dangerous_path() {
    let abi = emit_descriptor(
        r#"
pub extern "c"
agent refund_bot(ticket_id: String) -> Bool:
    return ticket_id == "vip"
"#,
    );
    assert!(!abi.agents[0].attributes.dangerous);
}

#[test]
fn pub_extern_c_flag_reflects_source_annotation() {
    let abi = emit_descriptor(
        r#"
pub extern "c"
agent refund_bot(ticket_id: String) -> Bool:
    return true
"#,
    );
    assert!(abi.agents[0].attributes.pub_extern_c);
}
