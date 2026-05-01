use std::path::PathBuf;

use anyhow::Result;

use crate::{is_generated_user_agent, snake_case, BindingContext, GeneratedFile};

pub(crate) fn render(context: &BindingContext) -> Result<Vec<GeneratedFile>> {
    let mut files = Vec::new();
    files.push(GeneratedFile {
        relative_path: PathBuf::from("Cargo.toml"),
        contents: render_cargo_toml(context),
    });
    files.push(GeneratedFile {
        relative_path: PathBuf::from("README.md"),
        contents: render_readme(context),
    });
    files.push(GeneratedFile {
        relative_path: PathBuf::from("src/lib.rs"),
        contents: render_lib_rs(context),
    });
    files.push(GeneratedFile {
        relative_path: PathBuf::from("src/common.rs"),
        contents: render_common_rs(context),
    });
    files.push(GeneratedFile {
        relative_path: PathBuf::from("src/types.rs"),
        contents: render_types_rs(context),
    });
    files.push(GeneratedFile {
        relative_path: PathBuf::from("src/catalog.rs"),
        contents: render_catalog_rs(),
    });
    for agent in context
        .abi
        .agents
        .iter()
        .filter(|agent| is_generated_user_agent(agent))
    {
        let module_name = snake_case(&agent.name);
        files.push(GeneratedFile {
            relative_path: PathBuf::from(format!("src/{module_name}.rs")),
            contents: render_agent_module(context, agent)?,
        });
    }
    Ok(files)
}


mod agent_emit;
mod cargo;
mod common_template;
mod lib_template;
mod types_template;
use agent_emit::render_agent_module;
use cargo::{render_cargo_toml, render_readme};
use common_template::render_common_rs;
use lib_template::render_lib_rs;
use types_template::{render_catalog_rs, render_types_rs};

