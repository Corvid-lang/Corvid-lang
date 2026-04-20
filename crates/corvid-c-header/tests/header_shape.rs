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
    assert!(header.contains("release returned strings with `corvid_free_string(...)`"));
    assert!(header.contains("const char* echo_name(const char* name);"));
    assert!(header.contains("void corvid_free_string(const char* value);"));
}

#[test]
fn header_emits_nothing_return_as_void() {
    let header = render();
    assert!(header.contains("void touch(void);"));
}
