//! Default source-file resolution for project-local commands.

use anyhow::{Context, Result};
use std::path::PathBuf;

pub(crate) fn resolve_project_source(file: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(file) = file {
        return Ok(file);
    }

    let cwd = std::env::current_dir().context("resolve current directory")?;
    let manifest = cwd.join("corvid.toml");
    let source = cwd.join("src").join("main.cor");
    if manifest.exists() && source.exists() {
        return Ok(source);
    }

    anyhow::bail!(
        "no source file supplied and `{}` does not exist; pass a file or run from a Corvid project with `corvid.toml` and `src/main.cor`",
        source.display()
    )
}
