use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result};
use corvid_bind::{generate_bindings_from_descriptor_path, BindLanguage};

pub fn run_bind(language: &str, descriptor: &Path, out: &Path) -> Result<u8> {
    let language = BindLanguage::from_str(language).map_err(anyhow::Error::msg)?;
    let generated = generate_bindings_from_descriptor_path(language, descriptor, out)
        .with_context(|| format!("generate bindings from `{}`", descriptor.display()))?;
    println!(
        "generated {} file(s) under {}",
        generated.files.len(),
        out.display()
    );
    Ok(0)
}
