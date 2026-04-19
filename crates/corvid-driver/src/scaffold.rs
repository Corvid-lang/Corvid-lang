//! Project-scaffolding helpers — `corvid new <name>` creates a minimal
//! Corvid project directory with `corvid.toml`, `src/main.cor`, and
//! a starter `.gitignore`.
//!
//! Extracted from `lib.rs` as part of Phase 20i responsibility
//! decomposition (20i-audit-driver-c).

use std::path::{Path, PathBuf};

/// Scaffold a new Corvid project at `<name>/` under the current directory.
pub fn scaffold_new(name: &str) -> anyhow::Result<PathBuf> {
    scaffold_new_in(&std::env::current_dir()?, name)
}

/// Scaffold a new Corvid project named `<name>` under `parent`.
pub fn scaffold_new_in(parent: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let root = parent.join(name);
    if root.exists() {
        anyhow::bail!("directory `{}` already exists", root.display());
    }
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::write(
        root.join("corvid.toml"),
        format!(
            r#"name = "{name}"
version = "0.1.0"

[llm]
# No default model is set. Pick one explicitly:
#   default_model = "claude-opus-4-6"
"#
        ),
    )?;
    std::fs::write(
        root.join(".gitignore"),
        "/target\n__pycache__/\n*.pyc\n",
    )?;
    std::fs::write(
        root.join("src").join("main.cor"),
        r#"# Your first Corvid agent.

tool echo(message: String) -> String

agent greet(name: String) -> String:
    message = echo(name)
    return message
"#,
    )?;
    std::fs::write(
        root.join("tools.py"),
        r#"# Implement your Corvid tools here.
from corvid_runtime import tool


@tool("echo")
async def echo(message: str) -> str:
    return message
"#,
    )?;
    Ok(root)
}
