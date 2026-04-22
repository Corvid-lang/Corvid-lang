use corvid_c_header::{emit_header, HeaderOptions};
use corvid_ir::{IrAgent, IrExternAbi, IrFile, IrParam};
use corvid_resolve::{DefId, LocalId};
use corvid_types::Type;
use corvid_ast::Span;

fn scalar_ir() -> IrFile {
    IrFile {
        imports: vec![],
        types: vec![],
        tools: vec![],
        prompts: vec![],
        agents: vec![
            IrAgent {
                id: DefId(1),
                name: "refund_bot".into(),
                extern_abi: Some(IrExternAbi::C),
                params: vec![
                    IrParam {
                        name: "ticket_id".into(),
                        local_id: LocalId(1),
                        ty: Type::String,
                        span: Span::new(0, 0),
                    },
                    IrParam {
                        name: "amount".into(),
                        local_id: LocalId(2),
                        ty: Type::Float,
                        span: Span::new(0, 0),
                    },
                ],
                return_ty: Type::Bool,
                cost_budget: None,
                body: corvid_ir::IrBlock {
                    stmts: vec![],
                    span: Span::new(0, 0),
                },
                span: Span::new(0, 0),
                borrow_sig: None,
            },
            IrAgent {
                id: DefId(2),
                name: "echo_name".into(),
                extern_abi: Some(IrExternAbi::C),
                params: vec![IrParam {
                    name: "name".into(),
                    local_id: LocalId(3),
                    ty: Type::String,
                    span: Span::new(0, 0),
                }],
                return_ty: Type::String,
                cost_budget: None,
                body: corvid_ir::IrBlock {
                    stmts: vec![],
                    span: Span::new(0, 0),
                },
                span: Span::new(0, 0),
                borrow_sig: None,
            },
            IrAgent {
                id: DefId(3),
                name: "touch".into(),
                extern_abi: Some(IrExternAbi::C),
                params: vec![],
                return_ty: Type::Nothing,
                cost_budget: None,
                body: corvid_ir::IrBlock {
                    stmts: vec![],
                    span: Span::new(0, 0),
                },
                span: Span::new(0, 0),
                borrow_sig: None,
            },
            IrAgent {
                id: DefId(4),
                name: "grounded_lookup".into(),
                extern_abi: Some(IrExternAbi::C),
                params: vec![IrParam {
                    name: "id".into(),
                    local_id: LocalId(4),
                    ty: Type::String,
                    span: Span::new(0, 0),
                }],
                return_ty: Type::Grounded(Box::new(Type::String)),
                cost_budget: None,
                body: corvid_ir::IrBlock {
                    stmts: vec![],
                    span: Span::new(0, 0),
                },
                span: Span::new(0, 0),
                borrow_sig: None,
            },
        ],
        evals: vec![],
    }
}

fn render() -> String {
    emit_header(
        &scalar_ir(),
        &HeaderOptions {
            library_name: "refund_bot".into(),
        },
    )
}

#[test]
fn header_has_include_guard() {
    let header = render();
    assert!(header.contains("#ifndef CORVID_REFUND_BOT_H"));
    assert!(header.contains("#define CORVID_REFUND_BOT_H"));
}

#[test]
fn header_has_extern_c_block() {
    let header = render();
    assert!(header.contains("extern \"C\" {"));
}

#[test]
fn header_includes_stdint_and_stdbool() {
    let header = render();
    assert!(header.contains("#include <stddef.h>"));
    assert!(header.contains("#include <stdint.h>"));
    assert!(header.contains("#include <stdbool.h>"));
}

#[test]
fn header_exports_scalar_agent_with_correct_c_types() {
    let header = render();
    assert!(header.contains("bool refund_bot(const char* ticket_id, double amount);"));
}

#[test]
fn header_exports_string_return_with_ownership_comment() {
    let header = render();
    assert!(header.contains("release returned agent strings with `corvid_free_string(...)`"));
    assert!(header.contains("release `corvid_call_agent` JSON payloads with `corvid_free_result(...)`"));
    assert!(header.contains("observation handles are runtime attestations; release them with `corvid_observation_release(...)`"));
    assert!(header.contains("const char* echo_name(const char* name);"));
    assert!(header.contains("void corvid_free_string(const char* value);"));
    assert!(header.contains("void corvid_free_result(char* result);"));
}

#[test]
fn header_exports_grounded_string_return_with_handle_out_param() {
    let header = render();
    assert!(header.contains("#define CORVID_NULL_GROUNDED_HANDLE ((uint64_t)0)"));
    assert!(header.contains("const char* grounded_lookup(const char* id, uint64_t* out_grounded_handle);"));
    assert!(header.contains("int32_t corvid_grounded_sources(uint64_t handle, const char** out, size_t capacity);"));
    assert!(header.contains("double corvid_grounded_confidence(uint64_t handle);"));
    assert!(header.contains("void corvid_grounded_release(uint64_t handle);"));
}

#[test]
fn header_emits_nothing_return_as_void() {
    let header = render();
    assert!(header.contains("void touch(void);"));
}

#[test]
fn header_exports_catalog_surface() {
    let header = render();
    assert!(header.contains("typedef struct {\n    const char* name;"));
    assert!(header.contains("#define CORVID_NULL_OBSERVATION_HANDLE ((uint64_t)0)"));
    assert!(header.contains("size_t corvid_list_agents(CorvidAgentHandle* out, size_t capacity);"));
    assert!(header.contains("CorvidFindAgentsResult corvid_find_agents_where("));
    assert!(header.contains("CorvidPreFlight corvid_pre_flight("));
    assert!(header.contains("CorvidCallStatus corvid_call_agent("));
    assert!(header.contains("uint64_t* out_observation_handle,"));
    assert!(header.contains("void corvid_register_approver(CorvidApproverFn fn, void* user_data);"));
    assert!(header.contains("CorvidApproverLoadStatus corvid_register_approver_from_source("));
    assert!(header.contains("void corvid_clear_approver(void);"));
    assert!(header.contains("const char* corvid_approval_predicate_json(const char* site_name, size_t* out_len);"));
    assert!(header.contains("CorvidPredicateResult corvid_evaluate_approval_predicate("));
    assert!(header.contains("double corvid_observation_cost_usd(uint64_t handle);"));
    assert!(header.contains("uint64_t corvid_observation_latency_ms(uint64_t handle);"));
    assert!(header.contains("uint64_t corvid_observation_tokens_in(uint64_t handle);"));
    assert!(header.contains("uint64_t corvid_observation_tokens_out(uint64_t handle);"));
    assert!(header.contains("bool corvid_observation_exceeded_bound(uint64_t handle);"));
    assert!(header.contains("void corvid_observation_release(uint64_t handle);"));
}
