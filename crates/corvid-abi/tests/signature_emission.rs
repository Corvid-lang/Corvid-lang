mod common;

use common::emit_descriptor;
use corvid_abi::{ScalarTypeName, TypeDescription};

const TWO_AGENTS_SRC: &str = r#"
pub extern "c"
agent first(a: Int, b: Float, c: Bool, d: String) -> Nothing:
    return nothing

agent helper() -> Bool:
    return true

pub extern "c"
agent second() -> String:
    return "ok"
"#;

#[test]
fn scalar_int_float_bool_string_nothing_emit_correctly() {
    let abi = emit_descriptor(TWO_AGENTS_SRC);
    let first = abi
        .agents
        .iter()
        .find(|agent| agent.name == "first")
        .expect("first");
    assert_eq!(first.params.len(), 4);
    assert_eq!(
        first.params[0].ty,
        TypeDescription::Scalar {
            scalar: ScalarTypeName::Int
        }
    );
    assert_eq!(
        first.params[1].ty,
        TypeDescription::Scalar {
            scalar: ScalarTypeName::Float
        }
    );
    assert_eq!(
        first.params[2].ty,
        TypeDescription::Scalar {
            scalar: ScalarTypeName::Bool
        }
    );
    assert_eq!(
        first.params[3].ty,
        TypeDescription::Scalar {
            scalar: ScalarTypeName::String
        }
    );
    assert_eq!(
        first.return_type,
        TypeDescription::Scalar {
            scalar: ScalarTypeName::Nothing
        }
    );
}

#[test]
fn agent_without_pub_extern_c_omitted_from_agents_list() {
    let abi = emit_descriptor(TWO_AGENTS_SRC);
    assert!(abi.agents.iter().all(|agent| agent.name != "helper"));
}

#[test]
fn multiple_agents_emit_in_declaration_order() {
    let abi = emit_descriptor(TWO_AGENTS_SRC);
    let names = abi
        .agents
        .iter()
        .map(|agent| agent.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["first", "second"]);
}
