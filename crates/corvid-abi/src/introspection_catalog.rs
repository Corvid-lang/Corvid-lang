use crate::schema::{
    AbiAgent, AbiApprovalContract, AbiAttributes, AbiBudget, AbiEffects, AbiParam,
    AbiProjectedUsd, AbiProvenanceContract, AbiSourceSpan, CorvidAbi, ScalarTypeName,
    TypeDescription,
};

pub fn with_introspection_agents(mut abi: CorvidAbi) -> CorvidAbi {
    if abi
        .agents
        .iter()
        .any(|agent| agent.name.starts_with("__corvid_"))
    {
        return abi;
    }
    abi.agents.extend(introspection_agents());
    abi
}

pub fn introspection_agents() -> Vec<AbiAgent> {
    vec![
        introspection_agent(
            "__corvid_abi_descriptor_json",
            "__corvid_abi_descriptor_json",
            Vec::new(),
        ),
        introspection_agent(
            "__corvid_abi_verify",
            "__corvid_abi_verify",
            vec![AbiParam {
                name: "expected_hash_hex".to_string(),
                ty: scalar(ScalarTypeName::String),
            }],
        ),
        introspection_agent(
            "__corvid_list_agents",
            "__corvid_list_agents",
            Vec::new(),
        ),
        introspection_agent(
            "__corvid_agent_signature_json",
            "__corvid_agent_signature_json",
            vec![AbiParam {
                name: "agent_name".to_string(),
                ty: scalar(ScalarTypeName::String),
            }],
        ),
        introspection_agent(
            "__corvid_pre_flight",
            "__corvid_pre_flight",
            vec![
                AbiParam {
                    name: "agent_name".to_string(),
                    ty: scalar(ScalarTypeName::String),
                },
                AbiParam {
                    name: "args_json".to_string(),
                    ty: scalar(ScalarTypeName::String),
                },
            ],
        ),
        introspection_agent(
            "__corvid_call_agent",
            "__corvid_call_agent",
            vec![
                AbiParam {
                    name: "agent_name".to_string(),
                    ty: scalar(ScalarTypeName::String),
                },
                AbiParam {
                    name: "args_json".to_string(),
                    ty: scalar(ScalarTypeName::String),
                },
            ],
        ),
    ]
}

fn introspection_agent(name: &str, symbol: &str, params: Vec<AbiParam>) -> AbiAgent {
    AbiAgent {
        name: name.to_string(),
        symbol: symbol.to_string(),
        source_span: AbiSourceSpan { start: 0, end: 0 },
        source_line: 0,
        params,
        return_type: scalar(ScalarTypeName::String),
        effects: AbiEffects {
            cost: Some(AbiProjectedUsd { projected_usd: 0.0 }),
            trust_tier: Some("autonomous".to_string()),
            reversibility: Some("reversible".to_string()),
            ..AbiEffects::default()
        },
        attributes: AbiAttributes {
            replayable: false,
            deterministic: true,
            dangerous: false,
            pub_extern_c: true,
        },
        budget: Some(AbiBudget { usd_per_call: 0.0 }),
        required_capability: None,
        dispatch: None,
        approval_contract: AbiApprovalContract {
            required: false,
            labels: Vec::new(),
        },
        provenance: AbiProvenanceContract {
            returns_grounded: false,
            grounded_param_deps: Vec::new(),
        },
    }
}

fn scalar(name: ScalarTypeName) -> TypeDescription {
    TypeDescription::Scalar { scalar: name }
}
