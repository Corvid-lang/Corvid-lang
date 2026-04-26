mod common;

use common::emit_descriptor;
use corvid_abi::AbiStoreAccessorKind;

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
    assert_eq!(session.accessors.len(), 6);
    let user_id_get = session
        .accessors
        .iter()
        .find(|accessor| accessor.name == "Conversation.user_id.get")
        .expect("user_id get accessor");
    assert_eq!(user_id_get.field, "user_id");
    assert_eq!(user_id_get.kind, AbiStoreAccessorKind::Get);
    assert_eq!(user_id_get.effect, "reads_session");
    let current_fact_set = session
        .accessors
        .iter()
        .find(|accessor| accessor.name == "Conversation.current_fact.set")
        .expect("current_fact set accessor");
    assert_eq!(current_fact_set.field, "current_fact");
    assert_eq!(current_fact_set.kind, AbiStoreAccessorKind::Set);
    assert_eq!(current_fact_set.effect, "writes_session");
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
    assert_eq!(memory.accessors.len(), 3);
    let memory_delete = memory
        .accessors
        .iter()
        .find(|accessor| accessor.name == "Profile.stable_fact.delete")
        .expect("memory delete accessor");
    assert_eq!(memory_delete.kind, AbiStoreAccessorKind::Delete);
    assert_eq!(memory_delete.effect, "writes_memory");
    assert_eq!(memory.policies.len(), 1);
    assert_eq!(memory.policies[0].name, "approval_required");
    assert_eq!(memory.policies[0].value, true);
}
