mod common;

use common::emit_descriptor;

#[test]
fn emits_session_and_memory_store_contracts() {
    let abi = emit_descriptor(
        r#"
type Fact:
    text: String

session Conversation:
    user_id: String
    current_fact: Fact
    policy retention: ttl_24h

memory Profile:
    stable_fact: Grounded<Fact>
    policy approval_required: true
"#,
    );

    assert_eq!(abi.stores.len(), 2);

    let session = abi
        .stores
        .iter()
        .find(|store| store.name == "Conversation")
        .expect("session store");
    assert_eq!(session.kind, "session");
    assert_eq!(session.effects.read, "reads_session");
    assert_eq!(session.effects.write, "writes_session");
    assert_eq!(session.fields.len(), 2);
    assert_eq!(session.policies.len(), 1);
    assert_eq!(session.policies[0].name, "retention");
    assert_eq!(session.policies[0].value, "ttl_24h");

    let memory = abi
        .stores
        .iter()
        .find(|store| store.name == "Profile")
        .expect("memory store");
    assert_eq!(memory.kind, "memory");
    assert_eq!(memory.effects.read, "reads_memory");
    assert_eq!(memory.effects.write, "writes_memory");
    assert_eq!(memory.fields.len(), 1);
    assert_eq!(memory.policies.len(), 1);
    assert_eq!(memory.policies[0].name, "approval_required");
    assert_eq!(memory.policies[0].value, true);
}
