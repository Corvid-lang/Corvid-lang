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

/// Locate the system stdlib directory shipped alongside the corvid binary.
/// The Corvid import resolver is purely relative to the importing `.cor`
/// file, so `std/` must be vendored into each project that uses it. This
/// helper finds the source copy in two places, in order:
///
/// 1. `$CORVID_HOME/std` — explicit override set by the installer.
/// 2. `<exe-dir>/../std` — the layout produced by the install bootstrap
///    (`~/.corvid/bin/corvid` → `~/.corvid/std`).
///
/// Returns `None` when neither candidate resolves to a directory, in
/// which case [`vendor_std`] becomes a no-op.
pub fn find_std_source() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("CORVID_HOME") {
        let candidate = PathBuf::from(home).join("std");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let candidate = exe.parent()?.parent()?.join("std");
    candidate.is_dir().then_some(candidate)
}

/// Copy the system stdlib into a fresh project so `import "./std/foo"`
/// works without users having to clone the language repository. Returns
/// the source path that was vendored from, or `None` if nothing was done
/// (no source found, or the destination already exists). Errors propagate
/// as filesystem failures during the copy.
pub fn vendor_std(project_root: &Path) -> anyhow::Result<Option<PathBuf>> {
    let dst = project_root.join("std");
    if dst.exists() {
        return Ok(None);
    }
    let Some(src) = find_std_source() else {
        return Ok(None);
    };
    vendor_std_from(&src, &dst)?;
    Ok(Some(src))
}

/// Recursive directory copy used by [`vendor_std`]. Exposed separately so
/// tests can drive it without touching `$CORVID_HOME` or the executable
/// path (both of which are process-global and racy under parallel tests).
pub fn vendor_std_from(src: &Path, dst: &Path) -> anyhow::Result<()> {
    copy_dir_recursive(src, dst)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}
