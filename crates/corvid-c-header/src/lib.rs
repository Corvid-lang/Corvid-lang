mod scalar_marshal;
mod template;

use corvid_ir::{IrAgent, IrExternAbi, IrFile};
pub use scalar_marshal::ScalarAbiType;

#[derive(Debug, Clone)]
pub struct HeaderOptions {
    pub library_name: String,
}

#[derive(Debug, Clone)]
pub struct HeaderAgent {
    pub name: String,
    pub signature_comment: String,
    pub return_c_type: &'static str,
    pub params_c: String,
}

pub fn emit_header(ir: &IrFile, opts: &HeaderOptions) -> String {
    let agents = ir
        .agents
        .iter()
        .filter(|agent| matches!(agent.extern_abi, Some(IrExternAbi::C)))
        .map(exported_agent)
        .collect::<Vec<_>>();
    template::render_header(opts, &agents)
}

fn exported_agent(agent: &IrAgent) -> HeaderAgent {
    let params_c = if agent.params.is_empty() {
        "void".to_string()
    } else {
        agent
            .params
            .iter()
            .map(|param| {
                let c_ty = ScalarAbiType::from_type(&param.ty)
                    .expect("extern-c checker guarantees scalar params")
                    .c_param_type();
                format!("{c_ty} {}", param.name)
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let return_c_type = ScalarAbiType::from_type(&agent.return_ty)
        .expect("extern-c checker guarantees scalar return")
        .c_return_type();
    let signature_comment = format!(
        "agent {}({}) -> {}",
        agent.name,
        agent
            .params
            .iter()
            .map(|param| format!("{}: {}", param.name, param.ty.display_name()))
            .collect::<Vec<_>>()
            .join(", "),
        agent.return_ty.display_name()
    );
    HeaderAgent {
        name: agent.name.clone(),
        signature_comment,
        return_c_type,
        params_c,
    }
}
