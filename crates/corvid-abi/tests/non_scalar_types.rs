mod common;

use common::emit_descriptor;
use corvid_abi::TypeDescription;

const NON_SCALAR_SRC: &str = r#"
type Decision:
    should_refund: Bool
    reason: String

type Failure:
    code: Int

effect retrieval:
    data: grounded

tool get_order(id: String) -> List<Option<Int>>

prompt classify(ticket_id: String) -> Result<List<Option<Int>>, String>:
    "classify {ticket_id}"

tool fetch_grounded(id: String) -> Grounded<String> uses retrieval

pub extern "c"
agent refund_bot(ticket_id: String) -> Bool:
    data = classify(ticket_id)
    grounded = grounded_bot(ticket_id)
    cached = weak_bot(ticket_id)
    return true

agent grounded_bot(ticket_id: String) -> Grounded<Decision>:
    order = fetch_grounded(ticket_id)
    return decide(ticket_id, order)

prompt decide(ticket_id: String, order: Grounded<String>) -> Grounded<Decision>:
    "decide {ticket_id} using {order}"

agent weak_bot(ticket_id: String) -> Weak<Decision, {tool_call, llm}>:
    return cached(ticket_id)

prompt cached(ticket_id: String) -> Weak<Decision, {tool_call, llm}>:
    "cached {ticket_id}"
"#;

fn find_prompt<'a>(abi: &'a corvid_abi::CorvidAbi, name: &str) -> &'a corvid_abi::AbiPrompt {
    abi.prompts.iter().find(|prompt| prompt.name == name).expect("prompt")
}

#[test]
fn list_type_descriptor_has_element_field() {
    let abi = emit_descriptor(NON_SCALAR_SRC);
    let classify = find_prompt(&abi, "classify");
    match &classify.return_type {
        TypeDescription::Result { result } => match result.ok.as_ref() {
            TypeDescription::List { list } => {
                assert!(matches!(list.element.as_ref(), TypeDescription::Option { .. }));
            }
            other => panic!("expected list type, got {other:?}"),
        },
        other => panic!("expected result type, got {other:?}"),
    }
}

#[test]
fn option_type_descriptor_has_inner_field() {
    let abi = emit_descriptor(NON_SCALAR_SRC);
    let classify = find_prompt(&abi, "classify");
    match &classify.return_type {
        TypeDescription::Result { result } => match result.ok.as_ref() {
            TypeDescription::List { list } => match list.element.as_ref() {
                TypeDescription::Option { option } => {
                    assert!(matches!(option.inner.as_ref(), TypeDescription::Scalar { .. }));
                }
                other => panic!("expected option type, got {other:?}"),
            },
            other => panic!("expected list type, got {other:?}"),
        },
        other => panic!("expected result type, got {other:?}"),
    }
}

#[test]
fn result_type_descriptor_has_ok_and_err_fields() {
    let abi = emit_descriptor(NON_SCALAR_SRC);
    let classify = find_prompt(&abi, "classify");
    match &classify.return_type {
        TypeDescription::Result { result } => {
            assert!(matches!(result.ok.as_ref(), TypeDescription::List { .. }));
            assert!(matches!(result.err.as_ref(), TypeDescription::Scalar { .. }));
        }
        other => panic!("expected result type, got {other:?}"),
    }
}

#[test]
fn grounded_type_descriptor_has_inner_field() {
    let abi = emit_descriptor(NON_SCALAR_SRC);
    let decide = find_prompt(&abi, "decide");
    match &decide.return_type {
        TypeDescription::Grounded { grounded } => {
            assert!(matches!(grounded.inner.as_ref(), TypeDescription::Struct { .. }));
        }
        other => panic!("expected grounded type, got {other:?}"),
    }
}

#[test]
fn weak_type_descriptor_has_effects_list() {
    let abi = emit_descriptor(NON_SCALAR_SRC);
    let cached = find_prompt(&abi, "cached");
    match &cached.return_type {
        TypeDescription::Weak { weak } => {
            assert_eq!(weak.effects, vec!["tool_call".to_string(), "llm_call".to_string()]);
        }
        other => panic!("expected weak type, got {other:?}"),
    }
}

#[test]
fn struct_references_go_through_types_array() {
    let abi = emit_descriptor(NON_SCALAR_SRC);
    assert!(abi.types.iter().any(|ty| ty.name == "Decision"));
    let decide = find_prompt(&abi, "decide");
    match &decide.return_type {
        TypeDescription::Grounded { grounded } => match grounded.inner.as_ref() {
            TypeDescription::Struct { name } => assert_eq!(name, "Decision"),
            other => panic!("expected struct ref, got {other:?}"),
        },
        other => panic!("expected grounded type, got {other:?}"),
    }
}

#[test]
fn nested_type_descriptors_nest_structurally() {
    let abi = emit_descriptor(NON_SCALAR_SRC);
    let classify = find_prompt(&abi, "classify");
    match &classify.return_type {
        TypeDescription::Result { result } => match result.ok.as_ref() {
            TypeDescription::List { list } => match list.element.as_ref() {
                TypeDescription::Option { option } => {
                    assert!(matches!(option.inner.as_ref(), TypeDescription::Scalar { .. }));
                }
                other => panic!("expected nested option, got {other:?}"),
            },
            other => panic!("expected list, got {other:?}"),
        },
        other => panic!("expected result, got {other:?}"),
    }
}
